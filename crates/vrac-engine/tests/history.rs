use tempfile::tempdir;
use vrac_engine::{
    CreateNode, Destination, Engine, Page, Placement, ReferenceInput, SyncApply, SyncDeviceId,
};

fn device(byte: u8) -> SyncDeviceId {
    SyncDeviceId::from_bytes([byte; 16])
}

fn flush(engine: &mut Engine) -> Vec<u8> {
    let package = engine
        .next_sync_package()
        .expect("prepare package")
        .expect("pending package");
    let bytes = package.bytes().to_vec();
    engine
        .confirm_sync_package(&package)
        .expect("confirm package");
    bytes
}

#[test]
fn undo_and_redo_restore_complete_product_mutations() {
    let mut engine = Engine::open(":memory:").expect("open workspace");
    assert!(!engine.undo().expect("empty undo"));
    assert!(!engine.redo().expect("empty redo"));

    let target = engine
        .create_node(CreateNode::new("Project X"))
        .expect("create target")
        .node;
    let source = engine
        .create_node(CreateNode::new("Draft"))
        .expect("create source")
        .node;
    let text = "Decision on [[Project X]]";
    engine
        .set_content(
            source.id,
            text.into(),
            vec![ReferenceInput {
                label_start: 14,
                label_end: 23,
                target_id: target.id,
            }],
        )
        .expect("set referenced content");
    engine
        .set_tags(source.id, vec!["Decision".into(), "Meeting".into()])
        .expect("set tags");
    engine
        .move_node(
            source.id,
            Destination {
                parent_id: Some(target.id),
                placement: Placement::Last,
            },
        )
        .expect("move source");
    engine.delete_node(source.id).expect("delete source");

    assert!(engine.undo().expect("undo delete"));
    let restored = engine.node(source.id).unwrap().expect("restored source");
    assert_eq!(restored.parent_id, Some(target.id));
    assert_eq!(restored.text, text);
    assert_eq!(restored.tags, ["decision", "meeting"]);
    assert_eq!(restored.references[0].target_id, target.id);

    assert!(engine.undo().expect("undo move"));
    assert_eq!(engine.node(source.id).unwrap().unwrap().parent_id, None);
    assert!(engine.undo().expect("undo tags"));
    assert!(engine.node(source.id).unwrap().unwrap().tags.is_empty());
    assert!(engine.undo().expect("undo content"));
    let draft = engine.node(source.id).unwrap().unwrap();
    assert_eq!(draft.text, "Draft");
    assert!(draft.references.is_empty());

    for _ in 0..4 {
        assert!(engine.redo().expect("redo mutation"));
    }
    assert!(engine.node(source.id).unwrap().is_none());
    assert!(engine.check().unwrap().is_ok());
}

#[test]
fn history_is_bounded_session_local_and_new_writes_clear_redo() {
    let directory = tempdir().expect("create temporary directory");
    let path = directory.path().join("workspace.vrac");
    let mut engine = Engine::open(&path).expect("open workspace");
    let node = engine
        .create_node(CreateNode::new("0"))
        .expect("create node")
        .node;
    for value in 1..=101 {
        engine
            .set_text(node.id, value.to_string())
            .expect("edit node");
    }
    for _ in 0..100 {
        assert!(engine.undo().expect("undo retained edit"));
    }
    assert_eq!(engine.node(node.id).unwrap().unwrap().text, "1");
    assert!(!engine.undo().expect("bounded undo"));

    assert!(engine.redo().expect("redo one edit"));
    engine
        .set_text(node.id, "replacement".into())
        .expect("new edit");
    assert!(!engine.redo().expect("redo cleared"));
    drop(engine);

    let mut reopened = Engine::open(&path).expect("reopen workspace");
    assert!(!reopened.undo().expect("session history not persisted"));
    assert_eq!(reopened.node(node.id).unwrap().unwrap().text, "replacement");
}

#[test]
fn creating_a_journal_day_preserves_undo_but_clears_redo() {
    let mut engine = Engine::open(":memory:").expect("open workspace");
    let node = engine
        .create_node(CreateNode::new("Original"))
        .expect("create node")
        .node;
    engine
        .set_text(node.id, "Edited".into())
        .expect("edit node");
    assert!(engine.undo().expect("undo edit"));

    let day = engine
        .journal_day("2026-07-21")
        .expect("create journal day");
    assert!(!engine.redo().expect("system mutation clears redo"));
    assert!(
        engine
            .undo()
            .expect("earlier user mutation remains undoable")
    );
    assert!(engine.node(node.id).unwrap().is_none());
    assert!(engine.node(day.id).unwrap().is_some());
}

#[test]
fn no_op_edits_preserve_redo_and_oversized_edits_drop_history() {
    let mut engine = Engine::open(":memory:").expect("open workspace");
    let node = engine
        .create_node(CreateNode::new("Original"))
        .expect("create node")
        .node;
    engine
        .set_text(node.id, "Edited".into())
        .expect("edit node");
    assert!(engine.undo().expect("undo edit"));
    engine
        .set_text(node.id, "Original".into())
        .expect("repeat current value");
    engine
        .set_content(node.id, "Original".into(), Vec::new())
        .expect("repeat current content");
    assert!(engine.redo().expect("no-op preserved redo"));

    engine
        .set_text(node.id, "x".repeat(8 * 1024 * 1024 + 1))
        .expect("store oversized edit");
    assert!(!engine.undo().expect("oversized edit is not retained"));
}

