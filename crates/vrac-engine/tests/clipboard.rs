use vrac_engine::{CreateNode, Destination, Engine, Page, Placement};

fn create(
    engine: &mut Engine,
    parent_id: Option<vrac_engine::NodeId>,
    text: &str,
    tags: &[&str],
) -> vrac_engine::Node {
    let mut input = CreateNode::new(text);
    input.parent_id = parent_id;
    input.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
    engine.create_node(input).expect("create node")
}

#[test]
fn copied_subtrees_are_portable_indented_bullets_without_overlap() {
    let mut engine = Engine::open(":memory:").expect("open engine");
    let root = create(&mut engine, None, "Project", &["active", "work"]);
    let first = create(&mut engine, Some(root.id), "First", &[]);
    create(&mut engine, Some(first.id), "Nested", &["task"]);
    create(&mut engine, Some(root.id), "Second", &[]);

    let text = engine
        .copy_nodes(&[root.id, first.id])
        .expect("copy subtree");

    assert_eq!(
        text,
        "- Project #active #work\n  - First\n    - Nested #task\n  - Second"
    );
}

#[test]
fn pasted_outline_is_one_atomic_hierarchical_mutation() {
    let mut engine = Engine::open(":memory:").expect("open engine");
    let existing = create(&mut engine, None, "Existing", &[]);

    let roots = engine
        .paste_nodes(
            Destination {
                parent_id: None,
                placement: Placement::After(existing.id),
            },
            "- Project #active\n  - First\n  - Second #task\n- Notes",
        )
        .expect("paste outline");

    assert_eq!(
        roots
            .iter()
            .map(|node| node.text.as_str())
            .collect::<Vec<_>>(),
        ["Project", "Notes"]
    );
    assert_eq!(roots[0].tags, ["active"]);
    assert!(roots[0].has_children);
    let children = engine
        .children(Some(roots[0].id), Page::default())
        .expect("read pasted children");
    assert_eq!(
        children
            .nodes
            .iter()
            .map(|node| node.text.as_str())
            .collect::<Vec<_>>(),
        ["First", "Second"]
    );
    assert!(children.nodes[0].tags.is_empty());
    assert_eq!(children.nodes[1].tags, ["task"]);

    assert!(engine.undo().expect("undo paste"));
    assert!(engine.node(roots[0].id).expect("read root").is_none());
    assert!(engine.node(roots[1].id).expect("read root").is_none());
    assert!(engine.node(existing.id).expect("read existing").is_some());
    assert!(engine.redo().expect("redo paste"));
    assert_eq!(
        engine
            .children(Some(roots[0].id), Page::default())
            .expect("read redone children")
            .nodes
            .len(),
        2
    );
}

#[test]
fn pasted_reference_syntax_materializes_and_reuses_concepts() {
    let mut engine = Engine::open(":memory:").expect("open engine");
    let existing = create(&mut engine, None, "Existing", &[]);

    let roots = engine
        .paste_nodes(
            Destination {
                parent_id: None,
                placement: Placement::Last,
            },
            "- See [[Existing]] and [[New concept]]\n  - Again [[New concept]]",
        )
        .expect("paste references");

    assert_eq!(roots[0].references.len(), 2);
    assert_eq!(roots[0].references[0].target_id, existing.id);
    let concept_id = roots[0].references[1].target_id;
    assert_eq!(
        engine.node(concept_id).unwrap().unwrap().text,
        "New concept"
    );
    let child = engine
        .children(Some(roots[0].id), Page::default())
        .unwrap()
        .nodes
        .remove(0);
    assert_eq!(child.references[0].target_id, concept_id);

    assert!(engine.undo().expect("undo paste"));
    assert!(engine.node(concept_id).unwrap().is_none());
}

#[test]
fn malformed_indentation_creates_nothing() {
    let mut engine = Engine::open(":memory:").expect("open engine");
    let before = engine.children(None, Page::default()).unwrap().nodes.len();

    assert!(
        engine
            .paste_nodes(
                Destination {
                    parent_id: None,
                    placement: Placement::Last,
                },
                "- Root\n    - Child\n  - Invalid dedent",
            )
            .is_err()
    );

    assert_eq!(
        engine.children(None, Page::default()).unwrap().nodes.len(),
        before
    );
}

#[test]
fn clipboard_round_trip_preserves_trailing_spaces_before_tags() {
    let mut engine = Engine::open(":memory:").expect("open engine");
    let source = create(&mut engine, None, "Keep this space ", &["tag"]);
    let text = engine.copy_nodes(&[source.id]).expect("copy node");
    let pasted = engine
        .paste_nodes(
            Destination {
                parent_id: None,
                placement: Placement::After(source.id),
            },
            &text,
        )
        .expect("paste node");

    assert_eq!(pasted[0].text, source.text);
    assert_eq!(pasted[0].tags, source.tags);

    let plain = create(&mut engine, None, "Also keep this space ", &[]);
    assert_eq!(
        engine.copy_nodes(&[plain.id]).expect("copy plain node"),
        "- Also keep this space "
    );
}
