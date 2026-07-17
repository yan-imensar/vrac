use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use tempfile::TempDir;
use vrac::{
    CheckIssue, CreateNode, Cursor, Destination, Engine, Error, GenerateShape, Node, NodeId, Page,
};

struct TestDatabase {
    _directory: TempDir,
    path: PathBuf,
}

impl TestDatabase {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("create temporary directory");
        let path = directory.path().join("test.vrac");
        Self {
            _directory: directory,
            path,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

fn create_node(
    engine: &mut Engine,
    parent_id: Option<NodeId>,
    position: Option<i64>,
    text: &str,
) -> Node {
    engine
        .create_node(CreateNode {
            parent_id,
            position,
            text: text.into(),
        })
        .expect("create node")
}

#[test]
fn a_node_can_be_read_after_reopening_the_database() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let root = create_node(&mut engine, None, None, "root");
    let child = create_node(&mut engine, Some(root.id), None, "draft");
    engine
        .set_text(child.id, "final".into())
        .expect("update text");
    drop(engine);

    let engine = Engine::open(database.path()).expect("reopen database");
    assert_eq!(engine.node(root.id).expect("read root"), Some(root.clone()));
    assert_eq!(
        engine.node(child.id).expect("read child"),
        Some(Node {
            text: "final".into(),
            ..child
        })
    );
}

#[test]
fn children_are_deterministic_and_cursor_pagination_is_complete() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let mut expected = vec![
        create_node(&mut engine, None, Some(10), "ten-a"),
        create_node(&mut engine, None, Some(0), "zero"),
        create_node(&mut engine, None, Some(10), "ten-b"),
        create_node(&mut engine, None, Some(-5), "negative"),
    ];
    expected.sort_by_key(|node| (node.position, node.id));

    let first_page = engine
        .children(
            None,
            Page {
                limit: 2,
                after: None,
            },
        )
        .expect("read first page");
    let second_page = engine
        .children(
            None,
            Page {
                limit: 2,
                after: first_page.last().map(Cursor::from),
            },
        )
        .expect("read second page");
    let third_page = engine
        .children(
            None,
            Page {
                limit: 2,
                after: second_page.last().map(Cursor::from),
            },
        )
        .expect("read final page");

    let actual: Vec<Node> = first_page.into_iter().chain(second_page).collect();
    assert_eq!(actual, expected);
    assert!(third_page.is_empty());
}

#[test]
fn invalid_parents_and_page_limits_are_rejected() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let absent = NodeId::from_bytes([42; 16]);

    assert!(matches!(
        engine.create_node(CreateNode {
            parent_id: Some(absent),
            position: None,
            text: "orphan".into(),
        }),
        Err(Error::ParentNotFound(id)) if id == absent
    ));
    assert!(matches!(
        engine.children(
            None,
            Page {
                limit: 0,
                after: None,
            }
        ),
        Err(Error::InvalidPageLimit { limit: 0, .. })
    ));
    assert!(matches!(
        engine.children(Some(absent), Page::default()),
        Err(Error::ParentNotFound(id)) if id == absent
    ));
    assert_eq!(engine.check().expect("check database").node_count, 0);
}

#[test]
fn cyclic_moves_are_rejected_without_changing_the_tree() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let root = create_node(&mut engine, None, None, "root");
    let child = create_node(&mut engine, Some(root.id), None, "child");
    let grandchild = create_node(&mut engine, Some(child.id), None, "grandchild");

    assert!(matches!(
        engine.move_node(
            root.id,
            Destination {
                parent_id: Some(grandchild.id),
                position: None,
            }
        ),
        Err(Error::Cycle)
    ));

    assert_eq!(
        engine.node(root.id).expect("read root").unwrap().parent_id,
        None
    );
    assert_eq!(
        engine
            .node(grandchild.id)
            .expect("read grandchild")
            .unwrap()
            .parent_id,
        Some(child.id)
    );
}

#[test]
fn moving_a_subtree_does_not_rewrite_its_descendants() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let first_root = create_node(&mut engine, None, None, "first root");
    let second_root = create_node(&mut engine, None, None, "second root");
    let child = create_node(&mut engine, Some(first_root.id), None, "child");
    let grandchild = create_node(&mut engine, Some(child.id), None, "grandchild");

    engine
        .move_node(
            child.id,
            Destination {
                parent_id: Some(second_root.id),
                position: Some(7),
            },
        )
        .expect("move subtree");

    let moved = engine.node(child.id).expect("read child").unwrap();
    assert_eq!(moved.parent_id, Some(second_root.id));
    assert_eq!(moved.position, 7);
    assert_eq!(
        engine
            .node(grandchild.id)
            .expect("read grandchild")
            .unwrap()
            .parent_id,
        Some(child.id)
    );
}