#[test]
fn history_mutations_synchronize_and_remote_imports_clear_local_history() {
    let directory = tempdir().expect("create temporary directory");
    let first_path = directory.path().join("first.vrac");
    let second_path = directory.path().join("second.vrac");
    let mut first = Engine::open_synced(&first_path, device(1)).expect("open first device");
    let node = first
        .create_node(CreateNode::new("Original"))
        .expect("create node")
        .node;
    flush(&mut first);
    first
        .checkpoint(&second_path)
        .expect("bootstrap second device");
    let mut second = Engine::open_synced(&second_path, device(2)).expect("open second device");

    first.set_text(node.id, "Edited".into()).expect("edit node");
    assert!(first.undo().expect("undo edit"));
    assert!(first.redo().expect("redo edit"));
    let package = flush(&mut first);
    assert_eq!(
        second.apply_sync_package(&package).expect("apply history"),
        SyncApply::Applied
    );
    assert_eq!(second.node(node.id).unwrap().unwrap().text, "Edited");

    second
        .create_node(CreateNode::new("Remote"))
        .expect("create remote node");
    let package = flush(&mut second);
    assert_eq!(
        first
            .apply_sync_package(&package)
            .expect("apply remote change"),
        SyncApply::Applied
    );
    assert!(!first.undo().expect("remote import cleared history"));
    assert_eq!(
        first.children(None, Page::default()).unwrap().nodes.len(),
        3
    );
}

#[test]
fn deleting_referenced_content_can_be_redone_and_synchronized() {
    let directory = tempdir().expect("create temporary directory");
    let first_path = directory.path().join("first.vrac");
    let second_path = directory.path().join("second.vrac");
    let mut first = Engine::open_synced(&first_path, device(1)).expect("open first device");
    let target = first
        .create_node(CreateNode::new("Target"))
        .expect("create target")
        .node;
    let text = "See [[Target]]";
    let mut source = CreateNode::new(text);
    source.references.push(ReferenceInput {
        label_start: 6,
        label_end: 12,
        target_id: target.id,
    });
    let source = first.create_node(source).expect("create source").node;
    flush(&mut first);
    first
        .checkpoint(&second_path)
        .expect("bootstrap second device");
    let mut second = Engine::open_synced(&second_path, device(2)).expect("open second device");

    let deleted = first.delete_node(source.id).expect("delete source");
    assert_eq!(deleted.pruned_roots, [target.id]);
    assert!(first.node(target.id).unwrap().is_none());
    assert!(first.undo().expect("undo deletion"));
    assert!(first.node(target.id).unwrap().is_some());
    assert!(first.redo().expect("redo deletion"));
    let package = flush(&mut first);
    assert_eq!(
        second.apply_sync_package(&package).expect("apply deletion"),
        SyncApply::Applied
    );
    assert!(second.node(source.id).unwrap().is_none());
    assert!(second.node(target.id).unwrap().is_none());
}

#[test]
fn pruning_a_detached_root_can_be_undone_redone_and_synchronized() {
    let directory = tempdir().expect("create temporary directory");
    let first_path = directory.path().join("first.vrac");
    let second_path = directory.path().join("second.vrac");
    let mut first = Engine::open_synced(&first_path, device(1)).expect("open first device");
    let target = first
        .create_node(CreateNode::new("Target"))
        .expect("create target")
        .node;
    let text = "See [[Target]]";
    let mut source = CreateNode::new(text);
    source.references.push(ReferenceInput {
        label_start: 6,
        label_end: 12,
        target_id: target.id,
    });
    let source = first.create_node(source).expect("create source").node;
    flush(&mut first);
    first
        .checkpoint(&second_path)
        .expect("bootstrap second device");
    let mut second = Engine::open_synced(&second_path, device(2)).expect("open second device");

    assert_eq!(
        first
            .set_content(source.id, "Detached".into(), Vec::new())
            .expect("detach target"),
        vrac_engine::ContentUpdate {
            references: Vec::new(),
            materialized_nodes: Vec::new(),
            pruned_roots: vec![target.id],
        }
    );
    assert!(first.node(target.id).unwrap().is_none());
    assert!(first.undo().expect("undo prune"));
    assert_eq!(
        first.node(source.id).unwrap().unwrap().references[0].target_id,
        target.id
    );
    assert!(first.node(target.id).unwrap().is_some());
    assert!(first.redo().expect("redo prune"));
    assert!(first.node(target.id).unwrap().is_none());

    let package = flush(&mut first);
    assert_eq!(
        second.apply_sync_package(&package).expect("apply prune"),
        SyncApply::Applied
    );
    assert!(second.node(target.id).unwrap().is_none());
    let synchronized = second.node(source.id).unwrap().unwrap();
    assert_eq!(synchronized.text, "Detached");
    assert!(synchronized.references.is_empty());
}
