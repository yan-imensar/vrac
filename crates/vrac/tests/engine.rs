use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use tempfile::TempDir;
use vrac::{
    CheckIssue, CreateNode, Cursor, Destination, Engine, Error, GenerateShape, Node, NodeId, Page,
    Placement,
};

const VRAC_APPLICATION_ID: i64 = 0x5652_4143;

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
    placement: Placement,
    text: &str,
) -> Node {
    engine
        .create_node(CreateNode {
            parent_id,
            placement,
            ..CreateNode::new(text)
        })
        .expect("create node")
}

fn children(engine: &Engine, parent_id: Option<NodeId>) -> Vec<Node> {
    engine
        .children(parent_id, Page::default())
        .expect("read children")
        .nodes
        .into_iter()
        .filter(|node| node.system.is_none())
        .collect()
}

#[test]
fn a_node_can_be_read_after_reopening_the_database() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let workspace_id = engine.workspace_id().expect("read workspace identity");
    let root = create_node(&mut engine, None, Placement::Last, "root");
    let child = create_node(&mut engine, Some(root.id), Placement::Last, "draft");
    engine
        .set_text(child.id, "final".into())
        .expect("update text");
    drop(engine);

    let engine = Engine::open(database.path()).expect("reopen database");
    assert_eq!(engine.workspace_id().unwrap(), workspace_id);
    assert_eq!(
        engine.node(root.id).expect("read root"),
        Some(Node {
            has_children: true,
            ..root.clone()
        })
    );
    assert_eq!(
        engine.node(child.id).expect("read child"),
        Some(Node {
            text: "final".into(),
            ..child
        })
    );
}

#[test]
fn workspace_identifiers_round_trip_without_panicking_on_invalid_unicode() {
    let id = vrac::WorkspaceId::from_bytes([0xab; vrac::WORKSPACE_ID_LENGTH]);
    assert_eq!(id.to_string().parse(), Ok(id));
    assert!("é".repeat(16).parse::<vrac::WorkspaceId>().is_err());
}

#[test]
fn new_databases_have_stable_format_markers_and_wal() {
    let database = TestDatabase::new();
    let engine = Engine::open(database.path()).expect("open database");
    drop(engine);

    let connection = Connection::open(database.path()).expect("open raw SQLite database");
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .expect("read application ID");
    let schema_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read schema version");
    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("read journal mode");

    assert_eq!(application_id, VRAC_APPLICATION_ID);
    assert_eq!(schema_version, 3);
    assert_eq!(journal_mode, "wal");
}

#[test]
fn version_two_workspaces_add_the_root_concept_index_without_losing_data() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let node = engine
        .create_node(CreateNode::new("Preserved"))
        .expect("create node");
    drop(engine);

    let connection = Connection::open(database.path()).expect("open raw SQLite database");
    connection
        .execute("DROP INDEX nodes_by_root_text", [])
        .expect("restore version two schema");
    connection
        .pragma_update(None, "user_version", 2)
        .expect("mark version two");
    drop(connection);

    let engine = Engine::open(database.path()).expect("migrate workspace");
    assert_eq!(engine.node(node.id).unwrap().unwrap().text, "Preserved");
    drop(engine);
    let connection = Connection::open(database.path()).expect("inspect migration");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read schema version");
    let index_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'nodes_by_root_text')",
            [],
            |row| row.get(0),
        )
        .expect("read root concept index");
    assert_eq!(version, 3);
    assert!(index_exists);
}

#[test]
fn valid_unmarked_current_databases_are_adopted() {
    let database = TestDatabase::new();
    let engine = Engine::open(database.path()).expect("open database");
    drop(engine);

    let connection = Connection::open(database.path()).expect("open raw SQLite database");
    connection
        .pragma_update(None, "application_id", 0)
        .expect("remove application ID");
    drop(connection);

    let engine = Engine::open(database.path()).expect("adopt unmarked database");
    drop(engine);
    let connection = Connection::open(database.path()).expect("reopen raw SQLite database");
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .expect("read application ID");
    assert_eq!(application_id, VRAC_APPLICATION_ID);
}

