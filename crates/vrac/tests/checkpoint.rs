use rusqlite::{Connection, params};
use tempfile::tempdir;
use vrac::{CheckIssue, CreateNode, Engine, Error, ReferenceInput};

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
    assert_eq!(report.node_count, 2);
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
