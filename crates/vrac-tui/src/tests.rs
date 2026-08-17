use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use vrac_engine::{CreateNode, Engine, Node, Page, Placement};
use vrac_workspace::remember_folder;

use super::WorkspaceSelection;
use super::commands::Command;
use super::editor::{EditTarget, Editor};
use super::model::{Action, App};
use super::prompts::{LauncherItem, LauncherKind, TagTarget};
use super::session::{actionable_key, choose_workspace_folder, open_workspace};
use super::ui::{display_lines, draw_inline_content, split_content, wrap_text};

#[test]
fn absolute_workspace_arguments_are_kept() {
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("vrac-workspace");
    std::fs::create_dir(&folder).unwrap();
    assert_eq!(
        choose_workspace_folder(
            WorkspaceSelection::Folder(folder.clone()),
            Path::new("unused")
        )
        .unwrap(),
        folder.canonicalize().unwrap()
    );
}

#[test]
fn an_unavailable_configured_folder_is_not_recreated() {
    let data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("missing-workspace");
    remember_folder(data.path(), &folder).unwrap();

    assert!(choose_workspace_folder(WorkspaceSelection::Remembered, data.path()).is_err());
    assert!(!folder.exists());
}

#[test]
fn a_workspace_that_cannot_open_can_be_replaced_before_the_tui_starts() {
    let data = tempfile::tempdir().unwrap();
    let providers = tempfile::tempdir().unwrap();
    let unavailable = providers.path().join("unavailable");
    let replacement = providers.path().join("replacement");
    std::fs::create_dir(&unavailable).unwrap();
    std::fs::create_dir(&replacement).unwrap();
    std::fs::write(unavailable.join("workspace-id"), "invalid").unwrap();
    let replacement = replacement.canonicalize().unwrap();
    let mut folder = unavailable.canonicalize().unwrap();
    let mut prompts = 0;

    let opened = open_workspace(data.path(), &mut folder, |status| {
        prompts += 1;
        assert!(status.contains("Current workspace cannot be opened"));
        Ok(Some(replacement.clone()))
    })
    .unwrap();

    assert_eq!(prompts, 1);
    assert_eq!(folder, replacement);
    assert_eq!(opened.workspace.folder(), replacement);
}

fn test_app() -> (App, Node, Node) {
    let mut engine = Engine::open(":memory:").unwrap();
    let parent = engine.create_node(CreateNode::new("Parent")).unwrap().node;
    let mut child_input = CreateNode::new("Child");
    child_input.parent_id = Some(parent.id);
    let child = engine.create_node(child_input).unwrap().node;
    (App::open_with_focus(engine, None).unwrap(), parent, child)
}

#[test]
fn editor_uses_character_offsets_for_unicode() {
    let mut editor = Editor::new(
        EditTarget::New {
            parent_id: None,
            placement: Placement::Last,
        },
        "été".into(),
        Vec::new(),
        Vec::new(),
    );
    editor.cursor = 1;
    editor.insert('🙂');
    assert_eq!(editor.text, "é🙂té");
    editor.backspace();
    assert_eq!(editor.text, "été");
    editor.delete();
    assert_eq!(editor.text, "éé");
}

#[test]
fn styled_lines_never_split_inside_unicode_markers() {
    assert_eq!(split_content("› item", "› ".len()), ("› ", "item"));
    assert_eq!(split_content("› item", 2), ("", "› item"));
}

#[test]
fn selected_style_resumes_after_references_and_tags() {
    let mut output = Vec::new();
    draw_inline_content(&mut output, "before [[Target]] after #task end", true).unwrap();
    let rendered = String::from_utf8(output).unwrap();
    let after_reference = rendered.split_once("]]").unwrap().1;
    let after_tag = rendered.split_once("#task").unwrap().1;

    assert!(after_reference.contains("\u{1b}[1m after"));
    assert!(after_tag.contains("\u{1b}[1m end"));
}

#[test]
fn editor_moves_by_visual_lines_and_words() {
    let mut editor = Editor::new(
        EditTarget::New {
            parent_id: None,
            placement: Placement::Last,
        },
        "alpha beta".into(),
        Vec::new(),
        Vec::new(),
    );
    editor.cursor = 9;
    editor.move_vertical(-1, 5);
    assert_eq!(editor.cursor, 4);
    editor.move_vertical(1, 5);
    assert_eq!(editor.cursor, 9);

    editor.move_word(-1);
    assert_eq!(editor.cursor, 6);
    editor.backspace_word();
    assert_eq!(editor.text, "beta");
    assert_eq!(editor.cursor, 0);
}

#[test]
fn home_and_end_follow_the_wrapped_visual_line() {
    let mut editor = Editor::new(
        EditTarget::New {
            parent_id: None,
            placement: Placement::Last,
        },
        "abcdefghij".into(),
        Vec::new(),
        Vec::new(),
    );
    editor.cursor = 7;
    editor.move_to_visual_edge(false, 4);
    assert_eq!(editor.cursor, 4);
    editor.move_to_visual_edge(true, 4);
    assert_eq!(editor.cursor, 7);
}

