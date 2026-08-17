//! Terminal key dispatch, prompts, and commands.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vrac_engine::ReferenceInput;

use super::commands::Command;
use super::editor::{EditTarget, char_to_byte};
use super::model::{Action, App};
use super::prompts::{
    BacklinkView, Launcher, LauncherItem, LauncherKind, ReferencePrompt, TagPrompt, TagTarget,
};

impl App {
    pub(super) fn handle_key(&mut self, key: KeyEvent) -> vrac_engine::Result<Action> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.finish_editor()?;
            return Ok(Action::Quit);
        }
        if self.help {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                self.help = false;
                self.scroll = 0;
            }
            Ok(Action::Continue)
        } else if self.reference_prompt.is_some() {
            self.handle_reference_key(key)
        } else if self.backlinks.is_some() {
            self.handle_backlink_key(key)
        } else if self.tag_prompt.is_some() {
            self.handle_tag_key(key)
        } else if self.launcher.is_some() {
            self.handle_launcher_key(key)
        } else if self.editor.is_some() {
            self.handle_editor_key(key)
        } else {
            self.handle_normal_key(key)
        }
    }

    pub(super) fn handle_paste(&mut self, pasted: &str) -> vrac_engine::Result<()> {
        let pasted = pasted
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', " ");
        if let Some(prompt) = &mut self.reference_prompt {
            self.editor
                .as_mut()
                .expect("reference completion edits a node")
                .insert_text(&pasted);
            if let Some((query, _)) = pasted.split_once(']') {
                prompt.query.push_str(query);
                self.reference_prompt = None;
                self.status.clear();
            } else {
                prompt.query.push_str(&pasted);
                self.refresh_reference_prompt()?;
            }
        } else if let Some(prompt) = &mut self.tag_prompt {
            prompt.query.extend(
                pasted
                    .chars()
                    .filter(|character| !character.is_whitespace() && *character != '#'),
            );
            self.refresh_tag_prompt()?;
        } else if let Some(launcher) = &mut self.launcher {
            for character in pasted.chars() {
                launcher.insert(character);
            }
            self.refresh_launcher()?;
        } else if let Some(editor) = &mut self.editor {
            editor.insert_text(&pasted);
        }
        Ok(())
    }

    pub(super) fn handle_normal_key(&mut self, key: KeyEvent) -> vrac_engine::Result<Action> {
        if let Some(pending) = self.pending_key.take() {
            self.status.clear();
            if key.code == KeyCode::Char(pending) {
                match pending {
                    'y' => self.copy_selected()?,
                    'd' => self.delete_selected()?,
                    _ => unreachable!("only known prefixes are retained"),
                }
                return Ok(Action::Continue);
            }
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
            KeyCode::Char('H') => self.zoom_out()?,
            KeyCode::Char(' ') => self.toggle_selected()?,
            KeyCode::Enter => self.zoom_selected()?,
            KeyCode::Char('/') => self.start_launcher(LauncherKind::Search)?,
            KeyCode::Char(':') => self.start_launcher(LauncherKind::Commands)?,
            KeyCode::Char('#') => self.start_tag_prompt()?,
            KeyCode::Char('b') => self.start_backlinks()?,
            KeyCode::Char('?') => {
                self.help = true;
                self.scroll = 0;
            }
            KeyCode::Char('i') | KeyCode::Char('I') => self.start_edit_at_start(),
            KeyCode::Char('a') | KeyCode::Char('A') => self.start_edit(),
            KeyCode::Char('o') => self.start_new_sibling(),
            KeyCode::Char('O') => self.start_new_before(),
            KeyCode::Char('c') => self.start_new_child()?,
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

    pub(super) fn handle_launcher_key(&mut self, key: KeyEvent) -> vrac_engine::Result<Action> {
        match key.code {
            KeyCode::Esc => {
                self.launcher = None;
                self.status.clear();
                self.scroll = 0;
            }
            KeyCode::Enter => return self.commit_launcher(),
            KeyCode::Up => {
                let launcher = self.launcher.as_mut().expect("launcher is active");
                launcher.selected = launcher.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                let launcher = self.launcher.as_mut().expect("launcher is active");
                launcher.selected =
                    (launcher.selected + 1).min(launcher.items.len().saturating_sub(1));
            }
            KeyCode::Left => {
                let launcher = self.launcher.as_mut().expect("launcher is active");
                launcher.cursor = launcher.cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                let launcher = self.launcher.as_mut().expect("launcher is active");
                launcher.cursor = (launcher.cursor + 1).min(launcher.text.chars().count());
            }
            KeyCode::Home => self.launcher.as_mut().expect("launcher is active").cursor = 0,
            KeyCode::End => {
                let launcher = self.launcher.as_mut().expect("launcher is active");
                launcher.cursor = launcher.text.chars().count();
            }
            KeyCode::Backspace => {
                self.launcher
                    .as_mut()
                    .expect("launcher is active")
                    .backspace();
                self.refresh_launcher()?;
            }
            KeyCode::Delete => {
                self.launcher.as_mut().expect("launcher is active").delete();
                self.refresh_launcher()?;
            }
            KeyCode::Char(character)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.launcher
                    .as_mut()
                    .expect("launcher is active")
                    .insert(character);
                self.refresh_launcher()?;
            }
            _ => {}
        }
        Ok(Action::Continue)
    }

    pub(super) fn handle_tag_key(&mut self, key: KeyEvent) -> vrac_engine::Result<Action> {
        match key.code {
            KeyCode::Esc => {
                self.tag_prompt = None;
                self.status.clear();
                self.scroll = 0;
            }
            KeyCode::Enter | KeyCode::Tab => self.commit_tag_prompt()?,
            KeyCode::Up => {
                let prompt = self.tag_prompt.as_mut().expect("tag prompt is active");
                prompt.selected = prompt.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                let prompt = self.tag_prompt.as_mut().expect("tag prompt is active");
                prompt.selected = (prompt.selected + 1).min(prompt.results.len().saturating_sub(1));
            }
            KeyCode::Backspace => {
                let prompt = self.tag_prompt.as_mut().expect("tag prompt is active");
                if prompt.query.is_empty() {
                    self.tag_prompt = None;
                    self.status.clear();
                } else {
                    prompt.query.pop();
                    self.refresh_tag_prompt()?;
                }
            }
            KeyCode::Char(' ') => {
                self.tag_prompt = None;
                if let Some(editor) = &mut self.editor {
                    editor.insert(' ');
                }
                self.status.clear();
            }
            KeyCode::Char(character)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
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

    pub(super) fn handle_backlink_key(&mut self, key: KeyEvent) -> vrac_engine::Result<Action> {
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

    pub(super) fn handle_reference_key(&mut self, key: KeyEvent) -> vrac_engine::Result<Action> {
        match key.code {
            KeyCode::Esc => {
                self.reference_prompt = None;
                self.status.clear();
            }
            KeyCode::Enter | KeyCode::Tab => self.commit_reference_prompt(),
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
            KeyCode::Char(']') => {
                self.editor
                    .as_mut()
                    .expect("reference completion edits a node")
                    .insert(']');
                self.reference_prompt = None;
                self.status.clear();
            }
            KeyCode::Char(character)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
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

    pub(super) fn handle_editor_key(&mut self, key: KeyEvent) -> vrac_engine::Result<Action> {
        let editor_width = self.editor_width();
        let word_modifier = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => self.finish_editor()?,
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
            {
                self.zoom_from_editor()?
            }
            KeyCode::Enter => self.create_sibling_from_editor()?,
            KeyCode::Tab => self.indent_editor()?,
            KeyCode::BackTab => self.outdent_editor()?,
            KeyCode::Up => {
                let moved = self
                    .editor
                    .as_mut()
                    .expect("editor is active")
                    .move_vertical(-1, editor_width);
                if !moved {
                    self.move_editor_to_adjacent(-1)?;
                }
            }
            KeyCode::Down => {
                let moved = self
                    .editor
                    .as_mut()
                    .expect("editor is active")
                    .move_vertical(1, editor_width);
                if !moved {
                    self.move_editor_to_adjacent(1)?;
                }
            }
            KeyCode::Left if word_modifier => self
                .editor
                .as_mut()
                .expect("editor is active")
                .move_word(-1),
            KeyCode::Left => {
                let editor = self.editor.as_mut().expect("editor is active");
                editor.cursor = editor.cursor.saturating_sub(1);
            }
            KeyCode::Right if word_modifier => {
                self.editor.as_mut().expect("editor is active").move_word(1)
            }
            KeyCode::Right => {
                let editor = self.editor.as_mut().expect("editor is active");
                editor.cursor = (editor.cursor + 1).min(editor.text.chars().count());
            }
            KeyCode::Home
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
            {
                self.editor.as_mut().expect("editor is active").cursor = 0
            }
            KeyCode::Home => self
                .editor
                .as_mut()
                .expect("editor is active")
                .move_to_visual_edge(false, editor_width),
            KeyCode::End
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
            {
                let editor = self.editor.as_mut().expect("editor is active");
                editor.cursor = editor.text.chars().count();
            }
            KeyCode::End => self
                .editor
                .as_mut()
                .expect("editor is active")
                .move_to_visual_edge(true, editor_width),
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::ALT) => self
                .editor
                .as_mut()
                .expect("editor is active")
                .backspace_word(),
            KeyCode::Backspace => self.editor.as_mut().expect("editor is active").backspace(),
            KeyCode::Delete => self.editor.as_mut().expect("editor is active").delete(),
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => self
                .editor
                .as_mut()
                .expect("editor is active")
                .backspace_word(),
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => self
                .editor
                .as_mut()
                .expect("editor is active")
                .move_to_visual_edge(false, editor_width),
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => self
                .editor
                .as_mut()
                .expect("editor is active")
                .move_to_visual_edge(true, editor_width),
            KeyCode::Char(character)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                if character == '#' {
                    let target = match self.editor.as_ref().expect("editor is active").target {
                        EditTarget::Existing(id) => TagTarget::Node(id),
                        EditTarget::New { .. } => TagTarget::Draft,
                    };
                    self.open_tag_prompt(target)?;
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

    pub(super) fn start_launcher(&mut self, kind: LauncherKind) -> vrac_engine::Result<()> {
        self.launcher = Some(Launcher::new(kind));
        self.status.clear();
        self.scroll = 0;
        self.refresh_launcher()
    }

    pub(super) fn commit_launcher(&mut self) -> vrac_engine::Result<Action> {
        let item = self
            .launcher
            .as_ref()
            .and_then(|launcher| launcher.items.get(launcher.selected))
            .cloned();
        self.launcher = None;
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

    pub(super) fn run_command(&mut self, command: Command) -> vrac_engine::Result<Action> {
        match command {
            Command::New => self.start_new_sibling(),
            Command::NewBefore => self.start_new_before(),
            Command::NewChild => self.start_new_child()?,
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
            Command::LinesOn => return Ok(Action::SetLines(true)),
            Command::LinesOff => return Ok(Action::SetLines(false)),
            Command::Sync => return Ok(Action::Sync),
            Command::Workspace => return Ok(Action::ChooseWorkspace),
            Command::Quit => return Ok(Action::Quit),
        }
        Ok(Action::Continue)
    }

    pub(super) fn start_tag_prompt(&mut self) -> vrac_engine::Result<()> {
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
        self.open_tag_prompt(TagTarget::Node(target_id))
    }

    pub(super) fn open_tag_prompt(&mut self, target: TagTarget) -> vrac_engine::Result<()> {
        let results = self.engine.tags("", 20)?;
        self.tag_prompt = Some(TagPrompt {
            target,
            query: String::new(),
            results,
            selected: 0,
        });
        self.status.clear();
        self.scroll = 0;
        Ok(())
    }

    pub(super) fn commit_tag_prompt(&mut self) -> vrac_engine::Result<()> {
        let Some(prompt) = self.tag_prompt.take() else {
            return Ok(());
        };
        let Some(tag) = prompt.results.get(prompt.selected).cloned() else {
            self.status = "Type a tag first".into();
            self.tag_prompt = Some(prompt);
            return Ok(());
        };
        let mut tags = match prompt.target {
            TagTarget::Node(id) => {
                self.engine
                    .node(id)?
                    .ok_or(vrac_engine::Error::NodeNotFound(id))?
                    .tags
            }
            TagTarget::Draft => self
                .editor
                .as_ref()
                .expect("a draft tag prompt has an editor")
                .tags
                .clone(),
        };
        let removed = if let Some(index) = tags.iter().position(|existing| existing == &tag) {
            tags.remove(index);
            true
        } else {
            tags.push(tag.clone());
            false
        };
        match prompt.target {
            TagTarget::Node(id) => {
                if let Err(error) = self.engine.set_tags(id, tags.clone()) {
                    self.tag_prompt = Some(prompt);
                    return Err(error);
                }
                self.refresh_cached_node(id)?;
                if let Some(editor) = &mut self.editor {
                    editor.tags = tags;
                }
            }
            TagTarget::Draft => {
                self.editor
                    .as_mut()
                    .expect("a draft tag prompt has an editor")
                    .tags = tags;
            }
        }
        self.status = if removed {
            format!("Removed #{tag}")
        } else {
            format!("Added #{tag}")
        };
        Ok(())
    }

    pub(super) fn start_backlinks(&mut self) -> vrac_engine::Result<()> {
        let target_id = match self.selected.or(self.focus) {
            Some(id) => id,
            None => {
                self.status = "No node is selected".into();
                return Ok(());
            }
        };
        self.backlinks = Some(BacklinkView {
            target_id,
            contexts: Vec::new(),
            next: None,
            selected: 0,
        });
        self.refresh_backlinks()?;
        self.status.clear();
        self.scroll = 0;
        Ok(())
    }

    pub(super) fn commit_reference_prompt(&mut self) {
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
}
