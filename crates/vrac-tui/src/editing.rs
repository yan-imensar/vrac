//! Inline editing and structural mutations.

use arboard::Clipboard;
use vrac_engine::{CreateNode, Destination, Node, NodeId, Placement, ReferenceInput};

use super::OUTLINE_INDENT;
use super::editor::{EditTarget, Editor};
use super::model::App;

impl App {
    pub(super) fn editor_width(&self) -> usize {
        let depth = match self.editor.as_ref().map(|editor| editor.target.clone()) {
            Some(EditTarget::Existing(id)) => self
                .visible_nodes()
                .iter()
                .find(|item| item.node.id == id)
                .map_or(0, |item| item.depth),
            Some(EditTarget::New {
                parent_id,
                placement,
            }) => match placement {
                Placement::Before(id) | Placement::After(id) => self
                    .visible_nodes()
                    .iter()
                    .find(|item| item.node.id == id)
                    .map_or(0, |item| item.depth),
                Placement::First | Placement::Last if parent_id == self.focus => 0,
                Placement::First | Placement::Last => parent_id
                    .and_then(|id| {
                        self.visible_nodes()
                            .iter()
                            .find(|item| item.node.id == id)
                            .map(|item| item.depth + 1)
                    })
                    .unwrap_or(0),
            },
            None => 0,
        };
        self.viewport_width
            .saturating_sub(4 + depth.saturating_mul(OUTLINE_INDENT))
            .max(1)
    }

    pub(super) fn move_editor_to_adjacent(&mut self, direction: isize) -> vrac_engine::Result<()> {
        if self.editor.as_ref().is_some_and(|editor| {
            matches!(editor.target, EditTarget::New { .. }) && editor.text.is_empty()
        }) {
            return Ok(());
        }
        let Some(saved) = self.commit_editor()? else {
            return Ok(());
        };
        if !self.is_visible(saved.id) {
            return Ok(());
        }
        self.selected = Some(saved.id);
        if direction > 0 {
            self.load_next_page_at_selection()?;
        }
        let visible = self.visible_nodes();
        let Some(index) = visible.iter().position(|item| item.node.id == saved.id) else {
            return Ok(());
        };
        let adjacent = if direction < 0 {
            visible[..index]
                .iter()
                .rev()
                .find(|item| item.node.system.is_none())
        } else {
            visible[index + 1..]
                .iter()
                .find(|item| item.node.system.is_none())
        };
        let target = adjacent.map_or(saved.id, |item| item.node.id);
        self.selected = Some(target);
        self.resume_editing(target)?;
        if direction > 0 {
            self.editor.as_mut().expect("editor was resumed").cursor = 0;
        }
        Ok(())
    }

    pub(super) fn start_edit(&mut self) {
        let Some(node) = self.selected_node() else {
            return;
        };
        if node.system.is_some() {
            self.status = "Protected Journal nodes cannot be edited".into();
            return;
        }
        let references = node
            .references
            .iter()
            .map(|reference| ReferenceInput {
                label_start: reference.label_start,
                label_end: reference.label_end,
                target_id: reference.target_id,
            })
            .collect();
        self.editor = Some(Editor::new(
            EditTarget::Existing(node.id),
            node.text,
            references,
            node.tags,
        ));
        self.status.clear();
    }

    pub(super) fn start_edit_at_start(&mut self) {
        self.start_edit();
        if let Some(editor) = &mut self.editor {
            editor.cursor = 0;
        }
    }

    pub(super) fn start_new_sibling(&mut self) {
        let target = match self.selected_node() {
            Some(node) => EditTarget::New {
                parent_id: node.parent_id,
                placement: Placement::After(node.id),
            },
            None => EditTarget::New {
                parent_id: self.focus,
                placement: Placement::Last,
            },
        };
        self.editor = Some(Editor::empty(target));
        self.status.clear();
    }

    pub(super) fn start_new_before(&mut self) {
        let target = match self.selected_node() {
            Some(node) => EditTarget::New {
                parent_id: node.parent_id,
                placement: Placement::Before(node.id),
            },
            None => EditTarget::New {
                parent_id: self.focus,
                placement: Placement::First,
            },
        };
        self.editor = Some(Editor::empty(target));
        self.status.clear();
    }

