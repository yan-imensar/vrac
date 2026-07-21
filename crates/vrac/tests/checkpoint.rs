use rusqlite::{Connection, params};
use tempfile::tempdir;
use vrac::{CheckIssue, CreateNode, Engine, Error, ReferenceInput, SyncDeviceId};

#[test]
fn a_checkpoint_is_an_independent_valid_snapshot_of_an_open_workspace() {
    let directory = tempdir().expect("create temporary directory");
    let source_path = directory.path().join("source.vrac");
    let checkpoint_path = directory.path().join("checkpoint.vrac");
    let mut engine = Engine::open(&source_path).expect("open source");

    let mut target_input = CreateNode::new("Project X");
    target_input.tags = vec!["project".into()];
    let target = engine.create_node(target_input).expect("create target");
    let text = "Decision about [[Project X]]";
    let start = text.find("Project X").expect("find reference label");
    let mut source_input = CreateNode::new(text);
    source_input.tags = vec!["decision".into(), "meeting".into()];
    source_input.references = vec![ReferenceInput {
        label_start: start,
        label_end: start + "Project X".len(),
        target_id: target.id,
    }];
    let source = engine.create_node(source_input).expect("create source");

    engine
        .checkpoint(&checkpoint_path)
        .expect("create checkpoint");
    engine
        .set_text(target.id, "Project Y".into())
        .expect("change active workspace");
    let later = engine
        .create_node(CreateNode::new("Created later"))
        .expect("create later node");

    let checkpoint = Engine::open(&checkpoint_path).expect("open checkpoint");
    assert_eq!(
        checkpoint.node(target.id).unwrap().unwrap().text,
        "Project X"
    );
    let source = checkpoint.node(source.id).unwrap().unwrap();
    assert_eq!(source.tags, ["decision", "meeting"]);
    assert_eq!(source.references[0].target_text, "Project X");
    assert!(checkpoint.node(later.id).unwrap().is_none());
    let report = checkpoint.check().expect("check checkpoint");
    assert!(report.is_ok());
    assert_eq!(report.node_count, 3);
}

#[test]
fn checkpoint_never_overwrites_an_existing_destination() {
    let directory = tempdir().expect("create temporary directory");
    let destination = directory.path().join("existing.vrac");
    std::fs::write(&destination, b"keep me").expect("create destination");
    let engine = Engine::open(":memory:").expect("open source");

    assert!(matches!(
        engine.checkpoint(&destination),
        Err(Error::CheckpointDestinationExists)
    ));
    assert_eq!(std::fs::read(destination).unwrap(), b"keep me");
}

#[test]
fn checkpoint_leaves_no_temporary_sidecars() {
    let directory = tempdir().expect("create temporary directory");
    let destination = directory.path().join("checkpoint.vrac");
    let engine = Engine::open(":memory:").expect("open source");

    engine.checkpoint(&destination).expect("create checkpoint");

    let files: Vec<_> = std::fs::read_dir(directory.path())
        .expect("read checkpoint directory")
        .map(|entry| entry.expect("read directory entry").file_name())
        .collect();
    assert_eq!(files, ["checkpoint.vrac"]);
}

#[test]
fn an_invalid_snapshot_is_not_published() {
    let directory = tempdir().expect("create temporary directory");
    let source_path = directory.path().join("source.vrac");
    let destination = directory.path().join("invalid-checkpoint.vrac");
    let mut engine = Engine::open(&source_path).expect("open source");
    let node = engine
        .create_node(CreateNode::new("Node"))
        .expect("create node");

    let connection = Connection::open(&source_path).expect("open raw source");
    connection
        .execute(
            "INSERT INTO node_tags (node_id, tag) VALUES (?1, 'Not-Canonical')",
            params![&node.id.as_bytes()[..]],
        )
        .expect("corrupt source");
    drop(connection);

    let error = engine
        .checkpoint(&destination)
        .expect_err("reject invalid checkpoint");
    assert!(matches!(
        error,
        Error::InvalidCheckpoint(report)
            if report.issues.iter().any(|issue| matches!(
                issue,
                CheckIssue::NonCanonicalTag { node_id, .. } if *node_id == node.id
            ))
    ));
    assert!(!destination.exists());
}

