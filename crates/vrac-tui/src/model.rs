//! Core terminal model and loaded view state.

use std::collections::{HashMap, HashSet};

use vrac_engine::{Cursor, Engine, Node, NodeId};

use super::editor::Editor;
use super::prompts::{BacklinkView, Launcher, ReferencePrompt, TagPrompt};

#[derive(Clone)]
pub(super) struct Branch {
    pub(super) nodes: Vec<Node>,
    pub(super) next: Option<Cursor>,
}

#[derive(Clone)]
pub(super) struct VisibleNode {
    pub(super) node: Node,
    pub(super) depth: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Action {
    Continue,
    Sync,
    ChooseWorkspace,
    SetLines(bool),
    Quit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SessionExit {
    Quit,
    ChooseWorkspace,
}

pub(super) struct App {
    pub(super) engine: Engine,
    pub(super) branches: HashMap<Option<NodeId>, Branch>,
    pub(super) expanded: HashSet<NodeId>,
    pub(super) focus: Option<NodeId>,
    pub(super) focus_path: Vec<Node>,
    pub(super) selected: Option<NodeId>,
    pub(super) editor: Option<Editor>,
    pub(super) launcher: Option<Launcher>,
    pub(super) tag_prompt: Option<TagPrompt>,
    pub(super) backlinks: Option<BacklinkView>,
    pub(super) reference_prompt: Option<ReferencePrompt>,
    pub(super) help: bool,
    pub(super) pending_key: Option<char>,
    pub(super) status: String,
    pub(super) lines: bool,
    pub(super) scroll: usize,
    pub(super) viewport_width: usize,
}

impl App {
    pub(super) fn open_with_lines(mut engine: Engine, lines: bool) -> vrac_engine::Result<Self> {
        let today = jiff::Zoned::now().date().to_string();
        let day = engine.journal_day(&today)?;
        Self::open_with_focus_and_lines(engine, Some(day.id), lines)
    }

    pub(super) fn open_with_focus(
        engine: Engine,
        focus: Option<NodeId>,
    ) -> vrac_engine::Result<Self> {
        Self::open_with_focus_and_lines(engine, focus, true)
    }

    pub(super) fn open_with_focus_and_lines(
        engine: Engine,
        focus: Option<NodeId>,
        lines: bool,
    ) -> vrac_engine::Result<Self> {
        let mut app = Self {
            engine,
            branches: HashMap::new(),
            expanded: HashSet::new(),
            focus,
            focus_path: Vec::new(),
            selected: None,
            editor: None,
            launcher: None,
            tag_prompt: None,
            backlinks: None,
            reference_prompt: None,
            help: false,
            pending_key: None,
            status: String::new(),
            lines,
            scroll: 0,
            viewport_width: 80,
        };
        app.reload_branch(focus)?;
        app.focus_path = match focus {
            Some(id) => app.engine.path(id)?,
            None => Vec::new(),
        };
        app.selected = app
            .branches
            .get(&focus)
            .and_then(|branch| branch.nodes.first())
            .map(|node| node.id);
        Ok(app)
    }

    pub(super) fn focus_label(&self) -> String {
        if self.focus_path.is_empty() {
            return "root".into();
        }
        let mut label = String::from("root");
        for node in &self.focus_path {
            label.push_str(" › ");
            label.push_str(&node.text.replace('\n', " "));
        }
        label
    }

    pub(super) fn visible_nodes(&self) -> Vec<VisibleNode> {
        let Some(root) = self.branches.get(&self.focus) else {
            return Vec::new();
        };
        let mut stack: Vec<_> = root
            .nodes
            .iter()
            .cloned()
            .rev()
            .map(|node| (node, 0))
            .collect();
        let mut visible = Vec::new();

        while let Some((node, depth)) = stack.pop() {
            let id = node.id;
            visible.push(VisibleNode { node, depth });
            if !self.expanded.contains(&id) {
                continue;
            }
            if let Some(branch) = self.branches.get(&Some(id)) {
                stack.extend(
                    branch
                        .nodes
                        .iter()
                        .cloned()
                        .rev()
                        .map(|child| (child, depth + 1)),
                );
            }
        }

        visible
    }

    pub(super) fn selected_node(&self) -> Option<Node> {
        let selected = self.selected?;
        self.branches
            .values()
            .flat_map(|branch| branch.nodes.iter())
            .find(|node| node.id == selected)
            .cloned()
    }

    pub(super) fn is_visible(&self, id: NodeId) -> bool {
        self.visible_nodes().iter().any(|item| item.node.id == id)
    }
}