#[test]
fn bracketed_paste_is_inserted_once_and_flattens_line_breaks() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);
    app.start_new_sibling();

    app.handle_paste("first line\r\nsecond line").unwrap();

    assert_eq!(app.editor.as_ref().unwrap().text, "first line second line");
}

#[test]
fn held_navigation_keys_are_actionable() {
    assert!(actionable_key(KeyEventKind::Press));
    assert!(actionable_key(KeyEventKind::Repeat));
    assert!(!actionable_key(KeyEventKind::Release));
}

#[test]
fn question_mark_opens_and_closes_help_without_changing_selection() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);

    app.handle_normal_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
        .unwrap();
    assert!(app.help);
    assert_eq!(app.selected, Some(parent.id));

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    assert!(!app.help);
    assert_eq!(app.selected, Some(parent.id));
}

#[test]
fn normal_startup_focuses_today() {
    let app = App::open_with_lines(Engine::open(":memory:").unwrap(), true).unwrap();
    let today = jiff::Zoned::now().date().to_string();
    assert!(matches!(
        app.focus_path.last().and_then(|node| node.system.as_ref()),
        Some(vrac_engine::SystemNode::JournalDay { date }) if date == &today
    ));
}

#[test]
fn navigation_loads_only_an_opened_branch() {
    let (mut app, parent, child) = test_app();
    app.selected = Some(parent.id);

    app.move_right().unwrap();
    assert!(app.expanded.contains(&parent.id));
    assert_eq!(app.selected, Some(child.id));
    assert_eq!(
        app.visible_nodes()
            .iter()
            .filter(|item| item.node.id == child.id)
            .count(),
        1
    );

    app.move_left().unwrap();
    assert_eq!(app.selected, Some(parent.id));
}

#[test]
fn visible_nodes_record_depth_without_sibling_dependent_guides() {
    let (mut app, parent, _) = test_app();
    let following = app
        .engine
        .create_node(CreateNode::new("Following"))
        .unwrap()
        .node;
    app.reload_branch(None).unwrap();
    app.expand(parent.id).unwrap();

    let visible = app.visible_nodes();
    let child = visible
        .iter()
        .find(|item| item.node.parent_id == Some(parent.id))
        .unwrap();
    assert_eq!(child.depth, 1);
    assert_eq!(
        visible
            .iter()
            .find(|item| item.node.id == following.id)
            .unwrap()
            .depth,
        0
    );
}

#[test]
fn zoom_keeps_a_path_and_returns_to_the_previous_level() {
    let (mut app, parent, child) = test_app();
    app.selected = Some(parent.id);

    app.zoom_selected().unwrap();
    assert_eq!(app.focus, Some(parent.id));
    assert_eq!(app.selected, Some(child.id));
    assert_eq!(app.focus_label(), "root › Parent");

    app.zoom_out().unwrap();
    assert_eq!(app.focus, None);
    assert_eq!(app.selected, Some(parent.id));
}

#[test]
fn left_collapses_an_expanded_node_before_selecting_its_parent() {
    let (mut app, parent, child) = test_app();
    app.selected = Some(parent.id);
    app.expand(parent.id).unwrap();

    app.handle_normal_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE))
        .unwrap();

    assert_eq!(app.selected, Some(parent.id));
    assert!(!app.expanded.contains(&parent.id));
    assert!(
        app.visible_nodes()
            .iter()
            .all(|item| item.node.id != child.id)
    );
}

#[test]
fn left_stops_at_the_zoom_boundary_and_capital_h_zooms_out() {
    let (mut app, parent, child) = test_app();
    app.selected = Some(parent.id);
    app.zoom_selected().unwrap();
    assert_eq!(app.focus, Some(parent.id));
    assert_eq!(app.selected, Some(child.id));

    app.handle_normal_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.focus, Some(parent.id));
    assert_eq!(app.selected, Some(child.id));

    app.handle_normal_key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT))
        .unwrap();
    assert_eq!(app.focus, None);
    assert_eq!(app.selected, Some(parent.id));
}

#[test]
fn an_incomplete_normal_mode_prefix_does_not_leave_stale_status() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);

    app.handle_normal_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.pending_key, Some('y'));
    assert_eq!(app.status, "y");

    app.handle_normal_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.pending_key, None);
    assert!(app.status.is_empty());
}

#[test]
fn moving_down_loads_the_next_sibling_page() {
    let mut engine = Engine::open(":memory:").unwrap();
    for index in 0..101 {
        engine
            .create_node(CreateNode::new(format!("Node {index:03}")))
            .unwrap();
    }
    let mut app = App::open_with_focus(engine, None).unwrap();
    let branch = app.branches.get(&None).unwrap();
    assert_eq!(branch.nodes.len(), Page::default().limit);
    assert!(branch.next.is_some());
    app.selected = branch.nodes.last().map(|node| node.id);

    app.move_selection(1).unwrap();

    assert!(app.branches.get(&None).unwrap().nodes.len() > Page::default().limit);
    assert_eq!(
        app.selected_node().unwrap().text,
        "Node 099",
        "navigation continues in sibling order after loading"
    );
}

