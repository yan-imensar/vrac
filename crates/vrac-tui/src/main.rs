use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use vrac::{
    CreateNode, Cursor, Destination, Engine, Node, NodeId, Page, Placement, ReferenceInput,
};

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

#[derive(Clone, Debug, Eq, PartialEq)]
struct Search {
    text: String,
    cursor: usize,
    results: Vec<Node>,
    selected: usize,
}

impl Search {
    fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            results: Vec::new(),
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
    status: String,
    scroll: usize,
}

impl App {
    fn open(engine: Engine) -> vrac::Result<Self> {
        let mut app = Self {
            engine,
            branches: HashMap::new(),
            expanded: HashSet::new(),
            focus: None,
            focus_path: Vec::new(),
            selected: None,
            editor: None,
            search: None,
            status: String::new(),
            scroll: 0,
        };
        app.reload_branch(None)?;
        app.selected = app
            .branches
            .get(&None)
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
        if self.search.is_some() {
            self.handle_search_key(key)
        } else if self.editor.is_some() {
            self.handle_editor_key(key)
        } else {
            self.handle_normal_key(key)
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> vrac::Result<Action> {
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
            KeyCode::Char('/') => self.start_search(),
            KeyCode::Char('i') => self.start_edit(),
            KeyCode::Char('o') => self.start_new_sibling(),
            KeyCode::Char('c') => self.start_new_child(),
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
            KeyCode::Enter => {
                let target = self
                    .search
                    .as_ref()
                    .and_then(|search| search.results.get(search.selected).map(|node| node.id));
                self.search = None;
                if let Some(target) = target {
                    self.set_focus(Some(target))?;
                }
            }
            KeyCode::Up => {
                let search = self.search.as_mut().expect("search is active");
                search.selected = search.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                let search = self.search.as_mut().expect("search is active");
                search.selected = (search.selected + 1).min(search.results.len().saturating_sub(1));
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

    fn handle_editor_key(&mut self, key: KeyEvent) -> vrac::Result<Action> {
        match key.code {
            KeyCode::Esc => {
                self.editor = None;
                self.status = "Edit cancelled".into();
            }
            KeyCode::Enter => self.commit_editor()?,
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
                self.editor
                    .as_mut()
                    .expect("editor is active")
                    .insert(character);
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

    fn start_search(&mut self) {
        self.search = Some(Search::new());
        self.status.clear();
        self.scroll = 0;
    }

    fn refresh_search(&mut self) -> vrac::Result<()> {
        let query = self.search.as_ref().expect("search is active").text.clone();
        let results = self.engine.search(&query, 20)?;
        let search = self.search.as_mut().expect("search is active");
        search.results = results;
        search.selected = search.selected.min(search.results.len().saturating_sub(1));
        self.status.clear();
        Ok(())
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

    fn commit_editor(&mut self) -> vrac::Result<()> {
        let Some(editor) = self.editor.take() else {
            return Ok(());
        };
        let result = (|| {
            match editor.target {
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
                }
                EditTarget::New {
                    parent_id,
                    placement,
                } => {
                    if editor.text.is_empty() {
                        self.status = "Empty node not created".into();
                        return Ok(());
                    }
                    let mut input = CreateNode::new(editor.text.clone());
                    input.parent_id = parent_id;
                    input.placement = placement;
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
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.editor = Some(editor);
        }
        result
    }
}

#[derive(Clone)]
struct DisplayLine {
    selected: bool,
    text: String,
}

fn draw(stdout: &mut Stdout, app: &mut App, path: &Path) -> io::Result<()> {
    let (width, height) = terminal::size()?;
    let width = usize::from(width);
    let height = usize::from(height);
    let body_height = height.saturating_sub(4);
    let lines = match &app.search {
        Some(search) => search_lines(search, width),
        None => display_lines(app, width),
    };
    let selected_start = lines
        .iter()
        .position(|line| line.selected)
        .unwrap_or_default();
    let selected_end = lines
        .iter()
        .rposition(|line| line.selected)
        .unwrap_or(selected_start);
    if selected_start < app.scroll {
        app.scroll = selected_start;
    } else if body_height > 0 && selected_end >= app.scroll + body_height {
        let selection_height = selected_end + 1 - selected_start;
        app.scroll = if selection_height > body_height {
            selected_start
        } else {
            selected_end + 1 - body_height
        };
    }
    app.scroll = app.scroll.min(lines.len().saturating_sub(body_height));

    queue!(stdout, Hide, MoveTo(0, 0), Clear(ClearType::All))?;
    queue!(
        stdout,
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold),
        Print(fit(
            &format!(
                "Vrac TUI  {}",
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into()
                )
            ),
            width
        )),
        SetAttribute(Attribute::Reset),
        ResetColor
    )?;
    if height > 1 {
        queue!(stdout, MoveTo(0, 1), SetForegroundColor(Color::DarkGrey))?;
        queue!(stdout, Print(fit(&app.focus_label(), width)), ResetColor)?;
    }

    for (offset, line) in lines.iter().skip(app.scroll).take(body_height).enumerate() {
        queue!(
            stdout,
            MoveTo(0, u16::try_from(offset + 2).unwrap_or(u16::MAX))
        )?;
        if line.selected {
            queue!(
                stdout,
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold)
            )?;
        }
        queue!(stdout, Print(fit(&line.text, width)))?;
        if line.selected {
            queue!(stdout, SetAttribute(Attribute::Reset), ResetColor)?;
        }
    }

    if let Some(search) = &app.search {
        draw_search_footer(stdout, search, &app.status, width, height)?;
    } else if let Some(editor) = &app.editor {
        draw_editor(stdout, editor, &app.status, width, height)?;
    } else {
        draw_normal_footer(stdout, &app.status, width, height)?;
    }
    stdout.flush()
}

fn draw_normal_footer(
    stdout: &mut Stdout,
    status: &str,
    width: usize,
    height: usize,
) -> io::Result<()> {
    if height >= 2 {
        queue!(stdout, MoveTo(0, u16::try_from(height - 2).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::Yellow),
            Print(fit(status, width)),
            ResetColor
        )?;
    }
    if height >= 1 {
        let help =
            "j/k move  h/l parent/child  Enter zoom  - back  / search  i edit  Tab indent  q quit";
        queue!(stdout, MoveTo(0, u16::try_from(height - 1).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(fit(help, width)),
            ResetColor
        )?;
    }
    Ok(())
}

fn draw_search_footer(
    stdout: &mut Stdout,
    search: &Search,
    status: &str,
    width: usize,
    height: usize,
) -> io::Result<()> {
    if height >= 2 {
        let label = if status.is_empty() {
            "SEARCH  ↑/↓ select · Enter open · Esc cancel"
        } else {
            status
        };
        queue!(stdout, MoveTo(0, u16::try_from(height - 2).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::Yellow),
            Print(fit(label, width)),
            ResetColor
        )?;
    }
    if height >= 1 {
        let input_width = width.saturating_sub(2);
        let (view, cursor_column) = editor_view(&search.text, search.cursor, input_width);
        queue!(stdout, MoveTo(0, u16::try_from(height - 1).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::Cyan),
            Print("/ "),
            ResetColor
        )?;
        queue!(stdout, Print(fit(&view, input_width)), Show)?;
        let column = (2 + cursor_column).min(width.saturating_sub(1));
        queue!(
            stdout,
            MoveTo(
                u16::try_from(column).unwrap_or(u16::MAX),
                u16::try_from(height - 1).unwrap_or(0)
            )
        )?;
    }
    Ok(())
}

fn draw_editor(
    stdout: &mut Stdout,
    editor: &Editor,
    status: &str,
    width: usize,
    height: usize,
) -> io::Result<()> {
    if height >= 2 {
        let label = if status.is_empty() {
            match editor.target {
                EditTarget::Existing(_) => "EDIT  Enter save · Esc cancel",
                EditTarget::New { .. } => "NEW   Enter create · Esc cancel",
            }
        } else {
            status
        };
        queue!(stdout, MoveTo(0, u16::try_from(height - 2).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::Yellow),
            Print(fit(label, width)),
            ResetColor
        )?;
    }
    if height >= 1 {
        let input_width = width.saturating_sub(2);
        let (view, cursor_column) = editor_view(&editor.text, editor.cursor, input_width);
        queue!(stdout, MoveTo(0, u16::try_from(height - 1).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::Cyan),
            Print("> "),
            ResetColor
        )?;
        queue!(stdout, Print(fit(&view, input_width)), Show)?;
        let column = (2 + cursor_column).min(width.saturating_sub(1));
        queue!(
            stdout,
            MoveTo(
                u16::try_from(column).unwrap_or(u16::MAX),
                u16::try_from(height - 1).unwrap_or(0)
            )
        )?;
    }
    Ok(())
}

fn display_lines(app: &App, width: usize) -> Vec<DisplayLine> {
    let mut lines = Vec::new();
    for item in app.visible_nodes() {
        let selected = app.selected == Some(item.node.id);
        let selector = if selected { "› " } else { "  " };
        let indent = "  ".repeat(item.depth);
        let marker = if item.node.has_children {
            if app.expanded.contains(&item.node.id) {
                "▾"
            } else {
                "▸"
            }
        } else {
            "•"
        };
        let prefix = format!("{selector}{indent}{marker} ");
        let continuation = " ".repeat(UnicodeWidthStr::width(prefix.as_str()));
        let tags = item
            .node
            .tags
            .iter()
            .map(|tag| format!("#{tag}"))
            .collect::<Vec<_>>()
            .join(" ");
        let text = if tags.is_empty() {
            item.node.text.replace('\n', " ↵ ")
        } else {
            format!("{}  {tags}", item.node.text.replace('\n', " ↵ "))
        };
        let available = width
            .saturating_sub(UnicodeWidthStr::width(prefix.as_str()))
            .max(1);
        let wrapped = wrap_text(&text, available);
        for (index, content) in wrapped.into_iter().enumerate() {
            lines.push(DisplayLine {
                selected,
                text: format!(
                    "{}{}",
                    if index == 0 { &prefix } else { &continuation },
                    content
                ),
            });
        }
    }
    lines
}

fn search_lines(search: &Search, width: usize) -> Vec<DisplayLine> {
    if search.results.is_empty() {
        return vec![DisplayLine {
            selected: false,
            text: if search.text.trim().chars().count() < 2 {
                "  Type at least two characters".into()
            } else {
                "  No results".into()
            },
        }];
    }
    search
        .results
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let selected = index == search.selected;
            let tags = node
                .tags
                .iter()
                .map(|tag| format!("#{tag}"))
                .collect::<Vec<_>>()
                .join(" ");
            let text = if tags.is_empty() {
                node.text.replace('\n', " ↵ ")
            } else {
                format!("{}  {tags}", node.text.replace('\n', " ↵ "))
            };
            DisplayLine {
                selected,
                text: fit(
                    &format!("{}• {text}", if selected { "› " } else { "  " }),
                    width,
                ),
            }
        })
        .collect()
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut line_width = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if !line.is_empty() && line_width + character_width > width {
            lines.push(line);
            line = String::new();
            line_width = 0;
        }
        line.push(character);
        line_width += character_width;
    }
    lines.push(line);
    lines
}

fn fit(text: &str, width: usize) -> String {
    let mut fitted = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width {
            break;
        }
        fitted.push(character);
        used += character_width;
    }
    fitted
}

fn editor_view(text: &str, cursor: usize, width: usize) -> (String, usize) {
    let characters: Vec<char> = text.chars().collect();
    let mut start = 0;
    while start < cursor
        && display_width(&characters[start..cursor]) >= width.saturating_sub(1).max(1)
    {
        start += 1;
    }
    let cursor_column = display_width(&characters[start..cursor]);
    let mut view = String::new();
    let mut used = 0;
    for character in &characters[start..] {
        let shown = if character.is_control() {
            '↵'
        } else {
            *character
        };
        let character_width = UnicodeWidthChar::width(shown).unwrap_or(0);
        if used + character_width > width {
            break;
        }
        view.push(shown);
        used += character_width;
    }
    (view, cursor_column)
}

fn display_width(characters: &[char]) -> usize {
    characters
        .iter()
        .map(|character| {
            UnicodeWidthChar::width(if character.is_control() {
                '↵'
            } else {
                *character
            })
            .unwrap_or(0)
        })
        .sum()
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
        (App::open(engine).unwrap(), parent, child)
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
        let mut app = App::open(engine).unwrap();
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
        app.start_search();
        for character in "vrac".chars() {
            app.search.as_mut().unwrap().insert(character);
        }
        app.refresh_search().unwrap();

        assert_eq!(app.search.as_ref().unwrap().results[0].id, parent.id);
        app.handle_search_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.focus, Some(parent.id));
        assert_eq!(app.focus_label(), "root › Vrac concept");
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
    fn editing_preserves_untouched_stable_references() {
        let mut engine = Engine::open(":memory:").unwrap();
        let source = engine
            .create_node(CreateNode::new("See [[Target]]"))
            .unwrap();
        let target = source.references[0].target_id;
        let mut app = App::open(engine).unwrap();
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
