use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::process::ExitCode;

use arboard::Clipboard;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::style::{Attribute, ResetColor, SetAttribute};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use vrac::{
    CreateNode, Cursor, Destination, Engine, Node, NodeId, Page, Placement, ReferenceInput,
};

mod ui;

use ui::draw;
#[cfg(test)]
use ui::{display_lines, wrap_text};

const USAGE: &str = "Usage: vrac-tui <workspace.vrac>";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let Some(argument) = arguments.next() else {
        return Err(USAGE.into());
    };
    if argument == "--help" || argument == "-h" {
        println!("{USAGE}");
        return Ok(());
    }
    if arguments.next().is_some() {
        return Err(USAGE.into());
    }

    let path = PathBuf::from(argument);
    let mut app = App::open(Engine::open(&path)?)?;
    let mut terminal = TerminalGuard::enter()?;

    loop {
        draw(terminal.stdout(), &mut app, &path)?;
        match event::read()? {
            Event::Key(key) if actionable_key(key.kind) => match app.handle_key(key) {
                Ok(Action::Quit) => break,
                Ok(Action::Continue) => {}
                Err(error) => app.status = error.to_string(),
            },
            Event::Resize(_, _) => {}
            _ => continue,
        }
    }

    Ok(())
}

fn actionable_key(kind: KeyEventKind) -> bool {
    matches!(kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

struct TerminalGuard {
    stdout: Stdout,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error);
        }
        Ok(Self { stdout })
    }

    fn stdout(&mut self) -> &mut Stdout {
        &mut self.stdout
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            self.stdout,
            ResetColor,
            SetAttribute(Attribute::Reset),
            Show
        );
        let _ = execute!(self.stdout, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

#[derive(Clone)]
struct Branch {
    nodes: Vec<Node>,
    next: Option<Cursor>,
}

#[derive(Clone)]
struct VisibleNode {
    node: Node,
    depth: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Continue,
    Quit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum EditTarget {
    Existing(NodeId),
    New {
        parent_id: Option<NodeId>,
        placement: Placement,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Editor {
    target: EditTarget,
    text: String,
    cursor: usize,
    references: Vec<ReferenceInput>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
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
    Quit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CommandEntry {
    command: Command,
    name: &'static str,
    hint: &'static str,
}

const COMMANDS: &[CommandEntry] = &[
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
        hint: "show references to the selected bullet",
    },
    CommandEntry {
        command: Command::Quit,
        name: "quit",
        hint: "close Vrac TUI",
    },
];

#[derive(Clone, Debug, Eq, PartialEq)]
enum LauncherItem {
    Command(CommandEntry),
    Node(Node),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Search {
    text: String,
    cursor: usize,
    items: Vec<LauncherItem>,
    selected: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TagPrompt {
    target_id: NodeId,
    query: String,
    results: Vec<String>,
    selected: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BacklinkView {
    target_id: NodeId,
    contexts: Vec<Vec<Node>>,
    next: Option<Cursor>,
    selected: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReferencePrompt {
    query: String,
    results: Vec<Node>,
    selected: usize,
}

impl Search {
    fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            items: Vec::new(),
            selected: 0,
        }
    }

    fn insert(&mut self, character: char) {
        let byte = char_to_byte(&self.text, self.cursor);
        self.text.insert(byte, character);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = char_to_byte(&self.text, self.cursor - 1);
        let end = char_to_byte(&self.text, self.cursor);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn delete(&mut self) {
        if self.cursor == self.text.chars().count() {
            return;
        }
        let start = char_to_byte(&self.text, self.cursor);
        let end = char_to_byte(&self.text, self.cursor + 1);
        self.text.replace_range(start..end, "");
    }
}

impl Editor {
    fn new(target: EditTarget, text: String, references: Vec<ReferenceInput>) -> Self {
        let cursor = text.chars().count();
        Self {
            target,
            text,
            cursor,
            references,
        }
    }

    fn insert(&mut self, character: char) {
        let byte = char_to_byte(&self.text, self.cursor);
        let added = character.len_utf8();
        self.references.retain_mut(|reference| {
            let token_start = reference.label_start.saturating_sub(2);
            let token_end = reference.label_end.saturating_add(2);
            if byte > token_start && byte < token_end {
                return false;
            }
            if byte <= token_start {
                reference.label_start += added;
                reference.label_end += added;
            }
            true
        });
        self.text.insert(byte, character);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = char_to_byte(&self.text, self.cursor - 1);
        let end = char_to_byte(&self.text, self.cursor);
        self.remove_range(start, end);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn delete(&mut self) {
        if self.cursor == self.text.chars().count() {
            return;
        }
        let start = char_to_byte(&self.text, self.cursor);
        let end = char_to_byte(&self.text, self.cursor + 1);
        self.remove_range(start, end);
        self.text.replace_range(start..end, "");
    }

    fn remove_range(&mut self, start: usize, end: usize) {
        let removed = end - start;
        self.references.retain_mut(|reference| {
            let token_start = reference.label_start.saturating_sub(2);
            let token_end = reference.label_end.saturating_add(2);
            if start < token_end && end > token_start {
                return false;
            }
            if end <= token_start {
                reference.label_start -= removed;
                reference.label_end -= removed;
            }
            true
        });
    }
}

struct App {
    engine: Engine,
    branches: HashMap<Option<NodeId>, Branch>,
    expanded: HashSet<NodeId>,
    focus: Option<NodeId>,
    focus_path: Vec<Node>,
    selected: Option<NodeId>,
    editor: Option<Editor>,
    search: Option<Search>,
    tag_prompt: Option<TagPrompt>,
    backlinks: Option<BacklinkView>,
    reference_prompt: Option<ReferencePrompt>,
    pending_key: Option<char>,
    status: String,
    scroll: usize,
}

impl App {
    fn open(mut engine: Engine) -> vrac::Result<Self> {
        let today = jiff::Zoned::now().date().to_string();
        let day = engine.journal_day(&today)?;
        Self::open_with_focus(engine, Some(day.id))
    }

    fn open_with_focus(engine: Engine, focus: Option<NodeId>) -> vrac::Result<Self> {
        let mut app = Self {
            engine,
            branches: HashMap::new(),
            expanded: HashSet::new(),
            focus,
            focus_path: Vec::new(),
            selected: None,
            editor: None,
            search: None,
            tag_prompt: None,
            backlinks: None,
            reference_prompt: None,
            pending_key: None,
            status: String::new(),
            scroll: 0,
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

    fn reload_branch(&mut self, parent_id: Option<NodeId>) -> vrac::Result<()> {
        let page = self.engine.children(parent_id, Page::default())?;
        self.branches.insert(
            parent_id,
            Branch {
                nodes: page.nodes,
                next: page.next,
            },
        );
        Ok(())
    }

    fn load_more(&mut self, parent_id: Option<NodeId>) -> vrac::Result<bool> {
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

    fn focus_label(&self) -> String {
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

    fn set_focus(&mut self, focus: Option<NodeId>) -> vrac::Result<()> {
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

    fn zoom_selected(&mut self) -> vrac::Result<()> {
        let Some(node) = self.selected_node() else {
            return Ok(());
        };
        self.set_focus(Some(node.id))
    }

    fn zoom_out(&mut self) -> vrac::Result<()> {
        let Some(current) = self.focus else {
            return Ok(());
        };
        let current_node = self
            .engine
            .node(current)?
            .ok_or(vrac::Error::NodeNotFound(current))?;
        self.set_focus(current_node.parent_id)?;
        self.selected = Some(current);
        Ok(())
    }

    fn visible_nodes(&self) -> Vec<VisibleNode> {
        let Some(root) = self.branches.get(&self.focus) else {
            return Vec::new();
        };
        let mut stack: Vec<_> = root
            .nodes
            .iter()
            .rev()
            .cloned()
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
                        .rev()
                        .cloned()
                        .map(|child| (child, depth + 1)),
                );
            }
        }

        visible
    }

    fn selected_node(&self) -> Option<Node> {
        let selected = self.selected?;
        self.branches
            .values()
            .flat_map(|branch| branch.nodes.iter())
            .find(|node| node.id == selected)
            .cloned()
    }

    fn handle_key(&mut self, key: KeyEvent) -> vrac::Result<Action> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(Action::Quit);
        }
        if self.reference_prompt.is_some() {
            self.handle_reference_key(key)
        } else if self.backlinks.is_some() {
            self.handle_backlink_key(key)
        } else if self.tag_prompt.is_some() {
            self.handle_tag_key(key)
        } else if self.search.is_some() {
            self.handle_search_key(key)
        } else if self.editor.is_some() {
            self.handle_editor_key(key)
        } else {
            self.handle_normal_key(key)
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> vrac::Result<Action> {
        if let Some(pending) = self.pending_key.take()
            && key.code == KeyCode::Char(pending)
        {
            match pending {
                'y' => self.copy_selected()?,
                'd' => self.delete_selected()?,
                _ => unreachable!("only known prefixes are retained"),
            }
            return Ok(Action::Continue);
        }
        match key.code {
            KeyCode::Char('q') => return Ok(Action::Quit),
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1)?,
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1)?,
            KeyCode::PageDown => self.move_selection_page(10)?,
            KeyCode::PageUp => self.move_selection_page(-10)?,
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_selection_page(10)?
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_selection_page(-10)?
            }
            KeyCode::Home | KeyCode::Char('g') => self.select_edge(false),
            KeyCode::End | KeyCode::Char('G') => self.select_edge(true),
            KeyCode::Char('l') | KeyCode::Right => self.move_right()?,
            KeyCode::Char('h') | KeyCode::Left => self.move_left()?,
            KeyCode::Char(' ') => self.toggle_selected()?,
            KeyCode::Enter => self.zoom_selected()?,
            KeyCode::Backspace | KeyCode::Char('-') => self.zoom_out()?,
            KeyCode::Char('/') | KeyCode::Char(':') => self.start_search()?,
            KeyCode::Char('#') => self.start_tag_prompt()?,
            KeyCode::Char('b') => self.start_backlinks()?,
            KeyCode::Char('i') | KeyCode::Char('I') => self.start_edit_at_start(),
            KeyCode::Char('a') | KeyCode::Char('A') => self.start_edit(),
            KeyCode::Char('o') => self.start_new_sibling(),
            KeyCode::Char('O') => self.start_new_before(),
            KeyCode::Char('c') => self.start_new_child(),
            KeyCode::Char('y') => {
                self.pending_key = Some('y');
                self.status = "y".into();
            }
            KeyCode::Char('d') => {
                self.pending_key = Some('d');
                self.status = "d".into();
            }
            KeyCode::Char('p') => self.paste_after_selected()?,
            KeyCode::Tab => self.indent_selected()?,
            KeyCode::BackTab => self.outdent_selected()?,
            KeyCode::Char('u') => self.undo()?,
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => self.redo()?,
            _ => {}
        }
        Ok(Action::Continue)
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> vrac::Result<Action> {
        match key.code {
            KeyCode::Esc => {
                self.search = None;
                self.status.clear();
                self.scroll = 0;
            }
            KeyCode::Enter => return self.commit_launcher(),
            KeyCode::Up => {
                let search = self.search.as_mut().expect("search is active");
                search.selected = search.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                let search = self.search.as_mut().expect("search is active");
                search.selected = (search.selected + 1).min(search.items.len().saturating_sub(1));
            }
            KeyCode::Left => {
                let search = self.search.as_mut().expect("search is active");
                search.cursor = search.cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                let search = self.search.as_mut().expect("search is active");
                search.cursor = (search.cursor + 1).min(search.text.chars().count());
            }
            KeyCode::Home => self.search.as_mut().expect("search is active").cursor = 0,
            KeyCode::End => {
                let search = self.search.as_mut().expect("search is active");
                search.cursor = search.text.chars().count();
            }
            KeyCode::Backspace => {
                self.search.as_mut().expect("search is active").backspace();
                self.refresh_search()?;
            }
            KeyCode::Delete => {
                self.search.as_mut().expect("search is active").delete();
                self.refresh_search()?;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.search
                    .as_mut()
                    .expect("search is active")
                    .insert(character);
                self.refresh_search()?;
            }
            _ => {}
        }
        Ok(Action::Continue)
    }

    fn handle_tag_key(&mut self, key: KeyEvent) -> vrac::Result<Action> {
        match key.code {
            KeyCode::Esc => {
                self.tag_prompt = None;
                self.status.clear();
                self.scroll = 0;
            }
            KeyCode::Enter => self.commit_tag_prompt()?,
            KeyCode::Up => {
                let prompt = self.tag_prompt.as_mut().expect("tag prompt is active");
                prompt.selected = prompt.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                let prompt = self.tag_prompt.as_mut().expect("tag prompt is active");
                prompt.selected = (prompt.selected + 1).min(prompt.results.len().saturating_sub(1));
            }
            KeyCode::Backspace => {
                self.tag_prompt
                    .as_mut()
                    .expect("tag prompt is active")
                    .query
                    .pop();
                self.refresh_tag_prompt()?;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.tag_prompt
                    .as_mut()
                    .expect("tag prompt is active")
                    .query
                    .push(character);
                self.refresh_tag_prompt()?;
            }
            _ => {}
        }
        Ok(Action::Continue)
    }

    fn handle_backlink_key(&mut self, key: KeyEvent) -> vrac::Result<Action> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('b') => {
                self.backlinks = None;
                self.status.clear();
                self.scroll = 0;
            }
            KeyCode::Enter => {
                let target = self.backlinks.as_ref().and_then(|view| {
                    view.contexts
                        .get(view.selected)
                        .and_then(|path| path.last())
                        .map(|node| node.id)
                });
                self.backlinks = None;
                if let Some(target) = target {
                    self.set_focus(Some(target))?;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let view = self.backlinks.as_mut().expect("backlinks are active");
                view.selected = view.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.load_more_backlinks_if_needed()?;
                let view = self.backlinks.as_mut().expect("backlinks are active");
                view.selected = (view.selected + 1).min(view.contexts.len().saturating_sub(1));
            }
            _ => {}
        }
        Ok(Action::Continue)
    }

    fn handle_reference_key(&mut self, key: KeyEvent) -> vrac::Result<Action> {
        match key.code {
            KeyCode::Esc => {
                self.reference_prompt = None;
                self.status.clear();
            }
            KeyCode::Enter => self.commit_reference_prompt(),
            KeyCode::Up => {
                let prompt = self
                    .reference_prompt
                    .as_mut()
                    .expect("reference prompt is active");
                prompt.selected = prompt.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                let prompt = self
                    .reference_prompt
                    .as_mut()
                    .expect("reference prompt is active");
                prompt.selected = (prompt.selected + 1).min(prompt.results.len().saturating_sub(1));
            }
            KeyCode::Backspace => {
                let empty = self
                    .reference_prompt
                    .as_ref()
                    .expect("reference prompt is active")
                    .query
                    .is_empty();
                self.editor
                    .as_mut()
                    .expect("reference completion edits a node")
                    .backspace();
                if empty {
                    self.reference_prompt = None;
                } else {
                    self.reference_prompt
                        .as_mut()
                        .expect("reference prompt is active")
                        .query
                        .pop();
                    self.refresh_reference_prompt()?;
                }
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.editor
                    .as_mut()
                    .expect("reference completion edits a node")
                    .insert(character);
                self.reference_prompt
                    .as_mut()
                    .expect("reference prompt is active")
                    .query
                    .push(character);
                self.refresh_reference_prompt()?;
            }
            _ => {}
        }
        Ok(Action::Continue)
    }

    fn handle_editor_key(&mut self, key: KeyEvent) -> vrac::Result<Action> {
        match key.code {
            KeyCode::Esc => self.finish_editor()?,
            KeyCode::Enter => self.create_sibling_from_editor()?,
            KeyCode::Tab => self.indent_editor()?,
            KeyCode::BackTab => self.outdent_editor()?,
            KeyCode::Left => {
                let editor = self.editor.as_mut().expect("editor is active");
                editor.cursor = editor.cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                let editor = self.editor.as_mut().expect("editor is active");
                editor.cursor = (editor.cursor + 1).min(editor.text.chars().count());
            }
            KeyCode::Home => self.editor.as_mut().expect("editor is active").cursor = 0,
            KeyCode::End => {
                let editor = self.editor.as_mut().expect("editor is active");
                editor.cursor = editor.text.chars().count();
            }
            KeyCode::Backspace => self.editor.as_mut().expect("editor is active").backspace(),
            KeyCode::Delete => self.editor.as_mut().expect("editor is active").delete(),
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if character == '#'
                    && let EditTarget::Existing(target_id) =
                        self.editor.as_ref().expect("editor is active").target
                {
                    self.open_tag_prompt(target_id)?;
                    return Ok(Action::Continue);
                }
                let editor = self.editor.as_mut().expect("editor is active");
                editor.insert(character);
                let before_caret: String = editor.text.chars().take(editor.cursor).collect();
                if character == '[' && before_caret.ends_with("[[") {
                    self.reference_prompt = Some(ReferencePrompt {
                        query: String::new(),
                        results: Vec::new(),
                        selected: 0,
                    });
                    self.status.clear();
                }
            }
            _ => {}
        }
        Ok(Action::Continue)
    }

    fn move_selection(&mut self, direction: isize) -> vrac::Result<()> {
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

    fn move_selection_page(&mut self, direction: isize) -> vrac::Result<()> {
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

    fn load_next_page_at_selection(&mut self) -> vrac::Result<()> {
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

    fn select_edge(&mut self, last: bool) {
        let visible = self.visible_nodes();
        self.selected = if last {
            visible.last().map(|item| item.node.id)
        } else {
            visible.first().map(|item| item.node.id)
        };
    }

    fn move_right(&mut self) -> vrac::Result<()> {
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

    fn move_left(&mut self) -> vrac::Result<()> {
        let Some(node) = self.selected_node() else {
            return Ok(());
        };
        if node.parent_id != self.focus {
            let parent_id = node
                .parent_id
                .expect("a visible descendant below the focus has a parent");
            self.selected = Some(parent_id);
        } else {
            self.zoom_out()?;
        }
        Ok(())
    }

    fn toggle_selected(&mut self) -> vrac::Result<()> {
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

    fn expand(&mut self, id: NodeId) -> vrac::Result<()> {
        if !self.branches.contains_key(&Some(id)) {
            self.reload_branch(Some(id))?;
        }
        self.expanded.insert(id);
        Ok(())
    }

    fn start_search(&mut self) -> vrac::Result<()> {
        self.search = Some(Search::new());
        self.status.clear();
        self.scroll = 0;
        self.refresh_search()
    }

    fn refresh_search(&mut self) -> vrac::Result<()> {
        let query = self.search.as_ref().expect("search is active").text.clone();
        let normalized = query.trim().to_lowercase();
        let mut items = COMMANDS
            .iter()
            .filter(|entry| {
                normalized.is_empty()
                    || entry.name.contains(&normalized)
                    || entry.hint.contains(&normalized)
            })
            .copied()
            .map(LauncherItem::Command)
            .collect::<Vec<_>>();
        if normalized.chars().count() >= 2 {
            items.extend(
                self.engine
                    .search(&query, 20)?
                    .into_iter()
                    .map(LauncherItem::Node),
            );
        }
        let search = self.search.as_mut().expect("search is active");
        search.items = items;
        search.selected = 0;
        self.status.clear();
        Ok(())
    }

    fn commit_launcher(&mut self) -> vrac::Result<Action> {
        let item = self
            .search
            .as_ref()
            .and_then(|search| search.items.get(search.selected))
            .cloned();
        self.search = None;
        self.scroll = 0;
        match item {
            Some(LauncherItem::Command(entry)) => self.run_command(entry.command),
            Some(LauncherItem::Node(node)) => {
                self.set_focus(Some(node.id))?;
                Ok(Action::Continue)
            }
            None => Ok(Action::Continue),
        }
    }

    fn run_command(&mut self, command: Command) -> vrac::Result<Action> {
        match command {
            Command::New => self.start_new_sibling(),
            Command::NewBefore => self.start_new_before(),
            Command::NewChild => self.start_new_child(),
            Command::Zoom => self.zoom_selected()?,
            Command::ZoomOut => self.zoom_out()?,
            Command::Today => {
                let today = jiff::Zoned::now().date().to_string();
                let day = self.engine.journal_day(&today)?;
                self.set_focus(Some(day.id))?;
            }
            Command::Root => self.set_focus(None)?,
            Command::FocusParent => self.move_left()?,
            Command::FocusChild => self.move_right()?,
            Command::Toggle => self.toggle_selected()?,
            Command::Indent => self.indent_selected()?,
            Command::Outdent => self.outdent_selected()?,
            Command::Delete => self.delete_selected()?,
            Command::Copy => self.copy_selected()?,
            Command::Paste => self.paste_after_selected()?,
            Command::Undo => self.undo()?,
            Command::Redo => self.redo()?,
            Command::Tag => self.start_tag_prompt()?,
            Command::Backlinks => self.start_backlinks()?,
            Command::Quit => return Ok(Action::Quit),
        }
        Ok(Action::Continue)
    }

    fn start_tag_prompt(&mut self) -> vrac::Result<()> {
        let target_id = match self.selected {
            Some(id) => id,
            None => match self.focus {
                Some(id) => id,
                None => {
                    self.status = "No node is selected".into();
                    return Ok(());
                }
            },
        };
        self.open_tag_prompt(target_id)
    }

    fn open_tag_prompt(&mut self, target_id: NodeId) -> vrac::Result<()> {
        let results = self.engine.tags("", 20)?;
        self.tag_prompt = Some(TagPrompt {
            target_id,
            query: String::new(),
            results,
            selected: 0,
        });
        self.status.clear();
        self.scroll = 0;
        Ok(())
    }

    fn refresh_tag_prompt(&mut self) -> vrac::Result<()> {
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

    fn commit_tag_prompt(&mut self) -> vrac::Result<()> {
        let Some(prompt) = self.tag_prompt.take() else {
            return Ok(());
        };
        let Some(tag) = prompt.results.get(prompt.selected).cloned() else {
            self.status = "Type a tag first".into();
            self.tag_prompt = Some(prompt);
            return Ok(());
        };
        let node = self
            .engine
            .node(prompt.target_id)?
            .ok_or(vrac::Error::NodeNotFound(prompt.target_id))?;
        let mut tags = node.tags;
        let removed = if let Some(index) = tags.iter().position(|existing| existing == &tag) {
            tags.remove(index);
            true
        } else {
            tags.push(tag.clone());
            false
        };
        if let Err(error) = self.engine.set_tags(prompt.target_id, tags) {
            self.tag_prompt = Some(prompt);
            return Err(error);
        }
        self.refresh_cached_node(prompt.target_id)?;
        self.status = if removed {
            format!("Removed #{tag}")
        } else {
            format!("Added #{tag}")
        };
        Ok(())
    }

    fn start_backlinks(&mut self) -> vrac::Result<()> {
        let target_id = match self.selected.or(self.focus) {
            Some(id) => id,
            None => {
                self.status = "No node is selected".into();
                return Ok(());
            }
        };
        let page = self.engine.backlinks(target_id, None, Page::default())?;
        self.backlinks = Some(BacklinkView {
            target_id,
            contexts: page
                .contexts
                .into_iter()
                .map(|context| context.path)
                .collect(),
            next: page.next,
            selected: 0,
        });
        self.status.clear();
        self.scroll = 0;
        Ok(())
    }

    fn load_more_backlinks_if_needed(&mut self) -> vrac::Result<()> {
        let Some(view) = self.backlinks.as_ref() else {
            return Ok(());
        };
        if view.selected + 1 < view.contexts.len() {
            return Ok(());
        }
        let Some(after) = view.next else {
            return Ok(());
        };
        let target_id = view.target_id;
        let page = self.engine.backlinks(
            target_id,
            None,
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

    fn refresh_reference_prompt(&mut self) -> vrac::Result<()> {
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

    fn commit_reference_prompt(&mut self) {
        let Some(prompt) = self.reference_prompt.take() else {
            return;
        };
        let editor = self
            .editor
            .as_mut()
            .expect("reference completion edits a node");
        if let Some(target) = prompt.results.get(prompt.selected) {
            for _ in 0..prompt.query.chars().count() {
                editor.backspace();
            }
            let label_start = char_to_byte(&editor.text, editor.cursor);
            for character in target.text.chars() {
                editor.insert(character);
            }
            let label_end = char_to_byte(&editor.text, editor.cursor);
            editor.insert(']');
            editor.insert(']');
            editor.references.push(ReferenceInput {
                label_start,
                label_end,
                target_id: target.id,
            });
        } else {
            editor.insert(']');
            editor.insert(']');
        }
        self.status.clear();
    }

    fn start_edit(&mut self) {
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
        ));
        self.status.clear();
    }

    fn start_edit_at_start(&mut self) {
        self.start_edit();
        if let Some(editor) = &mut self.editor {
            editor.cursor = 0;
        }
    }

    fn start_new_sibling(&mut self) {
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
        self.editor = Some(Editor::new(target, String::new(), Vec::new()));
        self.status.clear();
    }

    fn start_new_before(&mut self) {
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
        self.editor = Some(Editor::new(target, String::new(), Vec::new()));
        self.status.clear();
    }

    fn create_sibling_from_editor(&mut self) -> vrac::Result<()> {
        if self.editor.as_ref().is_some_and(|editor| {
            matches!(editor.target, EditTarget::New { .. }) && editor.text.is_empty()
        }) {
            return Ok(());
        }
        let Some(saved) = self.commit_editor()? else {
            return Ok(());
        };
        self.selected = Some(saved.id);
        self.editor = Some(Editor::new(
            EditTarget::New {
                parent_id: saved.parent_id,
                placement: Placement::After(saved.id),
            },
            String::new(),
            Vec::new(),
        ));
        self.status.clear();
        Ok(())
    }

    fn finish_editor(&mut self) -> vrac::Result<()> {
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

    fn indent_editor(&mut self) -> vrac::Result<()> {
        if self.adjust_new_draft(true)? {
            return Ok(());
        }
        let Some(saved) = self.commit_editor()? else {
            return Ok(());
        };
        self.selected = Some(saved.id);
        self.indent_selected()?;
        self.resume_editing(saved.id)
    }

    fn outdent_editor(&mut self) -> vrac::Result<()> {
        if self.adjust_new_draft(false)? {
            return Ok(());
        }
        let Some(saved) = self.commit_editor()? else {
            return Ok(());
        };
        self.selected = Some(saved.id);
        self.outdent_selected()?;
        self.resume_editing(saved.id)
    }

    fn adjust_new_draft(&mut self, indent: bool) -> vrac::Result<bool> {
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
            self.expanded.insert(parent_id);
        }
        self.editor.as_mut().expect("editor is active").target = next_target;
        self.status.clear();
        Ok(true)
    }

    fn resume_editing(&mut self, id: NodeId) -> vrac::Result<()> {
        let node = self.engine.node(id)?.ok_or(vrac::Error::NodeNotFound(id))?;
        let references = node
            .references
            .iter()
            .map(|reference| ReferenceInput {
                label_start: reference.label_start,
                label_end: reference.label_end,
                target_id: reference.target_id,
            })
            .collect();
        self.editor = Some(Editor::new(EditTarget::Existing(id), node.text, references));
        self.status.clear();
        Ok(())
    }

    fn indent_selected(&mut self) -> vrac::Result<()> {
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
        self.reload_branch(old_parent)?;
        self.reload_branch(Some(new_parent))?;
        self.expanded.insert(new_parent);
        self.selected = Some(node.id);
        self.status = "Indented".into();
        Ok(())
    }

    fn outdent_selected(&mut self) -> vrac::Result<()> {
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
            .ok_or(vrac::Error::NodeNotFound(parent_id))?;
        let destination_parent = parent.parent_id;
        self.engine.move_node(
            node.id,
            Destination {
                parent_id: destination_parent,
                placement: Placement::After(parent_id),
            },
        )?;
        self.reload_branch(Some(parent_id))?;
        self.reload_branch(destination_parent)?;
        if self.focus == Some(parent_id) {
            self.set_focus(destination_parent)?;
        }
        self.selected = Some(node.id);
        self.status = "Outdented".into();
        Ok(())
    }

    fn undo(&mut self) -> vrac::Result<()> {
        if self.engine.undo()? {
            self.reload_after_history()?;
            self.status = "Undone".into();
        } else {
            self.status = "Nothing to undo".into();
        }
        Ok(())
    }

    fn redo(&mut self) -> vrac::Result<()> {
        if self.engine.redo()? {
            self.reload_after_history()?;
            self.status = "Redone".into();
        } else {
            self.status = "Nothing to redo".into();
        }
        Ok(())
    }

    fn reload_after_history(&mut self) -> vrac::Result<()> {
        let focus = match self.focus {
            Some(id) if self.engine.node(id)?.is_some() => Some(id),
            _ => None,
        };
        self.branches.clear();
        self.expanded.clear();
        self.set_focus(focus)
    }

    fn copy_selected(&mut self) -> vrac::Result<()> {
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

    fn delete_selected(&mut self) -> vrac::Result<()> {
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
        self.reload_branch(node.parent_id)?;
        if let Some(parent_id) = node.parent_id {
            self.refresh_cached_node(parent_id)?;
        }
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

    fn paste_after_selected(&mut self) -> vrac::Result<()> {
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
        self.reload_branch(parent_id)?;
        if let Some(parent_id) = parent_id {
            self.refresh_cached_node(parent_id)?;
        }
        if parent_id.is_some() && self.branches.contains_key(&None) {
            self.reload_branch(None)?;
        }
        self.selected = created.first().map(|node| node.id).or(parent_id);
        self.status = format!("Pasted {} subtree(s)", created.len());
        Ok(())
    }

    fn refresh_cached_node(&mut self, id: NodeId) -> vrac::Result<()> {
        let Some(updated) = self.engine.node(id)? else {
            return Ok(());
        };
        for branch in self.branches.values_mut() {
            if let Some(node) = branch.nodes.iter_mut().find(|node| node.id == id) {
                *node = updated.clone();
            }
        }
        Ok(())
    }

    fn start_new_child(&mut self) {
        let Some(node) = self.selected_node() else {
            return;
        };
        self.editor = Some(Editor::new(
            EditTarget::New {
                parent_id: Some(node.id),
                placement: Placement::Last,
            },
            String::new(),
            Vec::new(),
        ));
        self.status.clear();
    }

    fn commit_editor(&mut self) -> vrac::Result<Option<Node>> {
        let Some(editor) = self.editor.take() else {
            return Ok(None);
        };
        let result = (|| -> vrac::Result<Option<Node>> {
            match editor.target.clone() {
                EditTarget::Existing(id) => {
                    let update = self.engine.set_content(
                        id,
                        editor.text.clone(),
                        editor.references.clone(),
                    )?;
                    let updated = self.engine.node(id)?.ok_or(vrac::Error::NodeNotFound(id))?;
                    if !update.materialized_nodes.is_empty() || !update.pruned_roots.is_empty() {
                        self.reload_branch(None)?;
                    }
                    for pruned in update.pruned_roots {
                        self.branches.remove(&Some(pruned));
                        self.expanded.remove(&pruned);
                    }
                    for branch in self.branches.values_mut() {
                        if let Some(node) = branch.nodes.iter_mut().find(|node| node.id == id) {
                            *node = updated.clone();
                        }
                    }
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
                    let created = self.engine.create_node(input)?;
                    self.reload_branch(parent_id)?;
                    if let Some(parent_id) = parent_id {
                        self.expanded.insert(parent_id);
                    }
                    let loaded = self.branches.get(&parent_id).is_some_and(|branch| {
                        branch.nodes.iter().any(|node| node.id == created.id)
                    });
                    self.selected = if loaded { Some(created.id) } else { parent_id };
                    self.status = if loaded {
                        "Created".into()
                    } else {
                        "Created after the first 100 loaded children".into()
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

fn char_to_byte(text: &str, character: usize) -> usize {
    text.char_indices()
        .nth(character)
        .map_or(text.len(), |(byte, _)| byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> (App, Node, Node) {
        let mut engine = Engine::open(":memory:").unwrap();
        let parent = engine.create_node(CreateNode::new("Parent")).unwrap();
        let mut child_input = CreateNode::new("Child");
        child_input.parent_id = Some(parent.id);
        let child = engine.create_node(child_input).unwrap();
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
    fn held_navigation_keys_are_actionable() {
        assert!(actionable_key(KeyEventKind::Press));
        assert!(actionable_key(KeyEventKind::Repeat));
        assert!(!actionable_key(KeyEventKind::Release));
    }

    #[test]
    fn normal_startup_focuses_today() {
        let app = App::open(Engine::open(":memory:").unwrap()).unwrap();
        let today = jiff::Zoned::now().date().to_string();
        assert!(matches!(
            app.focus_path.last().and_then(|node| node.system.as_ref()),
            Some(vrac::SystemNode::JournalDay { date }) if date == &today
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
        app.start_search().unwrap();
        for character in "vrac".chars() {
            app.search.as_mut().unwrap().insert(character);
        }
        app.refresh_search().unwrap();

        assert!(matches!(
            &app.search.as_ref().unwrap().items[0],
            LauncherItem::Node(node) if node.id == parent.id
        ));
        app.handle_search_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
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
        assert_eq!(app.tag_prompt.as_ref().unwrap().target_id, parent.id);
        assert_eq!(app.editor.as_ref().unwrap().text, "Parent");
    }

    #[test]
    fn backlinks_open_the_matching_context() {
        let mut engine = Engine::open(":memory:").unwrap();
        let source = engine
            .create_node(CreateNode::new("See [[Target]]"))
            .unwrap();
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
    fn indent_and_outdent_use_engine_moves() {
        let (mut app, parent, _) = test_app();
        let sibling = app.engine.create_node(CreateNode::new("Sibling")).unwrap();
        app.reload_branch(None).unwrap();
        app.selected = Some(sibling.id);

        app.indent_selected().unwrap();
        assert_eq!(
            app.engine.node(sibling.id).unwrap().unwrap().parent_id,
            Some(parent.id)
        );

        app.outdent_selected().unwrap();
        assert_eq!(
            app.engine.node(sibling.id).unwrap().unwrap().parent_id,
            None
        );
    }

    #[test]
    fn editing_and_creation_go_through_the_engine() {
        let (mut app, parent, _) = test_app();
        app.selected = Some(parent.id);
        app.start_edit();
        app.editor.as_mut().unwrap().text = "Renamed".into();
        app.commit_editor().unwrap();
        assert_eq!(app.engine.node(parent.id).unwrap().unwrap().text, "Renamed");

        app.start_new_child();
        app.editor.as_mut().unwrap().text = "New child".into();
        app.commit_editor().unwrap();
        let children = app
            .engine
            .children(Some(parent.id), Page::default())
            .unwrap();
        assert!(children.nodes.iter().any(|node| node.text == "New child"));
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
    fn tab_and_backtab_move_a_node_without_leaving_inline_editing() {
        let (mut app, parent, _) = test_app();
        let sibling = app.engine.create_node(CreateNode::new("Sibling")).unwrap();
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
    fn slash_and_colon_open_the_same_launcher() {
        let (mut app, _, _) = test_app();

        app.handle_normal_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
            .unwrap();
        assert!(app.search.as_ref().unwrap().items.iter().any(
            |item| matches!(item, LauncherItem::Command(entry) if entry.command == Command::New)
        ));
        app.handle_search_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();

        app.handle_normal_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE))
            .unwrap();
        assert!(app.search.as_ref().unwrap().items.iter().any(
            |item| matches!(item, LauncherItem::Command(entry) if entry.command == Command::New)
        ));
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
    fn editing_preserves_untouched_stable_references() {
        let mut engine = Engine::open(":memory:").unwrap();
        let source = engine
            .create_node(CreateNode::new("See [[Target]]"))
            .unwrap();
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
        let target = engine.create_node(CreateNode::new("Project")).unwrap();
        let source = engine.create_node(CreateNode::new("See ")).unwrap();
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
    fn a_failed_creation_keeps_its_draft() {
        let (mut app, _, _) = test_app();
        let journal = app
            .branches
            .get(&None)
            .unwrap()
            .nodes
            .iter()
            .find(|node| matches!(node.system, Some(vrac::SystemNode::Journal)))
            .unwrap()
            .id;
        app.selected = Some(journal);
        app.start_new_child();
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
}