#[test]
fn search_opens_a_result_as_the_new_focus() {
    let (mut app, parent, _) = test_app();
    app.engine
        .set_text(parent.id, "Vrac concept".into())
        .unwrap();
    app.start_launcher(LauncherKind::Search).unwrap();
    for character in "vrac".chars() {
        app.launcher.as_mut().unwrap().insert(character);
    }
    app.refresh_launcher().unwrap();

    assert!(matches!(
        &app.launcher.as_ref().unwrap().items[0],
        LauncherItem::Node(node) if node.id == parent.id
    ));
    app.handle_launcher_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.focus, Some(parent.id));
    assert_eq!(app.focus_label(), "root › Vrac concept");
}

#[test]
fn tag_prompt_toggles_a_node_property() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);
    app.start_tag_prompt().unwrap();
    app.tag_prompt.as_mut().unwrap().query = "task".into();
    app.refresh_tag_prompt().unwrap();
    app.commit_tag_prompt().unwrap();
    assert_eq!(app.engine.node(parent.id).unwrap().unwrap().tags, ["task"]);

    app.start_tag_prompt().unwrap();
    app.tag_prompt.as_mut().unwrap().query = "task".into();
    app.refresh_tag_prompt().unwrap();
    app.commit_tag_prompt().unwrap();
    assert!(app.engine.node(parent.id).unwrap().unwrap().tags.is_empty());

    app.start_edit();
    app.handle_editor_key(KeyEvent::new(KeyCode::Char('#'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(
        app.tag_prompt.as_ref().unwrap().target,
        TagTarget::Node(parent.id)
    );
    assert_eq!(app.editor.as_ref().unwrap().text, "Parent");
}

#[test]
fn inline_tag_completion_is_kept_on_a_new_draft() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);
    app.start_new_sibling();
    app.editor.as_mut().unwrap().text = "Tagged draft".into();

    app.handle_editor_key(KeyEvent::new(KeyCode::Char('#'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.tag_prompt.as_ref().unwrap().target, TagTarget::Draft);
    app.tag_prompt.as_mut().unwrap().query = "task".into();
    app.refresh_tag_prompt().unwrap();
    app.commit_tag_prompt().unwrap();

    assert_eq!(app.editor.as_ref().unwrap().tags, ["task"]);
    assert!(
        display_lines(&app, 80)
            .iter()
            .any(|line| line.text.contains("#task"))
    );

    app.handle_editor_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    let created = app
        .engine
        .children(None, Page::default())
        .unwrap()
        .nodes
        .into_iter()
        .find(|node| node.text == "Tagged draft")
        .unwrap();
    assert_eq!(created.tags, ["task"]);
}

#[test]
fn inline_tag_completion_has_natural_cancel_keys() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);
    app.start_new_sibling();
    app.editor.as_mut().unwrap().text = "Draft".into();
    app.editor.as_mut().unwrap().cursor = "Draft".chars().count();

    app.handle_editor_key(KeyEvent::new(KeyCode::Char('#'), KeyModifiers::NONE))
        .unwrap();
    app.handle_tag_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
        .unwrap();
    assert!(app.tag_prompt.is_none());

    app.handle_editor_key(KeyEvent::new(KeyCode::Char('#'), KeyModifiers::NONE))
        .unwrap();
    app.handle_tag_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE))
        .unwrap();
    app.handle_tag_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
        .unwrap();
    assert!(app.tag_prompt.is_none());
    assert_eq!(app.editor.as_ref().unwrap().text, "Draft ");
    assert!(app.editor.as_ref().unwrap().tags.is_empty());
}

#[test]
fn backlinks_open_the_matching_context() {
    let mut engine = Engine::open(":memory:").unwrap();
    let source = engine
        .create_node(CreateNode::new("See [[Target]]"))
        .unwrap()
        .node;
    let target = source.references[0].target_id;
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.selected = Some(target);

    app.start_backlinks().unwrap();
    let view = app.backlinks.as_ref().unwrap();
    assert_eq!(view.contexts[0].last().unwrap().id, source.id);

    app.handle_backlink_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.focus, Some(source.id));
}

#[test]
fn sync_reload_refreshes_an_open_backlink_view() {
    let mut engine = Engine::open(":memory:").unwrap();
    let source = engine
        .create_node(CreateNode::new("See [[Target]]"))
        .unwrap()
        .node;
    let target = source.references[0].target_id;
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.selected = Some(target);
    app.start_backlinks().unwrap();
    assert_eq!(app.backlinks.as_ref().unwrap().contexts.len(), 1);

    app.engine
        .create_node(CreateNode::new("Another [[Target]] reference"))
        .unwrap();
    app.reload_after_sync().unwrap();

    assert_eq!(app.backlinks.as_ref().unwrap().contexts.len(), 2);
}