#[test]
fn a_checkpoint_is_not_published_when_the_search_index_has_drifted() {
    let directory = tempdir().expect("create temporary directory");
    let source_path = directory.path().join("source.vrac");
    let destination = directory.path().join("invalid-checkpoint.vrac");
    let mut engine = Engine::open(&source_path).expect("open source");
    let node = engine
        .create_node(CreateNode::new("Searchable text"))
        .expect("create node");

    let connection = Connection::open(&source_path).expect("open raw source");
    let rowid: i64 = connection
        .query_row(
            "SELECT rowid FROM nodes WHERE id = ?1",
            params![&node.id.as_bytes()[..]],
            |row| row.get(0),
        )
        .expect("read node rowid");
    connection
        .execute(
            "INSERT INTO node_search(node_search, rowid, text)
             VALUES ('delete', ?1, ?2)",
            params![rowid, node.text],
        )
        .expect("remove search index entry");
    drop(connection);

    assert!(matches!(
        engine.checkpoint(&destination),
        Err(Error::Sqlite(_))
    ));
    assert!(!destination.exists());
}

#[test]
fn restoring_a_checkpoint_is_atomic_and_keeps_the_replaced_state_recoverable() {
    let directory = tempdir().expect("create temporary directory");
    let source_path = directory.path().join("source.vrac");
    let checkpoint_path = directory.path().join("checkpoint.vrac");
    let recovery_path = directory.path().join("recovery.vrac");
    let mut engine =
        Engine::open_synced(&source_path, SyncDeviceId::from_bytes([1; 16])).expect("open source");
    let original = engine
        .create_node(CreateNode::new("Original"))
        .expect("create original node");
    engine
        .checkpoint(&checkpoint_path)
        .expect("create checkpoint");
    engine
        .set_tags(original.id, vec!["changed".into()])
        .expect("change original node");
    let later = engine
        .create_node(CreateNode::new("Created later"))
        .expect("create later node");

    engine
        .restore_checkpoint(&checkpoint_path, &recovery_path)
        .expect("restore checkpoint");

    assert!(engine.node(later.id).unwrap().is_none());
    assert!(engine.node(original.id).unwrap().unwrap().tags.is_empty());
    assert!(!engine.undo().expect("restore cleared session history"));
    assert!(engine.has_pending_sync_changes().unwrap());
    let recovery = Engine::open(&recovery_path).expect("open recovery checkpoint");
    assert_eq!(
        recovery.node(later.id).unwrap().unwrap().text,
        "Created later"
    );
    assert_eq!(
        recovery.node(original.id).unwrap().unwrap().tags,
        ["changed"]
    );
}

#[test]
fn a_checkpoint_from_another_workspace_cannot_be_restored() {
    let directory = tempdir().expect("create temporary directory");
    let source_path = directory.path().join("source.vrac");
    let foreign_path = directory.path().join("foreign.vrac");
    let recovery_path = directory.path().join("recovery.vrac");
    let mut source = Engine::open(&source_path).expect("open source");
    let foreign =
        Engine::open(directory.path().join("foreign-source.vrac")).expect("open foreign workspace");
    foreign
        .checkpoint(&foreign_path)
        .expect("create foreign checkpoint");

    assert!(matches!(
        source.restore_checkpoint(&foreign_path, &recovery_path),
        Err(Error::CheckpointWorkspaceMismatch)
    ));
    assert!(!recovery_path.exists());
}

#[test]
fn an_active_workspace_cannot_be_used_as_its_own_restore_source() {
    let directory = tempdir().expect("create temporary directory");
    let source_path = directory.path().join("source.vrac");
    let recovery_path = directory.path().join("recovery.vrac");
    let mut source = Engine::open(&source_path).expect("open source");
    source
        .create_node(CreateNode::new("Keep me"))
        .expect("create node");

    assert!(matches!(
        source.restore_checkpoint(&source_path, &recovery_path),
        Err(Error::RestoreSourceIsActiveWorkspace)
    ));
    assert!(!recovery_path.exists());
    assert_eq!(source.check().unwrap().node_count, 2);
}
