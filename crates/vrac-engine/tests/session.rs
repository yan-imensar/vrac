use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::session::{ConflictAction, ConflictType, Session};
use rusqlite::{Connection, params};
use tempfile::tempdir;
use vrac_engine::{CreateNode, Engine};

fn raw_connection(path: &std::path::Path) -> Connection {
    let connection = Connection::open(path).expect("open raw workspace");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    connection
}

fn session(connection: &Connection) -> Session<'_> {
    let mut session = Session::new(connection).expect("create session");
    for table in ["nodes", "node_tags", "node_references"] {
        session.attach(Some(table)).expect("attach canonical table");
    }
    session
}

#[test]
fn sqlite_session_captures_the_complete_canonical_model() {
    let directory = tempdir().expect("create temporary directory");
    let first_path = directory.path().join("first.vrac");
    let second_path = directory.path().join("second.vrac");
    let mut engine = Engine::open(&first_path).expect("open first workspace");
    let target = engine
        .create_node(CreateNode::new("Project X"))
        .expect("create target");
    engine
        .checkpoint(&second_path)
        .expect("clone initial workspace");
    drop(engine);

    let first = raw_connection(&first_path);
    let mut captured = session(&first);
    let transaction = first
        .unchecked_transaction()
        .expect("start captured transaction");
    let source_id = [42_u8; 16];
    transaction
        .execute(
            "INSERT INTO nodes (id, parent_id, position, text)
             VALUES (?1, ?2, 1024, 'Decision on [[Project X]]')",
            params![&source_id[..], &target.id.as_bytes()[..]],
        )
        .expect("insert source");
    for tag in ["decision", "meeting"] {
        transaction
            .execute(
                "INSERT INTO node_tags (node_id, tag) VALUES (?1, ?2)",
                params![&source_id[..], tag],
            )
            .expect("insert tag");
    }
    transaction
        .execute(
            "INSERT INTO node_references (source_id, start_byte, end_byte, target_id)
             VALUES (?1, 14, 23, ?2)",
            params![&source_id[..], &target.id.as_bytes()[..]],
        )
        .expect("insert reference");
    transaction
        .execute(
            "UPDATE nodes SET text = 'Project Y' WHERE id = ?1",
            params![&target.id.as_bytes()[..]],
        )
        .expect("rename target");
    let mut changeset = Vec::new();
    captured
        .changeset_strm(&mut changeset)
        .expect("capture changeset");
    transaction.commit().expect("commit source transaction");
    drop(captured);
    drop(first);
    assert!(!changeset.is_empty());

    let second = raw_connection(&second_path);
    second
        .apply_strm(
            &mut changeset.as_slice(),
            None::<fn(&str) -> bool>,
            |_kind, _item| ConflictAction::SQLITE_CHANGESET_ABORT,
        )
        .expect("apply changeset");
    drop(second);

    let second = Engine::open(&second_path).expect("open synchronized workspace");
    let source = second
        .node(vrac_engine::NodeId::from_bytes(source_id))
        .expect("read source")
        .expect("source exists");
    assert_eq!(source.tags, ["decision", "meeting"]);
    assert_eq!(source.references[0].target_text, "Project Y");
    assert!(
        second
            .check()
            .expect("check synchronized workspace")
            .is_ok()
    );
}

#[test]
fn non_conflicting_changes_merge_and_real_conflicts_abort_atomically() {
    let directory = tempdir().expect("create temporary directory");
    let first_path = directory.path().join("first.vrac");
    let second_path = directory.path().join("second.vrac");
    let mut engine = Engine::open(&first_path).expect("open first workspace");
    let first_node = engine
        .create_node(CreateNode::new("First"))
        .expect("create first node");
    let second_node = engine
        .create_node(CreateNode::new("Second"))
        .expect("create second node");
    engine
        .checkpoint(&second_path)
        .expect("clone initial workspace");
    drop(engine);

    let first = raw_connection(&first_path);
    let mut captured = session(&first);
    first
        .execute(
            "UPDATE nodes
             SET parent_id = ?1, position = 0, text = 'First from A'
             WHERE id = ?2",
            params![
                &second_node.id.as_bytes()[..],
                &first_node.id.as_bytes()[..]
            ],
        )
        .expect("move and edit first node on A");
    let mut changeset = Vec::new();
    captured
        .changeset_strm(&mut changeset)
        .expect("capture A changeset");
    drop(captured);
    drop(first);

    let second = raw_connection(&second_path);
    second
        .execute(
            "UPDATE nodes SET text = 'Second from B' WHERE id = ?1",
            params![&second_node.id.as_bytes()[..]],
        )
        .expect("edit second node on B");
    second
        .apply_strm(
            &mut changeset.as_slice(),
            None::<fn(&str) -> bool>,
            |_kind, _item| ConflictAction::SQLITE_CHANGESET_ABORT,
        )
        .expect("merge independent edit");
    assert_eq!(
        second
            .query_row(
                "SELECT text FROM nodes WHERE id = ?1",
                params![&first_node.id.as_bytes()[..]],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "First from A"
    );
    assert_eq!(
        second
            .query_row(
                "SELECT parent_id FROM nodes WHERE id = ?1",
                params![&first_node.id.as_bytes()[..]],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap(),
        second_node.id.as_bytes()
    );

    let first = raw_connection(&first_path);
    let mut captured = session(&first);
    first
        .execute(
            "UPDATE nodes SET text = 'Conflict from A' WHERE id = ?1",
            params![&second_node.id.as_bytes()[..]],
        )
        .expect("edit conflicting node on A");
    let mut conflict = Vec::new();
    captured
        .changeset_strm(&mut conflict)
        .expect("capture conflicting changeset");
    drop(captured);
    drop(first);

    static CONFLICT_SEEN: AtomicBool = AtomicBool::new(false);
    let error = second
        .apply_strm(
            &mut conflict.as_slice(),
            None::<fn(&str) -> bool>,
            |kind, _item| {
                CONFLICT_SEEN.store(true, Ordering::Relaxed);
                assert_eq!(kind, ConflictType::SQLITE_CHANGESET_DATA);
                ConflictAction::SQLITE_CHANGESET_ABORT
            },
        )
        .expect_err("abort true conflict");
    assert!(CONFLICT_SEEN.load(Ordering::Relaxed));
    assert!(error.to_string().contains("abort"));
    assert_eq!(
        second
            .query_row(
                "SELECT text FROM nodes WHERE id = ?1",
                params![&second_node.id.as_bytes()[..]],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "Second from B"
    );
}
