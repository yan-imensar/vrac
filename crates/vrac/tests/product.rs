use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use tempfile::TempDir;
use vrac::{
    CheckIssue, CreateNode, Destination, Engine, Error, Node, NodeId, Page, Placement,
    ReferenceInput, SystemNode,
};

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

fn create_child(engine: &mut Engine, parent_id: NodeId, text: &str) -> Node {
    let mut input = CreateNode::new(text);
    input.parent_id = Some(parent_id);
    engine.create_node(input).expect("create child")
}

fn create_referencing_child(
    engine: &mut Engine,
    parent_id: NodeId,
    text: &str,
    label: &str,
    target_id: NodeId,
) -> Node {
    let mut input = CreateNode::new(text);
    input.parent_id = Some(parent_id);
    input.references = vec![reference(text, label, target_id)];
    engine.create_node(input).expect("create reference")
}

#[test]
fn journal_days_are_visible_tagged_protected_and_referenceable() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let roots = engine.children(None, Page::default()).expect("read roots");
    let journal = roots
        .nodes
        .into_iter()
        .find(|node| node.system == Some(SystemNode::Journal))
        .expect("journal exists");
    assert_eq!(journal.text, "Journal");

    let day = engine.journal_day("2026-07-19").expect("create day");
    assert_eq!(
        day.system,
        Some(SystemNode::JournalDay {
            date: "2026-07-19".into()
        })
    );
    assert_eq!(day.parent_id, Some(journal.id));
    assert_eq!(day.tags, ["journal"]);
    assert_eq!(engine.journal_day("2026-07-19").unwrap().id, day.id);

    for result in [
        engine.set_text(day.id, "Tomorrow".into()),
        engine
            .set_content(day.id, "Tomorrow".into(), Vec::new())
            .map(drop),
        engine.move_node(
            day.id,
            Destination {
                parent_id: None,
                placement: Placement::Last,
            },
        ),
        engine.delete_node(day.id).map(drop),
    ] {
        assert!(matches!(result, Err(Error::SystemNodeProtected(id)) if id == day.id));
    }
    assert!(matches!(
        engine.set_tags(day.id, vec!["plan".into()]),
        Err(Error::SystemNodeProtected(id)) if id == day.id
    ));
    engine
        .set_tags(day.id, vec!["journal".into(), "plan".into()])
        .expect("keep required tag");

    let task_text = "Prepare release for [[2026-07-19]]";
    let mut task = CreateNode::new(task_text);
    task.references = vec![reference(task_text, "2026-07-19", day.id)];
    let task = engine.create_node(task).expect("create planned task");
    assert_eq!(task.references[0].target_id, day.id);
    assert_eq!(task.references[0].target_text, "2026-07-19");

    assert!(matches!(
        engine.create_node(CreateNode {
            parent_id: Some(journal.id),
            ..CreateNode::new("Not a date")
        }),
        Err(Error::SystemNodeProtected(id)) if id == journal.id
    ));
    assert!(matches!(
        engine.delete_node(journal.id),
        Err(Error::SystemNodeProtected(id)) if id == journal.id
    ));
    assert!(engine.check().unwrap().is_ok());
}