#[test]
fn foreign_application_ids_are_rejected_without_modification() {
    let database = TestDatabase::new();
    let engine = Engine::open(database.path()).expect("open database");
    drop(engine);

    let connection = Connection::open(database.path()).expect("open raw SQLite database");
    connection
        .pragma_update(None, "application_id", 0x1234_5678_i64)
        .expect("replace application ID");
    drop(connection);

    assert!(matches!(
        Engine::open(database.path()),
        Err(Error::InvalidDatabase(reason)) if reason.contains("application ID")
    ));
    let connection = Connection::open(database.path()).expect("reopen raw SQLite database");
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .expect("read application ID");
    assert_eq!(application_id, 0x1234_5678);
}

#[test]
fn schema_mismatches_are_rejected_before_adopting_an_unmarked_database() {
    let database = TestDatabase::new();
    let engine = Engine::open(database.path()).expect("open database");
    drop(engine);

    let connection = Connection::open(database.path()).expect("open raw SQLite database");
    connection
        .execute_batch(
            "DROP INDEX nodes_by_parent;
             CREATE INDEX nodes_by_parent ON nodes(parent_id, id);
             PRAGMA application_id = 0;",
        )
        .expect("alter schema");
    drop(connection);

    assert!(matches!(
        Engine::open(database.path()),
        Err(Error::InvalidDatabase(reason)) if reason.contains("nodes_by_parent")
    ));
    let connection = Connection::open(database.path()).expect("reopen raw SQLite database");
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .expect("read application ID");
    assert_eq!(application_id, 0);
}

#[test]
fn invalid_sqlite_files_are_rejected_without_overwriting_them() {
    let database = TestDatabase::new();
    let contents = b"this is not a SQLite database";
    std::fs::write(database.path(), contents).expect("write invalid database");

    assert!(Engine::open(database.path()).is_err());
    assert_eq!(
        std::fs::read(database.path()).expect("read invalid database"),
        contents
    );
}

#[test]
fn relative_placement_and_cursor_pagination_preserve_exact_order() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let middle = create_node(&mut engine, None, Placement::Last, "middle");
    let first = create_node(&mut engine, None, Placement::First, "first");
    let after_first = create_node(&mut engine, None, Placement::After(first.id), "after first");
    let before_middle = create_node(
        &mut engine,
        None,
        Placement::Before(middle.id),
        "before middle",
    );
    let last = create_node(&mut engine, None, Placement::Last, "last");
    let expected = vec![first, after_first, before_middle, middle, last];

    let first_page = engine
        .children(
            None,
            Page {
                limit: 2,
                after: None,
            },
        )
        .expect("read first page");
    assert!(first_page.next.is_some());
    let cursor: Cursor = first_page
        .next
        .expect("first cursor")
        .to_string()
        .parse()
        .expect("parse opaque cursor");
    let second_page = engine
        .children(
            None,
            Page {
                limit: 2,
                after: Some(cursor),
            },
        )
        .expect("read second page");
    assert!(second_page.next.is_some());
    let third_page = engine
        .children(
            None,
            Page {
                limit: 2,
                after: second_page.next,
            },
        )
        .expect("read final page");
    assert!(third_page.next.is_none());

    let actual: Vec<Node> = first_page
        .nodes
        .into_iter()
        .chain(second_page.nodes)
        .chain(third_page.nodes)
        .filter(|node| node.system.is_none())
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn malformed_textual_cursors_are_rejected() {
    for value in [
        "",
        "v2:0000000000000000:00000000000000000000000000000000",
        "v1:0:00000000000000000000000000000000",
        "v1:0000000000000000:not-an-id",
        "v1:0000000000000000:00000000000000000000000000000000:extra",
    ] {
        assert!(value.parse::<Cursor>().is_err(), "accepted {value:?}");
    }
}