#[test]
fn sync_reload_refreshes_open_search_results() {
    let (mut app, parent, _) = test_app();
    app.start_launcher(LauncherKind::Search).unwrap();
    for character in "Parent".chars() {
        app.launcher.as_mut().unwrap().insert(character);
    }
    app.refresh_launcher().unwrap();
    assert!(
        app.launcher
            .as_ref()
            .unwrap()
            .items
            .iter()
            .any(|item| matches!(item, LauncherItem::Node(node) if node.id == parent.id))
    );

    app.engine
        .set_text(parent.id, "Renamed after sync".into())
        .unwrap();
    app.reload_after_sync().unwrap();

    assert!(
        app.launcher
            .as_ref()
            .unwrap()
            .items
            .iter()
            .all(|item| !matches!(item, LauncherItem::Node(node) if node.id == parent.id))
    );
}

#[test]
fn sync_reload_preserves_open_branches_and_the_visible_selection() {
    let (mut app, parent, child) = test_app();
    app.expand(parent.id).unwrap();
    app.selected = Some(child.id);

    app.engine
        .set_text(child.id, "Updated by sync".into())
        .unwrap();
    app.reload_after_sync().unwrap();

    assert!(app.expanded.contains(&parent.id));
    assert_eq!(app.selected, Some(child.id));
    assert_eq!(app.selected_node().unwrap().text, "Updated by sync");
}

#[test]
fn sync_reload_preserves_loaded_pages() {
    let mut engine = Engine::open(":memory:").unwrap();
    for index in 0..205 {
        engine
            .create_node(CreateNode::new(format!("Node {index:03}")))
            .unwrap();
    }
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.load_more(None).unwrap();
    let selected = app.branches[&None].nodes[150].id;
    let loaded = app.branches[&None].nodes.len();
    app.selected = Some(selected);

    app.engine
        .set_text(selected, "Updated beyond page one".into())
        .unwrap();
    app.reload_after_sync().unwrap();

    assert_eq!(app.branches[&None].nodes.len(), loaded);
    assert_eq!(app.selected, Some(selected));
    assert_eq!(app.selected_node().unwrap().text, "Updated beyond page one");
}

#[test]
fn indent_and_outdent_refresh_the_changed_parent() {
    let mut engine = Engine::open(":memory:").unwrap();
    let parent = engine.create_node(CreateNode::new("Parent")).unwrap().node;
    let sibling = engine.create_node(CreateNode::new("Sibling")).unwrap().node;
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.selected = Some(sibling.id);

    app.indent_selected().unwrap();
    assert_eq!(
        app.engine.node(sibling.id).unwrap().unwrap().parent_id,
        Some(parent.id)
    );
    app.selected = Some(parent.id);
    assert!(app.selected_node().unwrap().has_children);
    assert!(app.expanded.contains(&parent.id));
    assert!(
        app.visible_nodes()
            .iter()
            .any(|item| item.node.id == sibling.id)
    );

    app.selected = Some(sibling.id);
    app.outdent_selected().unwrap();
    assert_eq!(
        app.engine.node(sibling.id).unwrap().unwrap().parent_id,
        None
    );
    app.selected = Some(parent.id);
    assert!(!app.selected_node().unwrap().has_children);
    assert!(!app.expanded.contains(&parent.id));
    assert!(!app.branches.contains_key(&Some(parent.id)));
}

#[test]
fn editing_and_creation_go_through_the_engine() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);
    app.start_edit();
    app.editor.as_mut().unwrap().text = "Renamed".into();
    app.commit_editor().unwrap();
    assert_eq!(app.engine.node(parent.id).unwrap().unwrap().text, "Renamed");

    app.start_new_child().unwrap();
    app.editor.as_mut().unwrap().text = "New child".into();
    app.commit_editor().unwrap();
    let children = app
        .engine
        .children(Some(parent.id), Page::default())
        .unwrap();
    assert!(children.nodes.iter().any(|node| node.text == "New child"));
}

#[test]
fn creating_a_child_refreshes_and_expands_its_parent() {
    let mut engine = Engine::open(":memory:").unwrap();
    let parent = engine.create_node(CreateNode::new("Parent")).unwrap().node;
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.selected = Some(parent.id);
    assert!(!app.selected_node().unwrap().has_children);

    app.start_new_child().unwrap();
    app.editor.as_mut().unwrap().text = "Child".into();
    let child = app.commit_editor().unwrap().unwrap();

    app.selected = Some(parent.id);
    assert!(app.selected_node().unwrap().has_children);
    assert!(app.expanded.contains(&parent.id));
    assert!(
        app.visible_nodes()
            .iter()
            .any(|item| item.node.id == child.id)
    );

    app.toggle_selected().unwrap();
    assert!(!app.expanded.contains(&parent.id));
    app.toggle_selected().unwrap();
    assert!(app.expanded.contains(&parent.id));
}

