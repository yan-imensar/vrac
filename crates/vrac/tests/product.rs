use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use tempfile::TempDir;
use vrac::{CheckIssue, CreateNode, Engine, Error, Node, NodeId, Page, ReferenceInput};

struct TestDatabase {
    _directory: TempDir,
    path: PathBuf,
}

impl TestDatabase {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("create temporary directory");
        let path = directory.path().join("product.vrac");
        Self {
            _directory: directory,
            path,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

fn reference(text: &str, label: &str, target_id: NodeId) -> ReferenceInput {
    let label_start = text.find(label).expect("reference label exists");
    ReferenceInput {
        label_start,
        label_end: label_start + label.len(),
        target_id,
    }
}

fn create(engine: &mut Engine, text: &str) -> Node {
    engine
        .create_node(CreateNode::new(text))
        .expect("create node")
}

#[test]
fn the_frozen_v2_fixture_reopens_with_resolved_content() {
    let database = TestDatabase::new();
    std::fs::write(
        database.path(),
        include_bytes!("fixtures/v2.vrac").as_slice(),
    )
    .expect("copy v2 fixture");

    let engine = Engine::open(database.path()).expect("open v2 fixture");
    let roots = engine
        .children(None, Page::default())
        .expect("read fixture roots")
        .nodes;
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].text, "Project X");
    assert_eq!(roots[0].tags, ["project"]);
    let children = engine
        .children(Some(roots[0].id), Page::default())
        .expect("read fixture children")
        .nodes;
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].tags, ["decision", "meeting"]);
    assert_eq!(children[0].references[0].target_id, roots[0].id);
    assert_eq!(children[0].references[0].target_text, "Project X");
    assert!(engine.check().expect("check fixture").is_ok());
}

#[test]
fn tags_are_canonical_sets_stored_outside_text() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let mut input = CreateNode::new("Point on Project X");
    input.tags = vec![" Meeting ".into(), "DÉCISION".into(), "meeting".into()];
    let node = engine.create_node(input).expect("create tagged node");

    assert_eq!(node.text, "Point on Project X");
    assert_eq!(node.tags, ["décision", "meeting"]);

    engine
        .set_tags(node.id, vec![" Project ".into(), "project".into()])
        .expect("replace tags");
    assert_eq!(
        engine.node(node.id).expect("read node").unwrap().tags,
        ["project"]
    );
    drop(engine);

    let engine = Engine::open(database.path()).expect("reopen database");
    assert_eq!(
        engine
            .node(node.id)
            .expect("read persisted node")
            .unwrap()
            .tags,
        ["project"]
    );
}

#[test]
fn invalid_tags_do_not_replace_existing_tags() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let node = create(&mut engine, "Node");
    engine
        .set_tags(node.id, vec!["valid".into()])
        .expect("set initial tag");

    for invalid in ["", "two words", "#meeting", "line\nbreak"] {
        assert!(matches!(
            engine.set_tags(node.id, vec![invalid.into()]),
            Err(Error::InvalidTag(_))
        ));
        assert_eq!(
            engine.node(node.id).expect("read node").unwrap().tags,
            ["valid"]
        );
    }
}

#[test]
fn references_resolve_current_target_text_without_rewriting_sources() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let mut target_input = CreateNode::new("Project X");
    target_input.tags = vec!["project".into()];
    let target = engine.create_node(target_input).expect("create target");

    let source_text = "Point on [[Project X]]";
    let mut source_input = CreateNode::new(source_text);
    source_input.tags = vec!["meeting".into()];
    source_input.references = vec![reference(source_text, "Project X", target.id)];
    let source = engine.create_node(source_input).expect("create source");
    assert_eq!(source.references.len(), 1);
    assert_eq!(source.references[0].target_text, "Project X");
    assert_eq!(source.tags, ["meeting"]);

    engine
        .set_text(target.id, "Project Y".into())
        .expect("rename target");
    let source_after_rename = engine.node(source.id).expect("read source").unwrap();
    assert_eq!(source_after_rename.text, source_text);
    assert_eq!(source_after_rename.references[0].target_text, "Project Y");

    assert!(matches!(
        engine.set_text(source.id, "would discard the reference".into()),
        Err(Error::NodeHasReferences(id)) if id == source.id
    ));
    assert_eq!(
        engine.node(source.id).unwrap().unwrap(),
        source_after_rename
    );
}