#[test]
fn exhausted_position_gaps_renumber_only_the_affected_siblings() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let parent = create_node(&mut engine, None, Placement::Last, "parent");
    let unrelated_parent = create_node(&mut engine, None, Placement::Last, "unrelated parent");
    let unrelated_child = create_node(
        &mut engine,
        Some(unrelated_parent.id),
        Placement::Last,
        "unrelated child",
    );
    let first = create_node(&mut engine, Some(parent.id), Placement::First, "first");
    let last = create_node(&mut engine, Some(parent.id), Placement::Last, "last");

    let mut inserted = Vec::new();
    for index in 0..20 {
        inserted.push(create_node(
            &mut engine,
            Some(parent.id),
            Placement::After(first.id),
            &format!("inserted {index}"),
        ));
    }

    let mut expected = vec![first.clone()];
    expected.extend(inserted.into_iter().rev());
    expected.push(last.clone());
    assert_eq!(children(&engine, Some(parent.id)), expected);
    assert_eq!(
        children(&engine, Some(unrelated_parent.id)),
        vec![unrelated_child]
    );
    drop(engine);

    let connection = Connection::open(database.path()).expect("open raw SQLite database");
    let (count, distinct_positions, last_position): (i64, i64, i64) = connection
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT position),
                    MAX(CASE WHEN id = ?1 THEN position END)
             FROM nodes
             WHERE parent_id = ?2",
            params![&last.id.as_bytes()[..], &parent.id.as_bytes()[..]],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read sibling positions");
    assert_eq!(count, 22);
    assert_eq!(distinct_positions, count);
    assert!(last_position > 1_024, "the sibling list was not renumbered");
}

#[test]
fn placement_references_must_exist_in_the_destination_sibling_list() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let parent = create_node(&mut engine, None, Placement::Last, "parent");
    let child = create_node(&mut engine, Some(parent.id), Placement::Last, "child");
    let absent = NodeId::from_bytes([42; 16]);

    assert!(matches!(
        engine.create_node(CreateNode {
            parent_id: None,
            placement: Placement::Before(child.id),
            ..CreateNode::new("misplaced")
        }),
        Err(Error::PlacementReferenceNotSibling {
            reference,
            parent_id: None,
        }) if reference == child.id
    ));
    assert!(matches!(
        engine.create_node(CreateNode {
            parent_id: None,
            placement: Placement::After(absent),
            ..CreateNode::new("missing reference")
        }),
        Err(Error::NodeNotFound(id)) if id == absent
    ));
    assert_eq!(engine.check().expect("check database").node_count, 3);
}

