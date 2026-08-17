//! Bounded model reloads and cache refresh after external changes.

use std::collections::HashSet;

use vrac_engine::{Node, NodeId, Page};

use super::commands::COMMANDS;
use super::model::{App, Branch};
use super::prompts::{BacklinkView, LauncherItem, LauncherKind, TagTarget};

impl App {
    pub(super) fn reload_branch(&mut self, parent_id: Option<NodeId>) -> vrac_engine::Result<()> {
        let page = self.engine.children(parent_id, Page::default())?;
        for node in &page.nodes {
            if !node.has_children {
                self.expanded.remove(&node.id);
                self.branches.remove(&Some(node.id));
            }
        }
        self.branches.insert(
            parent_id,
            Branch {
                nodes: page.nodes,
                next: page.next,
            },
        );
        Ok(())
    }

    pub(super) fn reload_changed_branch(
        &mut self,
        parent_id: Option<NodeId>,
    ) -> vrac_engine::Result<()> {
        self.reload_branch(parent_id)?;
        if let Some(parent_id) = parent_id {
            self.refresh_cached_node(parent_id)?;
        }
        Ok(())
    }

    pub(super) fn load_more(&mut self, parent_id: Option<NodeId>) -> vrac_engine::Result<bool> {
        let Some(after) = self.branches.get(&parent_id).and_then(|branch| branch.next) else {
            return Ok(false);
        };
        let page = self.engine.children(
            parent_id,
            Page {
                limit: Page::default().limit,
                after: Some(after),
            },
        )?;
        let branch = self
            .branches
            .get_mut(&parent_id)
            .expect("the paginated branch is loaded");
        branch.nodes.extend(page.nodes);
        branch.next = page.next;
        Ok(true)
    }

    pub(super) fn load_to_count(
        &mut self,
        parent_id: Option<NodeId>,
        previous_count: usize,
    ) -> vrac_engine::Result<()> {
        while self
            .branches
            .get(&parent_id)
            .is_some_and(|branch| branch.nodes.len() < previous_count)
            && self.load_more(parent_id)?
        {}
        Ok(())
    }

    pub(super) fn refresh_launcher(&mut self) -> vrac_engine::Result<()> {
        let launcher = self.launcher.as_ref().expect("launcher is active");
        let kind = launcher.kind;
        let query = launcher.text.clone();
        let normalized = query.trim().to_lowercase();
        let items = match kind {
            LauncherKind::Commands => COMMANDS
                .iter()
                .filter(|entry| {
                    normalized.is_empty()
                        || entry.name.contains(&normalized)
                        || entry.hint.contains(&normalized)
                })
                .copied()
                .map(LauncherItem::Command)
                .collect(),
            LauncherKind::Search if normalized.chars().count() >= 2 => self
                .engine
                .search(&query, 20)?
                .into_iter()
                .map(LauncherItem::Node)
                .collect(),
            LauncherKind::Search => Vec::new(),
        };
        let launcher = self.launcher.as_mut().expect("launcher is active");
        launcher.items = items;
        launcher.selected = 0;
        self.status.clear();
        Ok(())
    }

    pub(super) fn refresh_tag_prompt(&mut self) -> vrac_engine::Result<()> {
        let query = self
            .tag_prompt
            .as_ref()
            .expect("tag prompt is active")
            .query
            .clone();
        let mut results = self.engine.tags(&query, 20)?;
        let typed = query.trim();
        if !typed.is_empty() && !results.iter().any(|tag| tag == typed) {
            results.insert(0, typed.into());
        }
        let prompt = self.tag_prompt.as_mut().expect("tag prompt is active");
        prompt.results = results;
        prompt.selected = prompt.selected.min(prompt.results.len().saturating_sub(1));
        self.status.clear();
        Ok(())
    }