#[test]
fn content_replacement_is_atomic_and_preserves_tags() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let target = create(&mut engine, "Café");
    let source = create(&mut engine, "Draft");
    engine
        .set_tags(source.id, vec!["idea".into(), "meeting".into()])
        .expect("set tags");

    let text = "Voir [[Café]]";
    engine
        .set_content(
            source.id,
            text.into(),
            vec![reference(text, "Café", target.id)],
        )
        .expect("set referenced content");
    let updated = engine.node(source.id).expect("read source").unwrap();
    assert_eq!(updated.text, text);
    assert_eq!(updated.tags, ["idea", "meeting"]);
    assert_eq!(updated.references[0].target_text, "Café");

    let invalid = ReferenceInput {
        label_start: 1,
        label_end: 3,
        target_id: target.id,
    };
    assert!(matches!(
        engine.set_content(source.id, "Broken".into(), vec![invalid]),
        Err(Error::InvalidReferenceRange { .. })
    ));
    assert_eq!(engine.node(source.id).unwrap().unwrap(), updated);

    let missing = NodeId::from_bytes([42; 16]);
    let missing_text = "Missing [[target]]";
    assert!(matches!(
        engine.set_content(
            source.id,
            missing_text.into(),
            vec![reference(missing_text, "target", missing)]
        ),
        Err(Error::ReferenceTargetNotFound(id)) if id == missing
    ));
    assert_eq!(engine.node(source.id).unwrap().unwrap(), updated);
}

#[test]
fn overlapping_self_and_cyclic_references_follow_explicit_rules() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let first = create(&mut engine, "First");
    let second = create(&mut engine, "Second");

    let first_text = "Links [[Second]] and [[First]]";
    engine
        .set_content(
            first.id,
            first_text.into(),
            vec![
                reference(first_text, "Second", second.id),
                reference(first_text, "First", first.id),
            ],
        )
        .expect("create self reference");
    let second_text = "Back to [[First]]";
    engine
        .set_content(
            second.id,
            second_text.into(),
            vec![reference(second_text, "First", first.id)],
        )
        .expect("create reference cycle");
    assert_eq!(engine.node(first.id).unwrap().unwrap().references.len(), 2);
    assert_eq!(engine.node(second.id).unwrap().unwrap().references.len(), 1);

    let nested = "[[[[x]]]]";
    let outer = ReferenceInput {
        label_start: 2,
        label_end: 7,
        target_id: first.id,
    };
    let inner = ReferenceInput {
        label_start: 4,
        label_end: 5,
        target_id: second.id,
    };
    assert!(matches!(
        engine.set_content(first.id, nested.into(), vec![outer, inner]),
        Err(Error::OverlappingReferences)
    ));
}

#[test]
fn invalid_reference_creation_rolls_back_the_node() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let missing = NodeId::from_bytes([24; 16]);
    let text = "See [[missing]]";
    let mut input = CreateNode::new(text);
    input.tags = vec!["meeting".into()];
    input.references = vec![reference(text, "missing", missing)];

    assert!(matches!(
        engine.create_node(input),
        Err(Error::ReferenceTargetNotFound(id)) if id == missing
    ));
    assert_eq!(engine.check().expect("check rollback").node_count, 0);
}

#[test]
fn paths_are_root_first_persisted_and_cycle_safe() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let root = create(&mut engine, "Root");
    let mut child_input = CreateNode::new("Child");
    child_input.parent_id = Some(root.id);
    let child = engine.create_node(child_input).expect("create child");
    let mut leaf_input = CreateNode::new("Leaf");
    leaf_input.parent_id = Some(child.id);
    leaf_input.tags = vec!["decision".into()];
    let leaf = engine.create_node(leaf_input).expect("create leaf");

    assert_eq!(
        engine
            .path(leaf.id)
            .expect("read path")
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        [root.id, child.id, leaf.id]
    );
    assert_eq!(
        engine.path(root.id).expect("read root path"),
        std::slice::from_ref(&root)
    );
    assert!(matches!(
        engine.path(NodeId::from_bytes([99; 16])),
        Err(Error::NodeNotFound(_))
    ));
    drop(engine);

    let engine = Engine::open(database.path()).expect("reopen database");
    assert_eq!(engine.path(leaf.id).expect("read persisted path").len(), 3);
    drop(engine);

    let connection = Connection::open(database.path()).expect("open raw database");
    connection
        .execute(
            "UPDATE nodes SET parent_id = ?1 WHERE id = ?2",
            params![&leaf.id.as_bytes()[..], &root.id.as_bytes()[..]],
        )
        .expect("create external cycle");
    drop(connection);
    let engine = Engine::open(database.path()).expect("reopen corrupt database");
    assert!(matches!(
        engine.path(leaf.id),
        Err(Error::InvalidDatabase(_))
    ));
}