#[test]
fn removing_the_last_child_clears_parent_expansion_state() {
    let mut engine = Engine::open(":memory:").unwrap();
    let parent = engine.create_node(CreateNode::new("Parent")).unwrap().node;
    let mut child_input = CreateNode::new("Child");
    child_input.parent_id = Some(parent.id);
    let child = engine.create_node(child_input).unwrap().node;
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.expand(parent.id).unwrap();
    assert!(app.expanded.contains(&parent.id));

    app.engine.delete_node(child.id).unwrap();
    app.reload_branch(Some(parent.id)).unwrap();
    app.refresh_cached_node(parent.id).unwrap();

    app.selected = Some(parent.id);
    assert!(!app.selected_node().unwrap().has_children);
    assert!(!app.expanded.contains(&parent.id));
    assert!(!app.branches.contains_key(&Some(parent.id)));
}

#[test]
fn creating_a_nested_reference_refreshes_materialized_root_concepts() {
    let mut engine = Engine::open(":memory:").unwrap();
    let parent = engine.create_node(CreateNode::new("Parent")).unwrap().node;
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.selected = Some(parent.id);

    app.start_new_child().unwrap();
    app.editor.as_mut().unwrap().text = "See [[Concept]]".into();
    app.commit_editor().unwrap();

    assert!(
        app.branches[&None]
            .nodes
            .iter()
            .any(|node| node.text == "Concept")
    );
}

#[test]
fn creating_a_nested_date_reference_refreshes_the_open_journal() {
    let mut engine = Engine::open(":memory:").unwrap();
    let parent = engine.create_node(CreateNode::new("Parent")).unwrap().node;
    let journal = engine
        .children(None, Page::default())
        .unwrap()
        .nodes
        .into_iter()
        .find(|node| matches!(node.system, Some(vrac_engine::SystemNode::Journal)))
        .unwrap();
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.expand(journal.id).unwrap();
    assert!(
        app.branches[&Some(journal.id)]
            .nodes
            .iter()
            .all(|node| node.text != "2030-01-01")
    );

    app.selected = Some(parent.id);
    app.start_new_child().unwrap();
    app.editor.as_mut().unwrap().text = "See [[2030-01-01]]".into();
    app.commit_editor().unwrap();

    assert!(
        app.branches[&Some(journal.id)]
            .nodes
            .iter()
            .any(|node| node.text == "2030-01-01")
    );
}

#[test]
fn enter_persists_and_continues_with_a_sibling_draft() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);
    app.start_edit();
    app.editor.as_mut().unwrap().text = "Renamed".into();

    app.handle_editor_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    assert_eq!(app.engine.node(parent.id).unwrap().unwrap().text, "Renamed");
    assert_eq!(
        app.editor.as_ref().unwrap().target,
        EditTarget::New {
            parent_id: None,
            placement: Placement::After(parent.id),
        }
    );
    assert_eq!(app.selected, Some(parent.id));
}

#[test]
fn vertical_arrows_cross_inline_editor_boundaries() {
    let mut engine = Engine::open(":memory:").unwrap();
    let first = engine.create_node(CreateNode::new("First")).unwrap().node;
    let second = engine.create_node(CreateNode::new("Second")).unwrap().node;
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.selected = Some(first.id);
    app.start_edit();

    app.handle_editor_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .unwrap();

    assert!(matches!(
        app.editor.as_ref().unwrap().target,
        EditTarget::Existing(id) if id == second.id
    ));
    assert_eq!(app.editor.as_ref().unwrap().cursor, 0);

    app.handle_editor_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
        .unwrap();

    assert!(matches!(
        app.editor.as_ref().unwrap().target,
        EditTarget::Existing(id) if id == first.id
    ));
    assert_eq!(
        app.editor.as_ref().unwrap().cursor,
        first.text.chars().count()
    );
}

#[test]
fn editing_down_loads_the_next_sibling_page() {
    let mut engine = Engine::open(":memory:").unwrap();
    for index in 0..101 {
        engine
            .create_node(CreateNode::new(format!("Node {index:03}")))
            .unwrap();
    }
    let mut app = App::open_with_focus(engine, None).unwrap();
    let selected = app.branches[&None].nodes.last().unwrap().id;
    let after = app.branches[&None].next;
    let expected = app
        .engine
        .children(None, Page { limit: 1, after })
        .unwrap()
        .nodes[0]
        .id;
    app.selected = Some(selected);
    app.start_edit();

    app.handle_editor_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .unwrap();

    assert!(matches!(
        app.editor.as_ref().unwrap().target,
        EditTarget::Existing(id) if id == expected
    ));
    assert_eq!(app.editor.as_ref().unwrap().cursor, 0);
}

#[test]
fn relative_creation_after_loaded_pages_keeps_the_editor_visible() {
    let mut engine = Engine::open(":memory:").unwrap();
    for index in 0..205 {
        engine
            .create_node(CreateNode::new(format!("Node {index:03}")))
            .unwrap();
    }
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.load_more(None).unwrap();
    let reference = app.branches[&None].nodes[150].id;
    app.selected = Some(reference);
    app.start_new_sibling();
    app.editor.as_mut().unwrap().text = "Inserted after page one".into();

    app.handle_editor_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    let created = app.selected.unwrap();
    assert!(app.is_visible(created));
    assert!(matches!(
        app.editor.as_ref().unwrap().target,
        EditTarget::New {
            placement: Placement::After(id),
            ..
        } if id == created
    ));
    assert!(
        display_lines(&app, 80)
            .iter()
            .any(|line| line.cursor.is_some())
    );
}