    pub(super) fn create_sibling_from_editor(&mut self) -> vrac_engine::Result<()> {
        if self.editor.as_ref().is_some_and(|editor| {
            matches!(editor.target, EditTarget::New { .. }) && editor.text.is_empty()
        }) {
            return Ok(());
        }
        let Some(saved) = self.commit_editor()? else {
            return Ok(());
        };
        if !self.is_visible(saved.id) {
            return Ok(());
        }
        self.selected = Some(saved.id);
        self.editor = Some(Editor::empty(EditTarget::New {
            parent_id: saved.parent_id,
            placement: Placement::After(saved.id),
        }));
        self.status.clear();
        Ok(())
    }

    pub(super) fn finish_editor(&mut self) -> vrac_engine::Result<()> {
        if self.editor.as_ref().is_some_and(|editor| {
            matches!(editor.target, EditTarget::New { .. }) && editor.text.is_empty()
        }) {
            self.editor = None;
            self.status.clear();
            return Ok(());
        }
        self.commit_editor()?;
        self.status.clear();
        Ok(())
    }

    pub(super) fn zoom_from_editor(&mut self) -> vrac_engine::Result<()> {
        if self.editor.as_ref().is_some_and(|editor| {
            matches!(editor.target, EditTarget::New { .. }) && editor.text.is_empty()
        }) {
            return Ok(());
        }
        if let Some(saved) = self.commit_editor()? {
            self.selected = Some(saved.id);
            self.set_focus(Some(saved.id))?;
            self.status.clear();
        }
        Ok(())
    }

    pub(super) fn indent_editor(&mut self) -> vrac_engine::Result<()> {
        if self.adjust_new_draft(true)? {
            return Ok(());
        }
        let Some(saved) = self.commit_editor()? else {
            return Ok(());
        };
        self.selected = Some(saved.id);
        self.indent_selected()?;
        if self.is_visible(saved.id) {
            self.resume_editing(saved.id)?;
        }
        Ok(())
    }

    pub(super) fn outdent_editor(&mut self) -> vrac_engine::Result<()> {
        if self.adjust_new_draft(false)? {
            return Ok(());
        }
        let Some(saved) = self.commit_editor()? else {
            return Ok(());
        };
        self.selected = Some(saved.id);
        self.outdent_selected()?;
        if self.is_visible(saved.id) {
            self.resume_editing(saved.id)?;
        }
        Ok(())
    }

    pub(super) fn adjust_new_draft(&mut self, indent: bool) -> vrac_engine::Result<bool> {
        let Some(editor) = self.editor.as_ref() else {
            return Ok(false);
        };
        let EditTarget::New {
            parent_id,
            placement,
        } = editor.target
        else {
            return Ok(false);
        };
        let next_target = if indent {
            let previous = match placement {
                Placement::After(id) => Some(id),
                Placement::Last => self
                    .branches
                    .get(&parent_id)
                    .and_then(|branch| branch.nodes.last())
                    .map(|node| node.id),
                Placement::First | Placement::Before(_) => None,
            };
            previous.map(|previous| EditTarget::New {
                parent_id: Some(previous),
                placement: Placement::Last,
            })
        } else if parent_id == self.focus {
            None
        } else {
            match parent_id {
                Some(parent_id) => self.engine.node(parent_id)?.map(|parent| EditTarget::New {
                    parent_id: parent.parent_id,
                    placement: Placement::After(parent_id),
                }),
                None => None,
            }
        };
        let Some(next_target) = next_target else {
            self.status = if indent {
                "There is no previous sibling to indent under".into()
            } else {
                "The bullet is already at the current outline level".into()
            };
            return Ok(true);
        };
        if let EditTarget::New {
            parent_id: Some(parent_id),
            ..
        } = next_target
        {
            self.expand(parent_id)?;
        }
        self.editor.as_mut().expect("editor is active").target = next_target;
        self.status.clear();
        Ok(true)
    }

    pub(super) fn resume_editing(&mut self, id: NodeId) -> vrac_engine::Result<()> {
        let node = self
            .engine
            .node(id)?
            .ok_or(vrac_engine::Error::NodeNotFound(id))?;
        let references = node
            .references
            .iter()
            .map(|reference| ReferenceInput {
                label_start: reference.label_start,
                label_end: reference.label_end,
                target_id: reference.target_id,
            })
            .collect();
        self.editor = Some(Editor::new(
            EditTarget::Existing(id),
            node.text,
            references,
            node.tags,
        ));
        self.status.clear();
        Ok(())
    }

