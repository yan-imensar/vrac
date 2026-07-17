use tempfile::tempdir;
use vrac::{
    CreateNode, Destination, Engine, Error, Placement, ReferenceInput, SyncApply, SyncDeviceId,
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
fn two_devices_exchange_complete_idempotent_product_changes() {
    let directory = tempdir().expect("create temporary directory");
    let first_path = directory.path().join("first.vrac");
    let second_path = directory.path().join("second.vrac");
    let mut first = Engine::open_synced(&first_path, device(1)).expect("open first device");
    let project = first
        .create_node(CreateNode::new("Project X"))
        .expect("create project");
    flush(&mut first);
    first
        .checkpoint(&second_path)
        .expect("bootstrap second device");
    let mut second = Engine::open_synced(&second_path, device(2)).expect("open second device");

    first
        .set_text(project.id, "Project Y".into())
        .expect("rename target");
    let text = "Decision on [[Project Y]]";
    let mut decision = CreateNode::new(text);
    decision.parent_id = Some(project.id);
    decision.tags = vec!["Meeting".into(), "decision".into()];
    decision.references = vec![ReferenceInput {
        label_start: 14,
        label_end: 23,
        target_id: project.id,
    }];
    let decision = first.create_node(decision).expect("create decision");

    let package = first
        .next_sync_package()
        .expect("prepare package")
        .expect("pending package");
    assert_eq!(
        first.next_sync_package().expect("retry package"),
        Some(package.clone())
    );
    assert_eq!(
        second
            .apply_sync_package(package.bytes())
            .expect("apply package"),
        SyncApply::Applied
    );
    assert_eq!(
        second
            .apply_sync_package(package.bytes())
            .expect("repeat package"),
        SyncApply::AlreadyApplied
    );
    let synchronized = second.node(decision.id).unwrap().unwrap();
    assert_eq!(synchronized.tags, ["decision", "meeting"]);
    assert_eq!(synchronized.references[0].target_text, "Project Y");
    first
        .confirm_sync_package(&package)
        .expect("confirm package");
    assert!(first.next_sync_package().unwrap().is_none());
}

#[test]
fn packages_are_ordered_validated_and_workspace_scoped() {
    let directory = tempdir().expect("create temporary directory");
    let first_path = directory.path().join("first.vrac");
    let second_path = directory.path().join("second.vrac");
    let other_path = directory.path().join("other.vrac");
    let mut first = Engine::open_synced(&first_path, device(1)).unwrap();
    let node = first.create_node(CreateNode::new("Zero")).unwrap();
    flush(&mut first);
    first.checkpoint(&second_path).unwrap();
    let mut second = Engine::open_synced(&second_path, device(2)).unwrap();

    first.set_text(node.id, "One".into()).unwrap();
    let one = flush(&mut first);
    first.set_text(node.id, "Two".into()).unwrap();
    let two = flush(&mut first);
    assert!(matches!(
        second.apply_sync_package(&two),
        Err(Error::SyncPackageOutOfOrder { .. })
    ));
    assert_eq!(second.apply_sync_package(&one).unwrap(), SyncApply::Applied);
    assert_eq!(second.apply_sync_package(&two).unwrap(), SyncApply::Applied);
    assert_eq!(second.node(node.id).unwrap().unwrap().text, "Two");

    let mut corrupted = one.clone();
    corrupted[20] ^= 1;
    assert!(matches!(
        second.apply_sync_package(&corrupted),
        Err(Error::InvalidSyncPackage(_))
    ));
    let mut other = Engine::open_synced(&other_path, device(3)).unwrap();
    assert!(matches!(
        other.apply_sync_package(&one),
        Err(Error::SyncWorkspaceMismatch)
    ));
}

#[test]
fn independent_edits_merge_while_row_conflicts_roll_back() {
    let directory = tempdir().expect("create temporary directory");
    let first_path = directory.path().join("first.vrac");
    let second_path = directory.path().join("second.vrac");
    let mut first = Engine::open_synced(&first_path, device(1)).unwrap();
    let left = first.create_node(CreateNode::new("Left")).unwrap();
    let right = first.create_node(CreateNode::new("Right")).unwrap();
    flush(&mut first);
    first.checkpoint(&second_path).unwrap();
    let mut second = Engine::open_synced(&second_path, device(2)).unwrap();

    first.set_text(left.id, "Left A".into()).unwrap();
    second.set_text(right.id, "Right B".into()).unwrap();
    let from_first = flush(&mut first);
    let from_second = flush(&mut second);
    assert_eq!(
        first.apply_sync_package(&from_second).unwrap(),
        SyncApply::Applied
    );
    assert_eq!(
        second.apply_sync_package(&from_first).unwrap(),
        SyncApply::Applied
    );

    first.set_text(left.id, "Conflict A".into()).unwrap();
    second.set_text(left.id, "Conflict B".into()).unwrap();
    let conflict = flush(&mut first);
    assert!(matches!(
        second.apply_sync_package(&conflict),
        Err(Error::SyncConflict { .. })
    ));
    assert_eq!(second.node(left.id).unwrap().unwrap().text, "Conflict B");
}

#[test]
fn a_cycle_created_only_by_merging_two_valid_moves_is_rejected() {
    let directory = tempdir().expect("create temporary directory");
    let first_path = directory.path().join("first.vrac");
    let second_path = directory.path().join("second.vrac");
    let mut first = Engine::open_synced(&first_path, device(1)).unwrap();
    let left = first.create_node(CreateNode::new("Left")).unwrap();
    let right = first.create_node(CreateNode::new("Right")).unwrap();
    flush(&mut first);
    first.checkpoint(&second_path).unwrap();
    let mut second = Engine::open_synced(&second_path, device(2)).unwrap();

    first
        .move_node(
            left.id,
            Destination {
                parent_id: Some(right.id),
                placement: Placement::Last,
            },
        )
        .unwrap();
    second
        .move_node(
            right.id,
            Destination {
                parent_id: Some(left.id),
                placement: Placement::Last,
            },
        )
        .unwrap();
    let cycle = flush(&mut first);
    assert!(matches!(
        second.apply_sync_package(&cycle),
        Err(Error::SyncConflict { .. })
    ));
    assert!(second.check().unwrap().is_ok());
}

#[test]
fn a_checkpoint_covers_pending_changes_without_losing_the_source_boundary() {
    let directory = tempdir().expect("create temporary directory");
    let first_path = directory.path().join("first.vrac");
    let second_path = directory.path().join("second.vrac");
    let mut first = Engine::open_synced(&first_path, device(1)).unwrap();
    let node = first
        .create_node(CreateNode::new("In checkpoint 0"))
        .unwrap();
    for index in 1..=300 {
        first
            .set_text(node.id, format!("In checkpoint {index}"))
            .unwrap();
    }
    first.checkpoint(&second_path).unwrap();
    let mut second = Engine::open_synced(&second_path, device(2)).unwrap();
    assert_eq!(
        second.node(node.id).unwrap().unwrap().text,
        "In checkpoint 300"
    );
    assert!(second.next_sync_package().unwrap().is_none());

    first.set_text(node.id, "After checkpoint".into()).unwrap();
    let covered = first.next_sync_package().unwrap().unwrap();
    assert_eq!(
        second.apply_sync_package(covered.bytes()).unwrap(),
        SyncApply::AlreadyApplied
    );
    first.confirm_sync_package(&covered).unwrap();
    let later = first.next_sync_package().unwrap().unwrap();
    assert_eq!(
        second.apply_sync_package(later.bytes()).unwrap(),
        SyncApply::Applied
    );
    assert_eq!(
        second.node(node.id).unwrap().unwrap().text,
        "After checkpoint"
    );
}

#[test]
fn restoring_a_device_identity_continues_after_the_checkpoint_frontier() {
    let directory = tempdir().expect("create temporary directory");
    let source_path = directory.path().join("source.vrac");
    let receiver_path = directory.path().join("receiver.vrac");
    let restored_path = directory.path().join("restored.vrac");
    let mut source = Engine::open_synced(&source_path, device(1)).unwrap();
    let node = source.create_node(CreateNode::new("Checkpoint")).unwrap();
    source.checkpoint(&receiver_path).unwrap();
    std::fs::copy(&receiver_path, &restored_path).unwrap();

    let mut receiver = Engine::open_synced(&receiver_path, device(2)).unwrap();
    let mut restored = Engine::open_synced(&restored_path, device(1)).unwrap();
    restored.set_text(node.id, "Restored edit".into()).unwrap();
    let package = flush(&mut restored);
    assert_eq!(
        receiver.apply_sync_package(&package).unwrap(),
        SyncApply::Applied
    );
    assert_eq!(
        receiver.node(node.id).unwrap().unwrap().text,
        "Restored edit"
    );
}