#[test]
fn paths_handle_substantial_depth_without_one_query_per_parent() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let mut parent = None;
    let mut expected = Vec::new();
    for index in 0..2_048 {
        let mut input = CreateNode::new(format!("Node {index}"));
        input.parent_id = parent;
        let node = engine.create_node(input).expect("create deep node");
        parent = Some(node.id);
        expected.push(node.id);
    }

    let path = engine.path(parent.unwrap()).expect("read deep path");
    assert_eq!(
        path.iter().map(|node| node.id).collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn paginated_children_include_batched_properties() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let target = create(&mut engine, "Target");
    for index in 0..5 {
        let text = format!("Child {index} [[Target]]");
        let mut input = CreateNode::new(&text);
        input.tags = vec![format!("tag-{index}")];
        input.references = vec![reference(&text, "Target", target.id)];
        engine.create_node(input).expect("create child");
    }

    let first = engine
        .children(
            None,
            Page {
                limit: 3,
                after: None,
            },
        )
        .expect("read first page");
    let second = engine
        .children(
            None,
            Page {
                limit: 3,
                after: first.next,
            },
        )
        .expect("read second page");
    let nodes: Vec<_> = first.nodes.into_iter().chain(second.nodes).collect();
    assert_eq!(nodes.len(), 6);
    for node in nodes.into_iter().skip(1) {
        assert_eq!(node.tags.len(), 1);
        assert_eq!(node.references[0].target_text, "Target");
    }
}

#[test]
fn check_reports_noncanonical_tags_and_invalid_reference_ranges() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let source = create(&mut engine, "Plain text");
    let target = create(&mut engine, "Target");
    drop(engine);

    let connection = Connection::open(database.path()).expect("open raw database");
    connection
        .execute(
            "INSERT INTO node_tags (node_id, tag) VALUES (?1, 'Meeting')",
            params![&source.id.as_bytes()[..]],
        )
        .expect("insert noncanonical tag");
    connection
        .execute(
            "INSERT INTO node_references (source_id, start_byte, end_byte, target_id)
             VALUES (?1, 1, 2, ?2)",
            params![&source.id.as_bytes()[..], &target.id.as_bytes()[..]],
        )
        .expect("insert invalid range");
    drop(connection);

    let engine = Engine::open(database.path()).expect("reopen database");
    let report = engine.check().expect("check content");
    assert!(report.issues.iter().any(
        |issue| matches!(issue, CheckIssue::NonCanonicalTag { node_id, .. } if *node_id == source.id)
    ));
    assert!(report.issues.iter().any(
        |issue| matches!(issue, CheckIssue::InvalidReference { source_id, .. } if *source_id == source.id)
    ));
}

#[test]
fn metadata_queries_use_their_dedicated_indexes() {
    let database = TestDatabase::new();
    Engine::open(database.path()).expect("create database");
    let connection = Connection::open(database.path()).expect("open raw database");
    let queries = [
        (
            "SELECT node_id, tag FROM node_tags WHERE node_id IN (?1)",
            "PRIMARY KEY",
        ),
        (
            "SELECT node_id FROM node_tags WHERE tag = ?1 ORDER BY node_id",
            "node_tags_by_tag",
        ),
        (
            "SELECT source_id, start_byte FROM node_references
             WHERE source_id IN (?1) ORDER BY source_id, start_byte",
            "PRIMARY KEY",
        ),
        (
            "SELECT source_id, start_byte FROM node_references
             WHERE target_id = ?1 ORDER BY source_id, start_byte",
            "node_references_by_target",
        ),
    ];

    for (query, expected_index) in queries {
        let sql = format!("EXPLAIN QUERY PLAN {query}");
        let mut statement = connection.prepare(&sql).expect("prepare query plan");
        let plan: Vec<String> = statement
            .query_map(params![&[0_u8; 16][..]], |row| row.get(3))
            .expect("read query plan")
            .collect::<rusqlite::Result<_>>()
            .expect("collect query plan");
        assert!(
            plan.iter().any(|step| step.contains(expected_index)),
            "query does not use {expected_index}: {plan:?}"
        );
    }
}
