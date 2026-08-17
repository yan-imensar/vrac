//! Selection, expansion, paging, and zoom navigation.

use vrac_engine::NodeId;

use super::model::App;

impl App {
    pub(super) fn set_focus(&mut self, focus: Option<NodeId>) -> vrac_engine::Result<()> {
        if !self.branches.contains_key(&focus) {
            self.reload_branch(focus)?;
        }
        self.focus = focus;
        self.focus_path = match focus {
            Some(id) => self.engine.path(id)?,
            None => Vec::new(),
        };
        self.selected = self
            .branches
            .get(&focus)
            .and_then(|branch| branch.nodes.first())
            .map(|node| node.id);
        self.scroll = 0;
        Ok(())
    }

    pub(super) fn zoom_selected(&mut self) -> vrac_engine::Result<()> {
        let Some(node) = self.selected_node() else {
            return Ok(());
        };
        self.set_focus(Some(node.id))
    }

    pub(super) fn zoom_out(&mut self) -> vrac_engine::Result<()> {
        let Some(current) = self.focus else {
            return Ok(());
        };
        let current_node = self
            .engine
            .node(current)?
            .ok_or(vrac_engine::Error::NodeNotFound(current))?;
        self.set_focus(current_node.parent_id)?;
        self.selected = Some(current);
        Ok(())
    }

    pub(super) fn move_selection(&mut self, direction: isize) -> vrac_engine::Result<()> {
        if direction > 0 {
            self.load_next_page_at_selection()?;
        }
        let visible = self.visible_nodes();
        if visible.is_empty() {
            self.selected = None;
            return Ok(());
        }
        let current = self
            .selected
            .and_then(|id| visible.iter().position(|item| item.node.id == id))
            .unwrap_or(0);
        let next = current
            .saturating_add_signed(direction)
            .min(visible.len().saturating_sub(1));
        self.selected = Some(visible[next].node.id);
        Ok(())
    }

    pub(super) fn move_selection_page(&mut self, direction: isize) -> vrac_engine::Result<()> {
        let step = direction.signum();
        for _ in 0..direction.unsigned_abs() {
            let before = self.selected;
            self.move_selection(step)?;
            if self.selected == before {
                break;
            }
        }
        Ok(())
    }

    pub(super) fn load_next_page_at_selection(&mut self) -> vrac_engine::Result<()> {
        let Some(node) = self.selected_node() else {
            return Ok(());
        };
        let parent_id = node.parent_id;
        let at_loaded_end = self
            .branches
            .get(&parent_id)
            .is_some_and(|branch| branch.nodes.last().is_some_and(|last| last.id == node.id));
        if at_loaded_end && self.load_more(parent_id)? {
            self.status = "Loaded 100 more siblings".into();
        }
        Ok(())
    }

    pub(super) fn select_edge(&mut self, last: bool) {
        let visible = self.visible_nodes();
        self.selected = if last {
            visible.last().map(|item| item.node.id)
        } else {
            visible.first().map(|item| item.node.id)
        };
    }

    pub(super) fn move_right(&mut self) -> vrac_engine::Result<()> {
        let Some(node) = self.selected_node() else {
            return Ok(());
        };
        if !node.has_children {
            return Ok(());
        }
        if !self.expanded.contains(&node.id) {
            self.expand(node.id)?;
        }
        if let Some(child) = self
            .branches
            .get(&Some(node.id))
            .and_then(|branch| branch.nodes.first())
        {
            self.selected = Some(child.id);
        }
        Ok(())
    }

    pub(super) fn move_left(&mut self) -> vrac_engine::Result<()> {
        let Some(node) = self.selected_node() else {
            return Ok(());
        };
        if self.expanded.remove(&node.id) {
            return Ok(());
        }
        if node.parent_id != self.focus {
            let parent_id = node
                .parent_id
                .expect("a visible descendant below the focus has a parent");
            self.selected = Some(parent_id);
        }
        Ok(())
    }

    pub(super) fn toggle_selected(&mut self) -> vrac_engine::Result<()> {
        let Some(node) = self.selected_node() else {
            return Ok(());
        };
        if !node.has_children {
            return Ok(());
        }
        if !self.expanded.remove(&node.id) {
            self.expand(node.id)?;
        }
        Ok(())
    }

    pub(super) fn expand(&mut self, id: NodeId) -> vrac_engine::Result<()> {
        if !self.branches.contains_key(&Some(id)) {
            self.reload_branch(Some(id))?;
        }
        self.expanded.insert(id);
        Ok(())
    }
}