    pub(super) fn indent_selected(&mut self) -> vrac_engine::Result<()> {
        let Some(node) = self.selected_node() else {
            return Ok(());
        };
        if node.system.is_some() {
            self.status = "Protected Journal nodes cannot be moved".into();
            return Ok(());
        }
        let Some(branch) = self.branches.get(&node.parent_id) else {
            return Ok(());
        };
        let Some(index) = branch
            .nodes
            .iter()
            .position(|sibling| sibling.id == node.id)
        else {
            return Ok(());
        };
        if index == 0 {
            self.status = "There is no previous sibling to indent under".into();
            return Ok(());
        }
        let new_parent = branch.nodes[index - 1].id;
        let old_parent = node.parent_id;
        self.engine.move_node(
            node.id,
            Destination {
                parent_id: Some(new_parent),
                placement: Placement::Last,
            },
        )?;
        self.reload_changed_branch(old_parent)?;
        self.reload_changed_branch(Some(new_parent))?;
        self.expanded.insert(new_parent);
        if self.is_visible(node.id) {
            self.selected = Some(node.id);
            self.status = "Indented".into();
        } else {
            self.selected = Some(new_parent);
            self.status = "Indented outside the loaded page".into();
        }
        Ok(())
    }

    pub(super) fn outdent_selected(&mut self) -> vrac_engine::Result<()> {
        let Some(node) = self.selected_node() else {
            return Ok(());
        };
        if node.system.is_some() {
            self.status = "Protected Journal nodes cannot be moved".into();
            return Ok(());
        }
        let Some(parent_id) = node.parent_id else {
            self.status = "The node is already at the root".into();
            return Ok(());
        };
        let parent = self
            .engine
            .node(parent_id)?
            .ok_or(vrac_engine::Error::NodeNotFound(parent_id))?;
        let destination_parent = parent.parent_id;
        self.engine.move_node(
            node.id,
            Destination {
                parent_id: destination_parent,
                placement: Placement::After(parent_id),
            },
        )?;
        self.reload_changed_branch(Some(parent_id))?;
        self.reload_changed_branch(destination_parent)?;
        if self.focus == Some(parent_id) {
            self.set_focus(destination_parent)?;
        }
        self.selected = Some(node.id);
        self.status = "Outdented".into();
        Ok(())
    }

    pub(super) fn undo(&mut self) -> vrac_engine::Result<()> {
        if self.engine.undo()? {
            self.reload_after_history()?;
            self.status = "Undone".into();
        } else {
            self.status = "Nothing to undo".into();
        }
        Ok(())
    }

    pub(super) fn redo(&mut self) -> vrac_engine::Result<()> {
        if self.engine.redo()? {
            self.reload_after_history()?;
            self.status = "Redone".into();
        } else {
            self.status = "Nothing to redo".into();
        }
        Ok(())
    }

