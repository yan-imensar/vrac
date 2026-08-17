//! Static command catalog exposed by the `:` launcher.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Command {
    New,
    NewBefore,
    NewChild,
    Zoom,
    ZoomOut,
    Today,
    Root,
    FocusParent,
    FocusChild,
    Toggle,
    Indent,
    Outdent,
    Delete,
    Copy,
    Paste,
    Undo,
    Redo,
    Tag,
    Backlinks,
    BacklinksOn,
    BacklinksOff,
    LinesOn,
    LinesOff,
    Sync,
    Workspace,
    Quit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CommandEntry {
    pub(super) command: Command,
    pub(super) name: &'static str,
    pub(super) hint: &'static str,
}

pub(super) const COMMANDS: &[CommandEntry] = &[
    CommandEntry {
        command: Command::New,
        name: "new",
        hint: "create a sibling after the selected bullet",
    },
    CommandEntry {
        command: Command::NewBefore,
        name: "new_before",
        hint: "create a sibling before the selected bullet",
    },
    CommandEntry {
        command: Command::NewChild,
        name: "new_child",
        hint: "create a child under the selected bullet",
    },
    CommandEntry {
        command: Command::Zoom,
        name: "zoom",
        hint: "focus the selected bullet",
    },
    CommandEntry {
        command: Command::ZoomOut,
        name: "zoom_out",
        hint: "return to the parent view",
    },
    CommandEntry {
        command: Command::Today,
        name: "today",
        hint: "open today's Journal page",
    },
    CommandEntry {
        command: Command::Root,
        name: "root",
        hint: "open the workspace root",
    },
    CommandEntry {
        command: Command::FocusParent,
        name: "focus_parent",
        hint: "collapse or select the parent bullet",
    },
    CommandEntry {
        command: Command::FocusChild,
        name: "focus_child",
        hint: "expand or select the first child",
    },
    CommandEntry {
        command: Command::Toggle,
        name: "toggle",
        hint: "expand or collapse the selected bullet",
    },
    CommandEntry {
        command: Command::Indent,
        name: "indent",
        hint: "move the bullet under its previous sibling",
    },
    CommandEntry {
        command: Command::Outdent,
        name: "outdent",
        hint: "move the bullet after its parent",
    },
    CommandEntry {
        command: Command::Delete,
        name: "delete",
        hint: "copy and delete the selected subtree",
    },
    CommandEntry {
        command: Command::Copy,
        name: "copy",
        hint: "copy the selected subtree",
    },
    CommandEntry {
        command: Command::Paste,
        name: "paste",
        hint: "paste after the selected bullet",
    },
    CommandEntry {
        command: Command::Undo,
        name: "undo",
        hint: "undo the latest change",
    },
    CommandEntry {
        command: Command::Redo,
        name: "redo",
        hint: "redo the latest undone change",
    },
    CommandEntry {
        command: Command::Tag,
        name: "tag",
        hint: "open tag completion for the selected bullet",
    },
    CommandEntry {
        command: Command::Backlinks,
        name: "backlinks",
        hint: "jump to contextual backlinks",
    },
    CommandEntry {
        command: Command::BacklinksOn,
        name: "backlinks on",
        hint: "always show contextual backlinks",
    },
    CommandEntry {
        command: Command::BacklinksOff,
        name: "backlinks off",
        hint: "hide contextual backlinks until requested",
    },
    CommandEntry {
        command: Command::LinesOn,
        name: "lines on",
        hint: "show hierarchy lines",
    },
    CommandEntry {
        command: Command::LinesOff,
        name: "lines off",
        hint: "hide hierarchy lines",
    },
    CommandEntry {
        command: Command::Sync,
        name: "sync",
        hint: "synchronize the current workspace now",
    },
    CommandEntry {
        command: Command::Workspace,
        name: "workspace",
        hint: "choose or create another workspace folder",
    },
    CommandEntry {
        command: Command::Quit,
        name: "quit",
        hint: "close Vrac TUI",
    },
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::COMMANDS;

    #[test]
    fn command_catalog_has_unique_documented_entries() {
        let mut names = HashSet::new();

        for entry in COMMANDS {
            assert!(
                names.insert(entry.name),
                "duplicate command: {}",
                entry.name
            );
            assert!(!entry.hint.trim().is_empty(), "{} has no hint", entry.name);
        }
    }
}