#[test]
fn backlinks_inherit_reference_context_downward_and_filter_by_tag() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let northstar = create(&mut engine, "Northstar");
    let alex = create(&mut engine, "Alex");
    let morgan = create(&mut engine, "Morgan");
    let archive = create(&mut engine, "Archive");
    let day = engine.journal_day("2026-07-20").expect("create day");

    let meeting = create_referencing_child(
        &mut engine,
        day.id,
        "project meeting [[Northstar]]",
        "Northstar",
        northstar.id,
    );
    let attendee = create_referencing_child(
        &mut engine,
        meeting.id,
        "with [[Alex]]",
        "Alex",
        alex.id,
    );
    let _other_attendee = create_referencing_child(
        &mut engine,
        meeting.id,
        "with [[Morgan]]",
        "Morgan",
        morgan.id,
    );
    let decision = create_child(&mut engine, meeting.id, "approve the release plan");
    engine
        .set_tags(decision.id, vec!["decision".into()])
        .expect("tag decision");

    let work = create_referencing_child(
        &mut engine,
        day.id,
        "work on the next version of [[Archive]]",
        "Archive",
        archive.id,
    );
    let ux = create_child(&mut engine, work.id, "UX / UI");
    let review = create_child(&mut engine, ux.id, "add an archive review screen");
    engine
        .set_tags(review.id, vec!["task".into()])
        .expect("tag review task");
    let engine_work = create_child(&mut engine, work.id, "Engine");
    let sqlite = create_child(&mut engine, engine_work.id, "add a SQLite snapshot");
    engine
        .set_tags(sqlite.id, vec!["task".into()])
        .expect("tag sqlite task");
    let contact = create_referencing_child(
        &mut engine,
        engine_work.id,
        "ask [[Morgan]] for a code review",
        "Morgan",
        morgan.id,
    );
    engine
        .set_tags(contact.id, vec!["task".into()])
        .expect("tag contact task");

    let northstar_links = engine
        .backlinks(northstar.id, None, Page::default())
        .expect("read northstar backlinks");
    assert_eq!(northstar_links.contexts.len(), 1);
    assert_eq!(
        northstar_links.contexts[0].path.last().unwrap().id,
        meeting.id
    );
    assert!(
        northstar_links.contexts[0]
            .path
            .iter()
            .any(|node| node.id == day.id)
    );

    let decisions = engine
        .backlinks(northstar.id, Some(" Decision "), Page::default())
        .expect("filter northstar decisions");
    assert_eq!(decisions.contexts.len(), 1);
    assert_eq!(decisions.contexts[0].path.last().unwrap().id, decision.id);
    assert!(
        decisions.contexts[0]
            .path
            .iter()
            .any(|node| node.id == meeting.id)
    );

    let alex_links = engine
        .backlinks(alex.id, None, Page::default())
        .expect("read alex backlinks");
    assert_eq!(
        alex_links.contexts[0].path.last().unwrap().id,
        attendee.id
    );
    assert!(
        alex_links.contexts[0]
            .path
            .iter()
            .any(|node| node.id == meeting.id)
    );

    let archive_tasks = engine
        .backlinks(archive.id, Some("task"), Page::default())
        .expect("read archive tasks");
    let archive_task_ids = archive_tasks
        .contexts
        .iter()
        .map(|context| context.path.last().unwrap().id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(archive_task_ids, [review.id, sqlite.id, contact.id].into());
    assert_eq!(
        engine
            .backlink_tags(archive.id, 20)
            .expect("read archive tag facets"),
        [vrac::BacklinkTag {
            tag: "task".into(),
            count: 3,
        }]
    );

    let morgan_tasks = engine
        .backlinks(morgan.id, Some("task"), Page::default())
        .expect("read morgan tasks");
    assert_eq!(morgan_tasks.contexts.len(), 1);
    assert_eq!(
        morgan_tasks.contexts[0].path.last().unwrap().id,
        contact.id
    );
    assert!(
        !morgan_tasks.contexts[0]
            .path
            .iter()
            .any(|node| node.id == sqlite.id)
    );
    assert_eq!(
        engine
            .backlink_tags(morgan.id, 20)
            .expect("read morgan tag facets"),
        [vrac::BacklinkTag {
            tag: "task".into(),
            count: 1,
        }]
    );

    let unrelated = create(&mut engine, "Unrelated");
    engine
        .set_tags(unrelated.id, vec!["elsewhere".into()])
        .expect("tag unrelated node");
    assert!(
        engine
            .backlink_tags(archive.id, 20)
            .unwrap()
            .iter()
            .all(|facet| facet.tag != "elsewhere")
    );
}