#[test]
fn a_generation_error_rolls_back_every_insert_in_its_transaction() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    create_node(&mut engine, None, Some(i64::MAX - 2_048), "near the end");

    assert!(matches!(
        engine.generate_nodes(3, GenerateShape::Wide),
        Err(Error::PositionOverflow)
    ));

    let report = engine.check().expect("check database");
    assert!(report.is_ok());
    assert_eq!(report.node_count, 1);
}

#[test]
fn check_detects_a_cycle_inserted_outside_the_engine() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let first = create_node(&mut engine, None, None, "first");
    let second = create_node(&mut engine, Some(first.id), None, "second");
    let separate_root = create_node(&mut engine, None, None, "separate root");
    drop(engine);

    let connection = Connection::open(database.path()).expect("open raw SQLite database");
    connection
        .execute(
            "UPDATE nodes SET parent_id = ?1 WHERE id = ?2",
            params![&second.id.as_bytes()[..], &first.id.as_bytes()[..]],
        )
        .expect("create a cycle outside the engine");
    drop(connection);

    let mut engine = Engine::open(database.path()).expect("reopen database");
    let report = engine.check().expect("check database");
    assert_eq!(report.node_count, 3);
    assert!(matches!(
        report.issues.as_slice(),
        [CheckIssue::UnreachableNodes(2)]
    ));

    assert!(matches!(
        engine.move_node(
            separate_root.id,
            Destination {
                parent_id: Some(first.id),
                position: None,
            }
        ),
        Err(Error::InvalidDatabase(_))
    ));
    assert_eq!(
        engine
            .node(separate_root.id)
            .expect("read separate root")
            .unwrap()
            .parent_id,
        None
    );
}

#[test]
fn generators_create_valid_wide_deep_and_mixed_trees() {
    for shape in [
        GenerateShape::Wide,
        GenerateShape::Deep,
        GenerateShape::Mixed,
    ] {
        let database = TestDatabase::new();
        let mut engine = Engine::open(database.path()).expect("open database");
        engine.generate_nodes(111, shape).expect("generate nodes");
        let report = engine.check().expect("check generated tree");
        assert!(report.is_ok(), "invalid {shape:?} tree: {report:?}");
        assert_eq!(report.node_count, 111);
    }
}

#[test]
fn children_queries_use_the_parent_order_index() {
    let database = TestDatabase::new();
    let engine = Engine::open(database.path()).expect("open database");
    drop(engine);

    let connection = Connection::open(database.path()).expect("open raw SQLite database");
    let queries = [
        "EXPLAIN QUERY PLAN
         SELECT id, parent_id, position, text
         FROM nodes
         WHERE parent_id IS ?1
         ORDER BY position, id
         LIMIT ?2",
        "EXPLAIN QUERY PLAN
         SELECT id, parent_id, position, text
         FROM nodes
         WHERE parent_id IS ?1 AND (position, id) > (?2, ?3)
         ORDER BY position, id
         LIMIT ?4",
    ];

    let mut first = connection.prepare(queries[0]).expect("prepare first page");
    let first_plan: Vec<String> = first
        .query_map(params![Option::<&[u8]>::None, 100], |row| row.get(3))
        .expect("explain first page")
        .collect::<rusqlite::Result<_>>()
        .expect("read first page plan");

    let cursor_id = NodeId::from_bytes([0; 16]);
    let mut next = connection.prepare(queries[1]).expect("prepare next page");
    let next_plan: Vec<String> = next
        .query_map(
            params![Option::<&[u8]>::None, 0, &cursor_id.as_bytes()[..], 100],
            |row| row.get(3),
        )
        .expect("explain next page")
        .collect::<rusqlite::Result<_>>()
        .expect("read next page plan");

    for plan in [first_plan, next_plan] {
        assert!(
            plan.iter().any(|step| step.contains("nodes_by_parent")),
            "children query does not use nodes_by_parent: {plan:?}"
        );
        assert!(
            plan.iter().all(|step| !step.contains("USE TEMP B-TREE")),
            "children query sorts outside the index: {plan:?}"
        );
    }
}

#[test]
fn node_ids_have_a_stable_text_representation() {
    let id = NodeId::from_bytes([
        0x00, 0x01, 0x0f, 0x10, 0x2a, 0x7f, 0x80, 0xff, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
        0x88,
    ]);
    let encoded = id.to_string();
    assert_eq!(encoded, "00010f102a7f80ff1122334455667788");
    assert_eq!(encoded.parse::<NodeId>().expect("parse node id"), id);
}

#[test]
fn unknown_schema_versions_are_not_modified() {
    let database = TestDatabase::new();
    let connection = Connection::open(database.path()).expect("open raw SQLite database");
    connection
        .pragma_update(None, "user_version", 99)
        .expect("set future schema version");
    drop(connection);

    assert!(matches!(
        Engine::open(database.path()),
        Err(Error::UnsupportedSchemaVersion(99))
    ));
}