#[test]
fn repeated_relative_creation_does_not_load_extra_pages() {
    let mut engine = Engine::open(":memory:").unwrap();
    for index in 0..205 {
        engine
            .create_node(CreateNode::new(format!("Node {index:03}")))
            .unwrap();
    }
    let mut app = App::open_with_focus(engine, None).unwrap();
    let selected = app.branches[&None]
        .nodes
        .iter()
        .find(|node| node.system.is_none())
        .unwrap()
        .id;
    app.selected = Some(selected);
    let loaded = app.branches[&None].nodes.len();

    for index in 0..5 {
        app.start_new_before();
        app.editor.as_mut().unwrap().text = format!("Inserted {index}");
        app.commit_editor().unwrap();
        assert_eq!(app.branches[&None].nodes.len(), loaded);
    }
}

#[test]
fn indenting_beyond_a_loaded_page_never_leaves_an_invisible_editor() {
    let mut engine = Engine::open(":memory:").unwrap();
    let parent = engine.create_node(CreateNode::new("Parent")).unwrap().node;
    for index in 0..101 {
        let mut input = CreateNode::new(format!("Child {index:03}"));
        input.parent_id = Some(parent.id);
        engine.create_node(input).unwrap();
    }
    let sibling = engine.create_node(CreateNode::new("Sibling")).unwrap().node;
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.selected = Some(sibling.id);
    app.start_edit();

    app.handle_editor_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .unwrap();

    assert!(app.editor.is_none());
    assert_eq!(app.selected, Some(parent.id));
    assert_eq!(app.status, "Indented outside the loaded page");
    assert!(!app.is_visible(sibling.id));
}

#[test]
fn control_enter_persists_and_zooms_the_edited_node() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);
    app.start_edit();
    app.editor.as_mut().unwrap().text = "Zoomed".into();

    app.handle_editor_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
        .unwrap();

    assert!(app.editor.is_none());
    assert_eq!(app.focus, Some(parent.id));
    assert_eq!(app.focus_label(), "root › Zoomed");
}

#[test]
fn control_c_persists_before_quitting() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);
    app.start_edit();
    app.editor.as_mut().unwrap().text = "Safe quit".into();

    let action = app
        .handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
        .unwrap();

    assert_eq!(action, Action::Quit);
    assert_eq!(
        app.engine.node(parent.id).unwrap().unwrap().text,
        "Safe quit"
    );
}

#[test]
fn sync_command_requests_a_provider_round() {
    let (mut app, _, _) = test_app();
    assert_eq!(app.run_command(Command::Sync).unwrap(), Action::Sync);
}

#[test]
fn workspace_command_requests_the_folder_picker() {
    let (mut app, _, _) = test_app();
    assert_eq!(
        app.run_command(Command::Workspace).unwrap(),
        Action::ChooseWorkspace
    );
}

#[test]
fn lines_commands_request_persistent_presentation_changes() {
    let (mut app, _, _) = test_app();

    assert_eq!(
        app.run_command(Command::LinesOff).unwrap(),
        Action::SetLines(false)
    );
    assert_eq!(
        app.run_command(Command::LinesOn).unwrap(),
        Action::SetLines(true)
    );
}

#[test]
fn tab_and_backtab_move_a_node_without_leaving_inline_editing() {
    let (mut app, parent, _) = test_app();
    let sibling = app
        .engine
        .create_node(CreateNode::new("Sibling"))
        .unwrap()
        .node;
    app.reload_branch(None).unwrap();
    app.selected = Some(sibling.id);
    app.start_edit();
    app.editor.as_mut().unwrap().text = "Changed".into();

    app.handle_editor_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .unwrap();

    assert_eq!(
        app.engine.node(sibling.id).unwrap().unwrap().parent_id,
        Some(parent.id)
    );
    assert!(matches!(
        app.editor.as_ref().unwrap().target,
        EditTarget::Existing(id) if id == sibling.id
    ));
    assert_eq!(app.editor.as_ref().unwrap().text, "Changed");

    app.handle_editor_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))
        .unwrap();

    assert_eq!(
        app.engine.node(sibling.id).unwrap().unwrap().parent_id,
        None
    );
    assert!(matches!(
        app.editor.as_ref().unwrap().target,
        EditTarget::Existing(id) if id == sibling.id
    ));
}

#[test]
fn tab_retargets_an_uncommitted_sibling_draft() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);
    app.start_new_sibling();
    app.editor.as_mut().unwrap().text = "Nested draft".into();

    app.handle_editor_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .unwrap();

    assert_eq!(
        app.editor.as_ref().unwrap().target,
        EditTarget::New {
            parent_id: Some(parent.id),
            placement: Placement::Last,
        }
    );
    assert_eq!(
        app.engine
            .children(Some(parent.id), Page::default())
            .unwrap()
            .nodes
            .len(),
        1,
        "Tab does not create the draft before Enter"
    );
}