    pub(super) fn copy_selected(&mut self) -> vrac_engine::Result<()> {
        let Some(node) = self.selected_node() else {
            return Ok(());
        };
        let text = self.engine.copy_nodes(&[node.id])?;
        match Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text)) {
            Ok(()) => self.status = "Copied subtree".into(),
            Err(error) => self.status = format!("Clipboard error: {error}"),
        }
        Ok(())
    }

    pub(super) fn delete_selected(&mut self) -> vrac_engine::Result<()> {
        let Some(node) = self.selected_node() else {
            return Ok(());
        };
        if node.system.is_some() {
            self.status = "Protected Journal nodes cannot be deleted".into();
            return Ok(());
        }
        let before = self.visible_nodes();
        let index = before
            .iter()
            .position(|item| item.node.id == node.id)
            .unwrap_or_default();
        let text = self.engine.copy_nodes(&[node.id])?;
        if let Err(error) = Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text)) {
            self.status = format!("Delete cancelled: clipboard error: {error}");
            return Ok(());
        }

        let outcome = self.engine.delete_node(node.id)?;
        self.reload_changed_branch(node.parent_id)?;
        self.branches.remove(&Some(node.id));
        self.expanded.remove(&node.id);
        for pruned in outcome.pruned_roots {
            self.branches.remove(&Some(pruned));
            self.expanded.remove(&pruned);
        }
        if self.branches.contains_key(&None) {
            self.reload_branch(None)?;
        }
        let after = self.visible_nodes();
        self.selected = after
            .get(index.min(after.len().saturating_sub(1)))
            .map(|item| item.node.id);
        self.status = format!("Deleted {} node(s); subtree copied", outcome.deleted_nodes);
        Ok(())
    }

    pub(super) fn paste_after_selected(&mut self) -> vrac_engine::Result<()> {
        let text = match Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
            Ok(text) => text,
            Err(error) => {
                self.status = format!("Clipboard error: {error}");
                return Ok(());
            }
        };
        let (parent_id, placement) = match self.selected_node() {
            Some(node) => (node.parent_id, Placement::After(node.id)),
            None => (self.focus, Placement::Last),
        };
        let created = self.engine.paste_nodes(
            Destination {
                parent_id,
                placement,
            },
            &text,
        )?;
        self.reload_changed_branch(parent_id)?;
        if parent_id.is_some() && self.branches.contains_key(&None) {
            self.reload_branch(None)?;
        }
        self.selected = created.first().map(|node| node.id).or(parent_id);
        self.status = format!("Pasted {} subtree(s)", created.len());
        Ok(())
    }

    pub(super) fn start_new_child(&mut self) -> vrac_engine::Result<()> {
        let Some(node) = self.selected_node() else {
            return Ok(());
        };
        if node.has_children {
            self.expand(node.id)?;
        }
        self.editor = Some(Editor::empty(EditTarget::New {
            parent_id: Some(node.id),
            placement: Placement::First,
        }));
        self.status.clear();
        Ok(())
    }

    pub(super) fn commit_editor(&mut self) -> vrac_engine::Result<Option<Node>> {
        let Some(editor) = self.editor.take() else {
            return Ok(None);
        };
        let result = (|| -> vrac_engine::Result<Option<Node>> {
            match editor.target.clone() {
                EditTarget::Existing(id) => {
                    let update = self.engine.set_content(
                        id,
                        editor.text.clone(),
                        editor.references.clone(),
                    )?;
                    let updated = self
                        .engine
                        .node(id)?
                        .ok_or(vrac_engine::Error::NodeNotFound(id))?;
                    if !update.materialized_nodes.is_empty() || !update.pruned_roots.is_empty() {
                        self.reload_branch(None)?;
                    }
                    for pruned in update.pruned_roots {
                        self.branches.remove(&Some(pruned));
                        self.expanded.remove(&pruned);
                    }
                    self.update_cached_node(&updated);
                    self.status = "Saved".into();
                    Ok(Some(updated))
                }
                EditTarget::New {
                    parent_id,
                    placement,
                } => {
                    if editor.text.is_empty() {
                        self.status = "Empty node not created".into();
                        return Ok(None);
                    }
                    let mut input = CreateNode::new(editor.text.clone());
                    input.parent_id = parent_id;
                    input.placement = placement;
                    input.references = editor.references.clone();
                    input.tags = editor.tags.clone();
                    let restore_count = self.branches.get(&parent_id).map_or(0, |branch| {
                        let previous_count = branch.nodes.len();
                        let needs_next = match placement {
                            Placement::After(reference) => {
                                branch.nodes.last().is_some_and(|node| node.id == reference)
                            }
                            Placement::Last => branch.next.is_none(),
                            Placement::First | Placement::Before(_) => false,
                        };
                        previous_count + usize::from(needs_next)
                    });
                    let may_materialize_root = parent_id.is_some()
                        && editor
                            .text
                            .split_once("[[")
                            .is_some_and(|(_, rest)| rest.contains("]]"));
                    let created = self.engine.create_node(input)?;
                    self.reload_changed_branch(parent_id)?;
                    self.load_to_count(parent_id, restore_count)?;
                    if let Some(parent_id) = parent_id {
                        if may_materialize_root && self.branches.contains_key(&None) {
                            self.reload_branch(None)?;
                        }
                        self.expanded.insert(parent_id);
                    }
                    let loaded = self.branches.get(&parent_id).is_some_and(|branch| {
                        branch.nodes.iter().any(|node| node.id == created.id)
                    });
                    let fallback = match placement {
                        Placement::Before(reference) | Placement::After(reference) => {
                            Some(reference)
                        }
                        Placement::First | Placement::Last => parent_id,
                    };
                    self.selected = if loaded { Some(created.id) } else { fallback };
                    self.status = if loaded {
                        "Created".into()
                    } else {
                        "Created outside the loaded page".into()
                    };
                    Ok(Some(created))
                }
            }
        })();
        if result.is_err() {
            self.editor = Some(editor);
        }
        result
    }
}