#[test]
fn backlinks_are_cursor_paginated_without_duplicates_or_omissions() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let target = create(&mut engine, "Project");
    let mut expected = std::collections::HashSet::new();
    for date in ["2026-07-18", "2026-07-19", "2026-07-20"] {
        let day = engine.journal_day(date).expect("create day");
        let source = create_referencing_child(
            &mut engine,
            day.id,
            "work on [[Project]]",
            "Project",
            target.id,
        );
        expected.insert(source.id);
    }

    let first = engine
        .backlinks(
            target.id,
            None,
            Page {
                limit: 2,
                after: None,
            },
        )
        .expect("read first page");
    assert_eq!(first.contexts.len(), 2);
    assert_eq!(first.contexts[0].path[1].text, "2026-07-20");
    let second = engine
        .backlinks(
            target.id,
            None,
            Page {
                limit: 2,
                after: first.next,
            },
        )
        .expect("read second page");
    assert_eq!(second.contexts.len(), 1);
    assert!(second.next.is_none());

    let actual = first
        .contexts
        .iter()
        .chain(&second.contexts)
        .map(|context| context.path.last().unwrap().id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn invalid_journal_dates_never_create_partial_nodes() {
    let mut engine = Engine::open(":memory:").expect("open database");
    for date in ["2026-2-03", "2026-02-29", "2024-02-30", "2026-13-01"] {
        assert!(matches!(
            engine.journal_day(date),
            Err(Error::InvalidJournalDate(value)) if value == date
        ));
    }
    assert_eq!(engine.check().unwrap().node_count, 1);
}

#[test]
fn check_detects_a_broken_journal_day() {
    let database = TestDatabase::new();
    let mut engine = Engine::open(database.path()).expect("open database");
    let day = engine.journal_day("2026-07-19").expect("create day");
    drop(engine);

    let connection = Connection::open(database.path()).expect("open raw database");
    connection
        .execute(
            "DELETE FROM node_tags WHERE node_id = ?1 AND tag = 'journal'",
            params![&day.id.as_bytes()[..]],
        )
        .expect("corrupt system tag");
    drop(connection);

    let engine = Engine::open(database.path()).expect("reopen database");
    assert!(
        engine
            .check()
            .unwrap()
            .issues
            .contains(&CheckIssue::InvalidSystemNode { node_id: day.id })
    );
}

#[test]
fn search_is_bounded_prefix_based_and_tracks_text_changes() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let project = create(&mut engine, "Projet Éléphant");
    let other = create(&mut engine, "Unrelated note");

    assert_eq!(engine.search("pro élé", 8).unwrap()[0].id, project.id);
    assert!(engine.search("[[]]", 8).unwrap().is_empty());
    assert!(engine.search("p", 8).unwrap().is_empty());
    assert!(matches!(
        engine.search("project", 0),
        Err(Error::InvalidPageLimit { .. })
    ));

    engine
        .set_text(project.id, "Archived topic".into())
        .expect("rename indexed node");
    assert!(engine.search("projet", 8).unwrap().is_empty());
    assert_eq!(engine.search("arch", 8).unwrap()[0].id, project.id);

    engine.delete_node(other.id).expect("delete indexed node");
    assert!(engine.search("unrelated", 8).unwrap().is_empty());
}

#[test]
fn deleting_a_subtree_is_atomic_and_preserves_external_references() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let root = create(&mut engine, "Project");
    let mut child_input = CreateNode::new("Decision");
    child_input.parent_id = Some(root.id);
    let child = engine.create_node(child_input).expect("create child");
    let source_text = "See [[Decision]]";
    let mut source_input = CreateNode::new(source_text);
    source_input.references = vec![reference(source_text, "Decision", child.id)];
    let source = engine
        .create_node(source_input)
        .expect("create external reference");

    assert!(matches!(
        engine.delete_node(root.id),
        Err(Error::NodeReferenced(id)) if id == child.id
    ));
    assert!(engine.node(root.id).unwrap().is_some());
    assert!(engine.node(child.id).unwrap().is_some());

    engine
        .set_content(source.id, "Reference removed".into(), Vec::new())
        .expect("remove external reference");
    assert_eq!(
        engine
            .delete_node(root.id)
            .expect("delete subtree")
            .deleted_nodes,
        2
    );
    assert!(engine.node(root.id).unwrap().is_none());
    assert!(engine.node(child.id).unwrap().is_none());
    assert!(engine.node(source.id).unwrap().is_some());
}