#[test]
fn escape_persists_text_but_discards_an_empty_draft() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);
    app.start_edit();
    app.editor.as_mut().unwrap().text = "Saved on escape".into();

    app.handle_editor_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();

    assert!(app.editor.is_none());
    assert_eq!(
        app.engine.node(parent.id).unwrap().unwrap().text,
        "Saved on escape"
    );

    app.start_new_sibling();
    app.handle_editor_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    assert!(app.editor.is_none());
}

#[test]
fn slash_searches_nodes_and_colon_opens_commands() {
    let (mut app, _, _) = test_app();

    app.handle_normal_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.launcher.as_ref().unwrap().kind, LauncherKind::Search);
    assert!(
        app.launcher
            .as_ref()
            .unwrap()
            .items
            .iter()
            .all(|item| matches!(item, LauncherItem::Node(_)))
    );
    app.handle_launcher_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();

    app.handle_normal_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.launcher.as_ref().unwrap().kind, LauncherKind::Commands);
    assert!(
        app.launcher.as_ref().unwrap().items.iter().any(
            |item| matches!(item, LauncherItem::Command(entry) if entry.command == Command::New)
        )
    );
    assert!(
        app.launcher
            .as_ref()
            .unwrap()
            .items
            .iter()
            .all(|item| matches!(item, LauncherItem::Command(_)))
    );
}

#[test]
fn insert_append_and_open_before_match_the_graphical_navigation() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);

    app.handle_normal_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.editor.as_ref().unwrap().cursor, 0);
    app.handle_editor_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();

    app.handle_normal_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(
        app.editor.as_ref().unwrap().cursor,
        "Parent".chars().count()
    );
    app.handle_editor_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();

    app.handle_normal_key(KeyEvent::new(KeyCode::Char('O'), KeyModifiers::SHIFT))
        .unwrap();
    assert_eq!(
        app.editor.as_ref().unwrap().target,
        EditTarget::New {
            parent_id: None,
            placement: Placement::Before(parent.id),
        }
    );
}

#[test]
fn editing_and_creation_render_the_caret_inside_the_outline() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);
    app.start_edit();
    let editing = display_lines(&app, 40);
    assert!(editing.iter().any(|line| line.cursor.is_some()));
    assert!(editing.iter().any(|line| line.text.contains("Parent")));

    app.editor = None;
    app.start_new_sibling();
    app.editor.as_mut().unwrap().insert('N');
    let creating = display_lines(&app, 40);
    let draft = creating.iter().find(|line| line.cursor.is_some()).unwrap();
    assert!(draft.text.contains('N'));
}

#[test]
fn a_new_draft_is_the_only_selected_outline_item() {
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);
    app.start_new_sibling();

    let lines = display_lines(&app, 80);
    assert!(
        lines
            .iter()
            .find(|line| line.text.contains("Parent"))
            .is_some_and(|line| !line.selected)
    );
    assert_eq!(lines.iter().filter(|line| line.cursor.is_some()).count(), 1);
}

#[test]
fn editing_preserves_untouched_stable_references() {
    let mut engine = Engine::open(":memory:").unwrap();
    let source = engine
        .create_node(CreateNode::new("See [[Target]]"))
        .unwrap()
        .node;
    let target = source.references[0].target_id;
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.selected = Some(source.id);
    app.start_edit();
    app.editor.as_mut().unwrap().insert('!');
    app.commit_editor().unwrap();

    let updated = app.engine.node(source.id).unwrap().unwrap();
    assert_eq!(updated.references[0].target_id, target);

    app.start_edit();
    let editor = app.editor.as_mut().unwrap();
    editor.cursor = "See ".chars().count();
    editor.delete();
    app.commit_editor().unwrap();
    assert!(
        app.engine
            .node(source.id)
            .unwrap()
            .unwrap()
            .references
            .is_empty()
    );
    assert!(app.engine.node(target).unwrap().is_none());
}

#[test]
fn inline_reference_completion_keeps_the_selected_identity() {
    let mut engine = Engine::open(":memory:").unwrap();
    let target = engine.create_node(CreateNode::new("Project")).unwrap().node;
    let source = engine.create_node(CreateNode::new("See ")).unwrap().node;
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.selected = Some(source.id);
    app.start_edit();
    for character in "[[pro".chars() {
        let key = KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE);
        if app.reference_prompt.is_some() {
            app.handle_reference_key(key).unwrap();
        } else {
            app.handle_editor_key(key).unwrap();
        }
    }
    assert_eq!(
        app.reference_prompt.as_ref().unwrap().results[0].id,
        target.id
    );
    app.handle_reference_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    app.commit_editor().unwrap();

    let updated = app.engine.node(source.id).unwrap().unwrap();
    assert_eq!(updated.text, "See [[Project]]");
    assert_eq!(updated.references[0].target_id, target.id);
}

