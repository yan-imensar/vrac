//! Transient state for bounded launchers and completion prompts.

use vrac_engine::{BacklinkTag, Cursor, Node, NodeId};

use super::commands::CommandEntry;
use super::editor::char_to_byte;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum LauncherItem {
    Command(CommandEntry),
    Node(Node),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LauncherKind {
    Search,
    Commands,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Launcher {
    pub(super) kind: LauncherKind,
    pub(super) text: String,
    pub(super) cursor: usize,
    pub(super) items: Vec<LauncherItem>,
    pub(super) selected: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TagPrompt {
    pub(super) target: TagTarget,
    pub(super) query: String,
    pub(super) results: Vec<String>,
    pub(super) selected: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TagTarget {
    Node(NodeId),
    Draft,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct BacklinkView {
    pub(super) target_id: NodeId,
    pub(super) tags: Vec<BacklinkTag>,
    pub(super) filter: Option<String>,
    pub(super) contexts: Vec<Vec<Node>>,
    pub(super) next: Option<Cursor>,
    pub(super) selected: Option<usize>,
    pub(super) temporary: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct BacklinkFilterPrompt {
    pub(super) query: String,
    pub(super) results: Vec<BacklinkTag>,
    pub(super) selected: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ReferencePrompt {
    pub(super) query: String,
    pub(super) results: Vec<Node>,
    pub(super) selected: usize,
}

impl Launcher {
    pub(super) fn new(kind: LauncherKind) -> Self {
        Self {
            kind,
            text: String::new(),
            cursor: 0,
            items: Vec::new(),
            selected: 0,
        }
    }

    pub(super) fn insert(&mut self, character: char) {
        let byte = char_to_byte(&self.text, self.cursor);
        self.text.insert(byte, character);
        self.cursor += 1;
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = char_to_byte(&self.text, self.cursor - 1);
        let end = char_to_byte(&self.text, self.cursor);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub(super) fn delete(&mut self) {
        if self.cursor == self.text.chars().count() {
            return;
        }
        let start = char_to_byte(&self.text, self.cursor);
        let end = char_to_byte(&self.text, self.cursor + 1);
        self.text.replace_range(start..end, "");
    }
}