#[test]
fn references_inside_a_deleted_subtree_do_not_block_deletion() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let root = create(&mut engine, "Project");
    let text = "Self [[Project]]";
    engine
        .set_content(
            root.id,
            text.into(),
            vec![reference(text, "Project", root.id)],
        )
        .expect("create internal reference");

    assert_eq!(
        engine
            .delete_node(root.id)
            .expect("delete subtree")
            .deleted_nodes,
        1
    );
    assert!(engine.check().unwrap().is_ok());
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
fn tag_completion_is_canonical_bounded_and_globally_deduplicated() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let first = create(&mut engine, "First");
    let second = create(&mut engine, "Second");
    engine
        .set_tags(
            first.id,
            vec!["Meeting".into(), "DÉCISION".into(), "idea".into()],
        )
        .expect("set first tags");
    engine
        .set_tags(second.id, vec!["meeting".into(), "decision".into()])
        .expect("set second tags");

    assert_eq!(
        engine.tags("", 8).expect("list tags"),
        ["decision", "décision", "idea", "meeting"]
    );
    assert_eq!(engine.tags(" ME ", 8).unwrap(), ["meeting"]);
    assert_eq!(engine.tags("DÉ", 8).unwrap(), ["décision"]);
    assert_eq!(engine.tags("d", 1).unwrap(), ["decision"]);
    assert!(matches!(
        engine.tags("two words", 8),
        Err(Error::InvalidTag(_))
    ));
    assert!(matches!(
        engine.tags("", 0),
        Err(Error::InvalidPageLimit { .. })
    ));
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
fn removing_references_prunes_only_empty_unreferenced_roots() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let empty = create(&mut engine, "Empty");

    let shared = create(&mut engine, "Shared");
    let shared_text = "Also [[Shared]]";
    let mut shared_source = CreateNode::new(shared_text);
    shared_source.references = vec![reference(shared_text, "Shared", shared.id)];
    engine
        .create_node(shared_source)
        .expect("create second incoming reference");

    let mut tagged_input = CreateNode::new("Tagged");
    tagged_input.tags = vec!["project".into()];
    let tagged = engine
        .create_node(tagged_input)
        .expect("create tagged target");

    let parent = create(&mut engine, "Parent");
    let nested = create_child(&mut engine, parent.id, "Nested");

    let with_child = create(&mut engine, "With child");
    create_child(&mut engine, with_child.id, "Content");

    let outgoing_text = "See [[Tagged]]";
    let mut outgoing_input = CreateNode::new(outgoing_text);
    outgoing_input.references = vec![reference(outgoing_text, "Tagged", tagged.id)];
    let outgoing = engine
        .create_node(outgoing_input)
        .expect("create target with outgoing content");

    let text = "[[Empty]] [[Shared]] [[Tagged]] [[Nested]] [[With child]] [[See Tagged]]";
    let mut source_input = CreateNode::new(text);
    source_input.references = vec![
        reference(text, "Empty", empty.id),
        reference(text, "Shared", shared.id),
        reference(text, "Tagged", tagged.id),
        reference(text, "Nested", nested.id),
        reference(text, "With child", with_child.id),
        reference(text, "See Tagged", outgoing.id),
    ];
    let source = engine.create_node(source_input).expect("create references");

    assert_eq!(
        engine
            .set_content(source.id, "Detached".into(), Vec::new())
            .expect("remove references"),
        vrac::ContentUpdate {
            references: Vec::new(),
            materialized_nodes: Vec::new(),
            pruned_roots: vec![empty.id],
        }
    );
    assert!(engine.node(empty.id).unwrap().is_none());
    for preserved in [shared.id, tagged.id, nested.id, with_child.id, outgoing.id] {
        assert!(engine.node(preserved).unwrap().is_some());
    }
    assert!(engine.check().unwrap().is_ok());
}