#[test]
fn invalid_parents_and_page_limits_are_rejected() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let absent = NodeId::from_bytes([42; 16]);

    assert!(matches!(
        engine.create_node(CreateNode {
            parent_id: Some(absent),
            placement: Placement::Last,
            ..CreateNode::new("orphan")
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
    assert_eq!(engine.check().expect("check database").node_count, 1);
}

#[test]
fn moves_support_relative_placement_and_self_references_are_noops() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let first = create_node(&mut engine, None, Placement::Last, "first");
    let second = create_node(&mut engine, None, Placement::Last, "second");
    let third = create_node(&mut engine, None, Placement::Last, "third");

    engine
        .move_node(
            third.id,
            Destination {
                parent_id: None,
                placement: Placement::First,
            },
        )
        .expect("move third first");
    assert_eq!(
        children(&engine, None),
        vec![third.clone(), first.clone(), second.clone()]
    );

    engine
        .move_node(
            third.id,
            Destination {
                parent_id: None,
                placement: Placement::After(second.id),
            },
        )
        .expect("move third after second");
    let expected = vec![first.clone(), second, third.clone()];
    assert_eq!(children(&engine, None), expected);

    engine
        .move_node(
            third.id,
            Destination {
                parent_id: None,
                placement: Placement::Before(third.id),
            },
        )
        .expect("move relative to self");
    assert_eq!(children(&engine, None), expected);
}

#[test]
fn cyclic_moves_are_rejected_without_changing_the_tree() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let root = create_node(&mut engine, None, Placement::Last, "root");
    let child = create_node(&mut engine, Some(root.id), Placement::Last, "child");
    let grandchild = create_node(&mut engine, Some(child.id), Placement::Last, "grandchild");

    assert!(matches!(
        engine.move_node(
            root.id,
            Destination {
                parent_id: Some(grandchild.id),
                placement: Placement::Last,
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
    let first_root = create_node(&mut engine, None, Placement::Last, "first root");
    let second_root = create_node(&mut engine, None, Placement::Last, "second root");
    let child = create_node(&mut engine, Some(first_root.id), Placement::Last, "child");
    let grandchild = create_node(&mut engine, Some(child.id), Placement::Last, "grandchild");

    engine
        .move_node(
            child.id,
            Destination {
                parent_id: Some(second_root.id),
                placement: Placement::Last,
            },
        )
        .expect("move subtree");

    assert_eq!(
        engine
            .node(child.id)
            .expect("read child")
            .unwrap()
            .parent_id,
        Some(second_root.id)
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
fn a_generation_error_rolls_back_every_insert_in_its_transaction() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let existing = create_node(&mut engine, None, Placement::Last, "near the end");
    drop(engine);

    let connection = Connection::open(database.path()).expect("open raw SQLite database");
    connection
        .execute(
            "UPDATE nodes SET position = ?1 WHERE id = ?2",
            params![i64::MAX - 2_048, &existing.id.as_bytes()[..]],
        )
        .expect("move position near integer limit");
    drop(connection);

    let mut engine = Engine::open(database.path()).expect("reopen database");
    assert!(matches!(
        engine.generate_nodes(3, GenerateShape::Wide),
        Err(Error::PositionOverflow)
    ));

    let report = engine.check().expect("check database");
    assert!(report.is_ok());
    assert_eq!(report.node_count, 2);
}

#[test]
fn check_detects_a_cycle_inserted_outside_the_engine() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let first = create_node(&mut engine, None, Placement::Last, "first");
    let second = create_node(&mut engine, Some(first.id), Placement::Last, "second");
    let separate_root = create_node(&mut engine, None, Placement::Last, "separate root");
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
    assert_eq!(report.node_count, 4);
    assert!(matches!(
        report.issues.as_slice(),
        [CheckIssue::UnreachableNodes(2)]
    ));

    assert!(matches!(
        engine.move_node(
            separate_root.id,
            Destination {
                parent_id: Some(first.id),
                placement: Placement::Last,
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
        assert_eq!(report.node_count, 112);
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
fn placement_queries_use_the_parent_order_index() {
    let database = TestDatabase::new();
    let engine = Engine::open(database.path()).expect("open database");
    drop(engine);

    let connection = Connection::open(database.path()).expect("open raw SQLite database");
    let excluded_id = NodeId::from_bytes([1; 16]);
    let reference_id = NodeId::from_bytes([2; 16]);
    let queries = [
        (
            "EXPLAIN QUERY PLAN
             SELECT id, position
             FROM nodes
             WHERE parent_id IS ?1 AND id IS NOT ?2
             ORDER BY position, id
             LIMIT 1",
            false,
        ),
        (
            "EXPLAIN QUERY PLAN
             SELECT id, position
             FROM nodes
             WHERE parent_id IS ?1
               AND id IS NOT ?2
               AND (position, id) > (?3, ?4)
             ORDER BY position, id
             LIMIT 1",
            true,
        ),
    ];

    for (sql, has_reference) in queries {
        let mut statement = connection.prepare(sql).expect("prepare placement query");
        let plan: Vec<String> = if has_reference {
            statement
                .query_map(
                    params![
                        Option::<&[u8]>::None,
                        &excluded_id.as_bytes()[..],
                        0,
                        &reference_id.as_bytes()[..]
                    ],
                    |row| row.get(3),
                )
                .expect("explain adjacent placement")
                .collect::<rusqlite::Result<_>>()
                .expect("read adjacent placement plan")
        } else {
            statement
                .query_map(
                    params![Option::<&[u8]>::None, &excluded_id.as_bytes()[..]],
                    |row| row.get(3),
                )
                .expect("explain edge placement")
                .collect::<rusqlite::Result<_>>()
                .expect("read edge placement plan")
        };

        assert!(
            plan.iter().any(|step| step.contains("nodes_by_parent")),
            "placement query does not use nodes_by_parent: {plan:?}"
        );
        assert!(
            plan.iter().all(|step| !step.contains("USE TEMP B-TREE")),
            "placement query sorts outside the index: {plan:?}"
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
    let connection = Connection::open(database.path()).expect("reopen raw SQLite database");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read schema version");
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .expect("read application ID");
    assert_eq!(version, 99);
    assert_eq!(application_id, 0);
}