    pub(super) fn refresh_backlinks(&mut self) -> vrac_engine::Result<()> {
        let Some(view) = self.backlinks.as_ref() else {
            return Ok(());
        };
        let target_id = view.target_id;
        let selected = view.selected;
        let filter = view.filter.clone();
        let temporary = view.temporary;
        if self.engine.node(target_id)?.is_none() {
            self.backlinks = None;
            return Ok(());
        }
        let tags = self.engine.backlink_tags(target_id, 100)?;
        let page = self
            .engine
            .backlinks(target_id, filter.as_deref(), Page::default())?;
        let contexts = page
            .contexts
            .into_iter()
            .map(|context| context.path)
            .collect::<Vec<_>>();
        self.backlinks = Some(BacklinkView {
            target_id,
            tags,
            filter,
            selected: selected
                .filter(|_| !contexts.is_empty())
                .map(|selected| selected.min(contexts.len() - 1)),
            contexts,
            next: page.next,
            temporary,
        });
        Ok(())
    }

    pub(super) fn refresh_contextual_backlinks(
        &mut self,
        temporary: bool,
    ) -> vrac_engine::Result<()> {
        let target_id = if temporary {
            self.focus.or(self.selected)
        } else if self.backlinks_visible {
            self.focus
        } else {
            None
        };
        let Some(target_id) = target_id else {
            self.backlinks = None;
            self.backlink_filter = None;
            return Ok(());
        };
        self.backlinks = Some(BacklinkView {
            target_id,
            tags: Vec::new(),
            filter: None,
            contexts: Vec::new(),
            next: None,
            selected: None,
            temporary,
        });
        self.refresh_backlinks()
    }

    pub(super) fn set_backlinks_visible(&mut self, enabled: bool) -> vrac_engine::Result<()> {
        self.backlinks_visible = enabled;
        self.backlink_filter = None;
        self.refresh_contextual_backlinks(false)
    }

    pub(super) fn refresh_backlink_filter(&mut self) {
        let Some(prompt) = self.backlink_filter.as_ref() else {
            return;
        };
        let normalized = prompt.query.trim().trim_start_matches('#').to_lowercase();
        let results = self
            .backlinks
            .as_ref()
            .into_iter()
            .flat_map(|view| &view.tags)
            .filter(|tag| normalized.is_empty() || tag.tag.contains(&normalized))
            .cloned()
            .collect();
        let prompt = self
            .backlink_filter
            .as_mut()
            .expect("backlink filter is active");
        prompt.results = results;
        prompt.selected = prompt.selected.min(prompt.results.len());
    }

    pub(super) fn load_more_backlinks_if_needed(&mut self) -> vrac_engine::Result<()> {
        let Some(view) = self.backlinks.as_ref() else {
            return Ok(());
        };
        let Some(selected) = view.selected else {
            return Ok(());
        };
        if selected + 1 < view.contexts.len() {
            return Ok(());
        }
        let Some(after) = view.next else {
            return Ok(());
        };
        let target_id = view.target_id;
        let filter = view.filter.clone();
        let page = self.engine.backlinks(
            target_id,
            filter.as_deref(),
            Page {
                limit: Page::default().limit,
                after: Some(after),
            },
        )?;
        let view = self.backlinks.as_mut().expect("backlinks are active");
        view.contexts
            .extend(page.contexts.into_iter().map(|context| context.path));
        view.next = page.next;
        Ok(())
    }

    pub(super) fn refresh_reference_prompt(&mut self) -> vrac_engine::Result<()> {
        let query = self
            .reference_prompt
            .as_ref()
            .expect("reference prompt is active")
            .query
            .clone();
        let results = self.engine.search(&query, 8)?;
        let prompt = self
            .reference_prompt
            .as_mut()
            .expect("reference prompt is active");
        prompt.results = results;
        prompt.selected = prompt.selected.min(prompt.results.len().saturating_sub(1));
        self.status.clear();
        Ok(())
    }

    pub(super) fn reload_after_history(&mut self) -> vrac_engine::Result<()> {
        let focus = match self.focus {
            Some(id) if self.engine.node(id)?.is_some() => Some(id),
            _ => None,
        };
        self.branches.clear();
        self.expanded.clear();
        self.set_focus(focus)
    }