#[test]
fn typing_or_pasting_closing_brackets_leaves_reference_completion() {
    let mut engine = Engine::open(":memory:").unwrap();
    let typed_source = engine.create_node(CreateNode::new("Typed ")).unwrap().node;
    let pasted_source = engine.create_node(CreateNode::new("Pasted ")).unwrap().node;
    let mut app = App::open_with_focus(engine, None).unwrap();

    app.selected = Some(typed_source.id);
    app.start_edit();
    for character in "[[Typed concept]]".chars() {
        let key = KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE);
        if app.reference_prompt.is_some() {
            app.handle_reference_key(key).unwrap();
        } else {
            app.handle_editor_key(key).unwrap();
        }
    }
    assert!(app.reference_prompt.is_none());
    assert_eq!(app.editor.as_ref().unwrap().text, "Typed [[Typed concept]]");
    app.commit_editor().unwrap();

    app.selected = Some(pasted_source.id);
    app.start_edit();
    app.handle_editor_key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE))
        .unwrap();
    app.handle_editor_key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE))
        .unwrap();
    app.handle_paste("Pasted concept]] suffix").unwrap();
    assert!(app.reference_prompt.is_none());
    assert_eq!(
        app.editor.as_ref().unwrap().text,
        "Pasted [[Pasted concept]] suffix"
    );
    app.commit_editor().unwrap();

    let roots = &app.branches[&None].nodes;
    assert!(roots.iter().any(|node| node.text == "Typed concept"));
    assert!(roots.iter().any(|node| node.text == "Pasted concept"));
}

#[test]
fn renaming_a_target_refreshes_loaded_references_without_rewriting_sources() {
    let mut engine = Engine::open(":memory:").unwrap();
    let target = engine.create_node(CreateNode::new("Project")).unwrap().node;
    let source = engine
        .create_node(CreateNode::new("See [[Project]]"))
        .unwrap()
        .node;
    let mut app = App::open_with_focus(engine, None).unwrap();
    app.selected = Some(target.id);
    app.start_edit();
    app.editor.as_mut().unwrap().text = "Renamed".into();
    app.commit_editor().unwrap();

    assert!(
        display_lines(&app, 80)
            .iter()
            .any(|line| line.text.contains("[[Renamed]]"))
    );

    app.selected = Some(source.id);
    app.start_edit();
    assert_eq!(app.editor.as_ref().unwrap().text, "See [[Project]]");
    app.finish_editor().unwrap();
    let stored = app.engine.node(source.id).unwrap().unwrap();
    assert_eq!(stored.text, "See [[Project]]");
    assert_eq!(stored.references[0].target_text, "Renamed");
}

#[test]
fn a_failed_creation_keeps_its_draft() {
    let (mut app, _, _) = test_app();
    let journal = app
        .branches
        .get(&None)
        .unwrap()
        .nodes
        .iter()
        .find(|node| matches!(node.system, Some(vrac_engine::SystemNode::Journal)))
        .unwrap()
        .id;
    app.selected = Some(journal);
    app.start_new_child().unwrap();
    app.editor.as_mut().unwrap().text = "Draft".into();

    assert!(app.commit_editor().is_err());
    assert_eq!(app.editor.as_ref().unwrap().text, "Draft");
}

#[test]
fn wrapped_lines_keep_the_text_aligned_after_the_bullet() {
    assert_eq!(wrap_text("abcdefgh", 3), ["abc", "def", "gh"]);
    let (mut app, parent, _) = test_app();
    app.selected = Some(parent.id);
    let lines = display_lines(&app, 8);
    let first = lines.iter().position(|line| line.selected).unwrap();
    assert!(lines[first + 1].text.starts_with("    "));
}

#[test]
fn top_level_nodes_are_compact() {
    let (mut app, _, _) = test_app();
    app.engine
        .create_node(CreateNode::new("Second root"))
        .unwrap();
    app.reload_branch(None).unwrap();
    let lines = display_lines(&app, 80);

    assert!(lines.iter().any(|line| line.text.contains("Parent")));
    assert!(lines.iter().any(|line| line.text.contains("Second root")));
    assert!(lines.iter().all(|line| !line.text.is_empty()));
}

#[test]
fn an_only_child_still_has_its_parent_guide() {
    let (mut app, parent, child) = test_app();
    app.expand(parent.id).unwrap();

    let lines = display_lines(&app, 80);
    let child_line = lines
        .iter()
        .find(|line| line.text.contains(&child.text))
        .unwrap();
    assert!(child_line.text.starts_with("  │   • "));
}

#[test]
fn disabling_lines_preserves_indentation_without_guides() {
    let (mut app, parent, child) = test_app();
    app.expand(parent.id).unwrap();
    app.lines = false;

    let lines = display_lines(&app, 80);
    let child_line = lines
        .iter()
        .find(|line| line.text.contains(&child.text))
        .unwrap();

    assert!(child_line.text.starts_with("      • "));
    assert!(!child_line.text.contains('│'));
}