#[test]
fn complete_reference_syntax_reuses_or_creates_concepts_atomically() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let existing = create(&mut engine, "Existing");
    let source = create(&mut engine, "Draft");
    let text = "[[Existing]] then [[New concept]] and [[New concept]]";

    let update = engine
        .set_content(source.id, text.into(), Vec::new())
        .expect("materialize references");

    assert_eq!(update.references.len(), 3);
    assert_eq!(update.references[0].target_id, existing.id);
    assert_eq!(
        update.references[1].target_id,
        update.references[2].target_id
    );
    assert_eq!(update.materialized_nodes.len(), 1);
    assert_eq!(update.materialized_nodes[0].text, "New concept");
    assert_eq!(
        update.materialized_nodes[0].id,
        update.references[1].target_id
    );
    assert!(update.pruned_roots.is_empty());
    assert_eq!(
        engine.node(source.id).unwrap().unwrap().references,
        update.references
    );

    let created = engine
        .create_node(CreateNode::new("Created with [[Create concept]]"))
        .expect("materialize while creating");
    assert_eq!(created.references[0].target_text, "Create concept");

    let plain = create(&mut engine, "Plain");
    engine
        .set_text(plain.id, "Changed to [[Text concept]]".into())
        .expect("materialize through text replacement");
    assert_eq!(
        engine.node(plain.id).unwrap().unwrap().references[0].target_text,
        "Text concept"
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
    assert_eq!(engine.check().expect("check rollback").node_count, 1);
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
        [Node {
            has_children: true,
            ..root.clone()
        }]
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
fn node_reads_include_current_child_presence() {
    let mut engine = Engine::open(":memory:").expect("open database");
    let root = create(&mut engine, "Root");
    assert!(!root.has_children);

    let mut child_input = CreateNode::new("Child");
    child_input.parent_id = Some(root.id);
    let child = engine.create_node(child_input).expect("create child");
    let mut leaf_input = CreateNode::new("Leaf");
    leaf_input.parent_id = Some(child.id);
    let leaf = engine.create_node(leaf_input).expect("create leaf");

    assert!(engine.node(root.id).unwrap().unwrap().has_children);
    assert!(
        engine
            .children(None, Page::default())
            .unwrap()
            .nodes
            .into_iter()
            .find(|node| node.id == root.id)
            .unwrap()
            .has_children
    );
    assert!(engine.node(child.id).unwrap().unwrap().has_children);
    assert!(!engine.node(leaf.id).unwrap().unwrap().has_children);
    assert_eq!(
        engine
            .path(leaf.id)
            .unwrap()
            .iter()
            .map(|node| node.has_children)
            .collect::<Vec<_>>(),
        [true, true, false]
    );

    engine.delete_node(leaf.id).expect("delete leaf");
    assert!(!engine.node(child.id).unwrap().unwrap().has_children);
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
                limit: 4,
                after: None,
            },
        )
        .expect("read first page");
    let second = engine
        .children(
            None,
            Page {
                limit: 4,
                after: first.next,
            },
        )
        .expect("read second page");
    let nodes: Vec<_> = first
        .nodes
        .into_iter()
        .chain(second.nodes)
        .filter(|node| node.system.is_none())
        .collect();
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
        (
            "SELECT parent_id FROM nodes WHERE parent_id IN (?1) GROUP BY parent_id",
            "nodes_by_parent",
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

    let mut statement = connection
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT DISTINCT tag
             FROM node_tags INDEXED BY node_tags_by_tag
             WHERE tag >= ?1 AND tag < ?2
             ORDER BY tag
             LIMIT ?3",
        )
        .expect("prepare tag completion query plan");
    let plan: Vec<String> = statement
        .query_map(params!["dec", "ded", 8], |row| row.get(3))
        .expect("read tag completion query plan")
        .collect::<rusqlite::Result<_>>()
        .expect("collect tag completion query plan");
    assert!(
        plan.iter().any(|step| step.contains("node_tags_by_tag")),
        "tag completion does not use node_tags_by_tag: {plan:?}"
    );
    assert!(
        plan.iter().all(|step| !step.contains("USE TEMP B-TREE")),
        "tag completion sorts outside the index: {plan:?}"
    );

    let mut statement = connection
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT id
             FROM nodes INDEXED BY nodes_by_root_text
             WHERE parent_id IS NULL AND text = ?1
             ORDER BY text, position, id
             LIMIT 1",
        )
        .expect("prepare root concept query plan");
    let plan: Vec<String> = statement
        .query_map(["Concept"], |row| row.get(3))
        .expect("read root concept query plan")
        .collect::<rusqlite::Result<_>>()
        .expect("collect root concept query plan");
    assert!(
        plan.iter().any(|step| step.contains("nodes_by_root_text")),
        "concept lookup does not use nodes_by_root_text: {plan:?}"
    );
    assert!(
        plan.iter().all(|step| !step.contains("USE TEMP B-TREE")),
        "concept lookup sorts outside the index: {plan:?}"
    );
}