    pub(super) fn reload_after_sync(&mut self) -> vrac_engine::Result<()> {
        let selected = self.selected;
        let backlinks = self.backlinks.clone();
        let open_branches = self
            .visible_nodes()
            .into_iter()
            .filter(|visible| self.expanded.contains(&visible.node.id))
            .map(|visible| {
                let loaded = self
                    .branches
                    .get(&Some(visible.node.id))
                    .map_or(0, |branch| branch.nodes.len());
                (visible.node.id, loaded)
            })
            .collect::<Vec<_>>();
        let focus = match self.focus {
            Some(id) if self.engine.node(id)?.is_some() => Some(id),
            _ => None,
        };
        let focused_count = self
            .branches
            .get(&self.focus)
            .map_or(0, |branch| branch.nodes.len());
        self.branches.clear();
        self.expanded.clear();
        self.set_focus(focus)?;
        self.load_to_count(focus, focused_count)?;

        let mut reachable = self
            .branches
            .get(&focus)
            .into_iter()
            .flat_map(|branch| branch.nodes.iter().map(|node| node.id))
            .collect::<HashSet<_>>();
        for (id, previous_count) in open_branches {
            if !reachable.contains(&id) {
                continue;
            }
            let has_children = self
                .branches
                .values()
                .flat_map(|branch| &branch.nodes)
                .find(|node| node.id == id)
                .is_some_and(|node| node.has_children);
            if !has_children {
                continue;
            }
            self.reload_branch(Some(id))?;
            self.load_to_count(Some(id), previous_count)?;
            if let Some(branch) = self.branches.get(&Some(id)) {
                reachable.extend(branch.nodes.iter().map(|node| node.id));
            }
            self.expanded.insert(id);
        }
        if selected.is_some_and(|id| {
            self.visible_nodes()
                .iter()
                .any(|visible| visible.node.id == id)
        }) {
            self.selected = selected;
        }
        if let Some(view) = backlinks
            && self.engine.node(view.target_id)?.is_some()
            && (view.temporary || self.focus == Some(view.target_id))
        {
            self.backlinks = Some(view);
        }
        if self.launcher.is_some() {
            self.refresh_launcher()?;
        }
        if let Some(TagTarget::Node(id)) = self.tag_prompt.as_ref().map(|prompt| prompt.target)
            && self.engine.node(id)?.is_none()
        {
            self.tag_prompt = None;
        } else if self.tag_prompt.is_some() {
            self.refresh_tag_prompt()?;
        }
        if self.backlinks.is_some() {
            self.refresh_backlinks()?;
        }
        Ok(())
    }

    pub(super) fn refresh_cached_node(&mut self, id: NodeId) -> vrac_engine::Result<()> {
        let Some(updated) = self.engine.node(id)? else {
            return Ok(());
        };
        self.update_cached_node(&updated);
        Ok(())
    }

    pub(super) fn update_cached_node(&mut self, updated: &Node) {
        for node in &mut self.focus_path {
            if node.id == updated.id {
                *node = updated.clone();
            }
        }
        for branch in self.branches.values_mut() {
            for node in &mut branch.nodes {
                if node.id == updated.id {
                    *node = updated.clone();
                }
                for reference in &mut node.references {
                    if reference.target_id == updated.id {
                        reference.target_text.clone_from(&updated.text);
                    }
                }
            }
        }
        if let Some(view) = &mut self.backlinks {
            for path in &mut view.contexts {
                for node in path {
                    if node.id == updated.id {
                        *node = updated.clone();
                    }
                    for reference in &mut node.references {
                        if reference.target_id == updated.id {
                            reference.target_text.clone_from(&updated.text);
                        }
                    }
                }
            }
        }
        if !updated.has_children {
            self.expanded.remove(&updated.id);
            self.branches.remove(&Some(updated.id));
        }
    }
}
