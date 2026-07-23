use std::fs;
use std::io::{self, IsTerminal, Stdout, Write};
use std::path::{Path, PathBuf};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{
    self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use unicode_width::UnicodeWidthChar;

pub(crate) fn choose_workspace_folder() -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(
            "workspace selection needs an interactive terminal; run `vrac tui <workspace-folder>` to select one non-interactively"
                .into(),
        );
    }
    let home = dirs::home_dir().ok_or("cannot determine the home directory")?;
    let mut picker = FolderPicker::new(known_locations(&home))?;
    let mut terminal = SetupTerminal::enter()?;

    loop {
        picker.draw(terminal.stdout())?;
        match event::read()? {
            Event::Key(key) if actionable_key(key.kind) => match picker.handle_key(key)? {
                PickerAction::Continue => {}
                PickerAction::Selected(path) => return Ok(Some(path)),
                PickerAction::Cancelled => return Ok(None),
            },
            Event::Resize(_, _) => {}
            _ => continue,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Location {
    name: String,
    path: PathBuf,
}

fn known_locations(home: &Path) -> Vec<Location> {
    let mut locations = vec![Location {
        name: "Home".into(),
        path: home.into(),
    }];
    let candidates = [
        (
            "iCloud Drive",
            home.join("Library/Mobile Documents/com~apple~CloudDocs"),
        ),
        ("Dropbox", home.join("Dropbox")),
        ("OneDrive", home.join("OneDrive")),
        ("Syncthing", home.join("Sync")),
        ("Syncthing", home.join("Syncthing")),
    ];
    for (name, path) in candidates {
        push_location(&mut locations, name, path);
    }
    for base in [home.join("Library/CloudStorage"), home.join("CloudStorage")] {
        let Ok(entries) = fs::read_dir(base) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("OneDrive") || name.starts_with("Dropbox") {
                push_location(&mut locations, &name, entry.path());
            }
        }
    }
    locations
}

fn push_location(locations: &mut Vec<Location>, name: &str, path: PathBuf) {
    if path.is_dir() && !locations.iter().any(|location| location.path == path) {
        locations.push(Location {
            name: name.into(),
            path,
        });
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Directory {
    name: String,
    path: PathBuf,
}

struct FolderPicker {
    screen: PickerScreen,
    locations: Vec<Location>,
    location_index: usize,
    status: String,
}

enum PickerScreen {
    Locations,
    Explorer(DirectoryExplorer),
}

struct DirectoryExplorer {
    cwd: PathBuf,
    directories: Vec<Directory>,
    selected: usize,
    scroll: usize,
    show_hidden: bool,
    new_folder: Option<String>,
}

enum PickerAction {
    Continue,
    Selected(PathBuf),
    Cancelled,
}

impl FolderPicker {
    fn new(locations: Vec<Location>) -> io::Result<Self> {
        if locations.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no accessible starting directory is available",
            ));
        }
        Ok(Self {
            screen: PickerScreen::Locations,
            locations,
            location_index: 0,
            status: String::new(),
        })
    }

    fn handle_key(&mut self, key: KeyEvent) -> io::Result<PickerAction> {
        match &mut self.screen {
            PickerScreen::Locations => self.handle_location_key(key),
            PickerScreen::Explorer(explorer) => {
                let action = explorer.handle_key(key, &mut self.status)?;
                if matches!(action, ExplorerAction::Locations) {
                    self.screen = PickerScreen::Locations;
                    self.status.clear();
                    return Ok(PickerAction::Continue);
                }
                Ok(match action {
                    ExplorerAction::Continue | ExplorerAction::Locations => PickerAction::Continue,
                    ExplorerAction::Selected(path) => PickerAction::Selected(path),
                    ExplorerAction::Cancelled => PickerAction::Cancelled,
                })
            }
        }
    }

    fn handle_location_key(&mut self, key: KeyEvent) -> io::Result<PickerAction> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(PickerAction::Cancelled),
            KeyCode::Up | KeyCode::Char('k') => {
                self.location_index = self.location_index.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.location_index =
                    (self.location_index + 1).min(self.locations.len().saturating_sub(1));
            }
            KeyCode::Home | KeyCode::Char('g') => self.location_index = 0,
            KeyCode::End | KeyCode::Char('G') => self.location_index = self.locations.len() - 1,
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                let path = self.locations[self.location_index].path.clone();
                match DirectoryExplorer::open(path, false) {
                    Ok(explorer) => {
                        self.screen = PickerScreen::Explorer(explorer);
                        self.status.clear();
                    }
                    Err(error) => self.status = error.to_string(),
                }
            }
            _ => {}
        }
        Ok(PickerAction::Continue)
    }

    fn draw(&mut self, stdout: &mut Stdout) -> io::Result<()> {
        let (width, height) = terminal::size()?;
        queue!(
            stdout,
            BeginSynchronizedUpdate,
            Hide,
            MoveTo(0, 0),
            Clear(ClearType::All)
        )?;
        match &mut self.screen {
            PickerScreen::Locations => draw_locations(
                stdout,
                &self.locations,
                self.location_index,
                &self.status,
                width,
                height,
            )?,
            PickerScreen::Explorer(explorer) => {
                explorer.draw(stdout, &self.status, width, height)?
            }
        }
        queue!(stdout, ResetColor, SetAttribute(Attribute::Reset))?;
        queue!(stdout, EndSynchronizedUpdate)?;
        stdout.flush()
    }
}

enum ExplorerAction {
    Continue,
    Locations,
    Selected(PathBuf),
    Cancelled,
}

impl DirectoryExplorer {
    fn open(cwd: PathBuf, show_hidden: bool) -> io::Result<Self> {
        Ok(Self {
            directories: read_directories(&cwd, show_hidden)?,
            cwd,
            selected: 0,
            scroll: 0,
            show_hidden,
            new_folder: None,
        })
    }

    fn set_cwd(&mut self, cwd: PathBuf) -> io::Result<()> {
        let directories = read_directories(&cwd, self.show_hidden)?;
        self.cwd = cwd;
        self.directories = directories;
        self.selected = 0;
        self.scroll = 0;
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent, status: &mut String) -> io::Result<ExplorerAction> {
        if self.new_folder.is_some() {
            return self.handle_new_folder_key(key, status);
        }
        status.clear();
        match key.code {
            KeyCode::Char('q') => return Ok(ExplorerAction::Cancelled),
            KeyCode::Esc | KeyCode::Char('b') => return Ok(ExplorerAction::Locations),
            KeyCode::Char(' ') => return Ok(ExplorerAction::Selected(self.cwd.clone())),
            KeyCode::Char('n') => self.new_folder = Some(String::new()),
            KeyCode::Char('.') => {
                let show_hidden = !self.show_hidden;
                match read_directories(&self.cwd, show_hidden) {
                    Ok(directories) => {
                        self.show_hidden = show_hidden;
                        self.directories = directories;
                        self.selected = 0;
                        self.scroll = 0;
                    }
                    Err(error) => *status = error.to_string(),
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => {
                self.selected = self.directories.len().saturating_sub(1)
            }
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => {
                if let Some(parent) = self.cwd.parent()
                    && let Err(error) = self.set_cwd(parent.into())
                {
                    *status = error.to_string();
                }
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(directory) = self.directories.get(self.selected)
                    && let Err(error) = self.set_cwd(directory.path.clone())
                {
                    *status = error.to_string();
                }
            }
            _ => {}
        }
        Ok(ExplorerAction::Continue)
    }

    fn handle_new_folder_key(
        &mut self,
        key: KeyEvent,
        status: &mut String,
    ) -> io::Result<ExplorerAction> {
        let name = self
            .new_folder
            .as_mut()
            .expect("new-folder input is active");
        match key.code {
            KeyCode::Esc => {
                self.new_folder = None;
                status.clear();
            }
            KeyCode::Enter => match valid_new_folder(&self.cwd, name) {
                Ok(path) => match fs::create_dir(&path) {
                    Ok(()) => {
                        self.new_folder = None;
                        match self.set_cwd(path) {
                            Ok(()) => status.clear(),
                            Err(error) => *status = error.to_string(),
                        }
                    }
                    Err(error) => *status = error.to_string(),
                },
                Err(error) => *status = error.to_string(),
            },
            KeyCode::Backspace => {
                name.pop();
                status.clear();
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                name.push(character);
                status.clear();
            }
            _ => {}
        }
        Ok(ExplorerAction::Continue)
    }

    fn move_selection(&mut self, offset: isize) {
        let last = self.directories.len().saturating_sub(1);
        self.selected = if offset.is_negative() {
            self.selected.saturating_sub(offset.unsigned_abs())
        } else {
            self.selected.saturating_add(offset as usize).min(last)
        };
    }

    fn draw(
        &mut self,
        stdout: &mut Stdout,
        status: &str,
        width: u16,
        height: u16,
    ) -> io::Result<()> {
        let width = usize::from(width);
        let height = usize::from(height);
        let existing = self.cwd.join("workspace-id").is_file();
        styled_line(
            stdout,
            0,
            if existing {
                "Open this Vrac workspace"
            } else {
                "Create a Vrac workspace here"
            },
            Color::Cyan,
            true,
            width,
        )?;
        plain_line(stdout, 1, &self.cwd.display().to_string(), width)?;

        let body_start = 3;
        let body_height = height.saturating_sub(body_start + 3);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if body_height > 0 && self.selected >= self.scroll + body_height {
            self.scroll = self.selected + 1 - body_height;
        }
        if self.directories.is_empty() {
            styled_line(
                stdout,
                body_start,
                "  No subfolders",
                Color::DarkGrey,
                false,
                width,
            )?;
        } else {
            for (row, (index, directory)) in self
                .directories
                .iter()
                .enumerate()
                .skip(self.scroll)
                .take(body_height)
                .enumerate()
            {
                queue!(stdout, MoveTo(0, to_u16(body_start + row)))?;
                if index == self.selected {
                    queue!(
                        stdout,
                        SetForegroundColor(Color::Magenta),
                        SetAttribute(Attribute::Bold),
                        Print(fit(&format!("› {}/", directory.name), width)),
                        SetAttribute(Attribute::Reset),
                        ResetColor
                    )?;
                } else {
                    queue!(
                        stdout,
                        SetForegroundColor(Color::Blue),
                        Print(fit(&format!("  {}/", directory.name), width)),
                        ResetColor
                    )?;
                }
            }
        }

        let footer_row = height.saturating_sub(2);
        let footer = if let Some(name) = &self.new_folder {
            format!("New folder: {name}█   Enter create · Esc cancel")
        } else if status.is_empty() {
            "Enter open · Space choose here · n new folder · h parent · b locations · q quit".into()
        } else {
            status.into()
        };
        styled_line(
            stdout,
            footer_row,
            &footer,
            if status.is_empty() {
                Color::DarkGrey
            } else {
                Color::Red
            },
            false,
            width,
        )
    }
}

fn draw_locations(
    stdout: &mut Stdout,
    locations: &[Location],
    selected: usize,
    status: &str,
    width: u16,
    height: u16,
) -> io::Result<()> {
    let width = usize::from(width);
    styled_line(stdout, 0, "Welcome to Vrac", Color::Cyan, true, width)?;
    plain_line(
        stdout,
        1,
        "Choose where the shared workspace should live.",
        width,
    )?;
    styled_line(
        stdout,
        2,
        "The active SQLite database remains private and local.",
        Color::DarkGrey,
        false,
        width,
    )?;
    let body_height = usize::from(height).saturating_sub(6);
    let first = selected.saturating_add(1).saturating_sub(body_height);
    for (row, (index, location)) in locations
        .iter()
        .enumerate()
        .skip(first)
        .take(body_height)
        .enumerate()
    {
        queue!(stdout, MoveTo(0, to_u16(row + 4)))?;
        let line = format!("{}  {}", location.name, location.path.display());
        if index == selected {
            queue!(
                stdout,
                SetForegroundColor(Color::Magenta),
                SetAttribute(Attribute::Bold),
                Print(fit(&format!("› {line}"), width)),
                SetAttribute(Attribute::Reset),
                ResetColor
            )?;
        } else {
            queue!(stdout, Print(fit(&format!("  {line}"), width)))?;
        }
    }
    styled_line(
        stdout,
        usize::from(height).saturating_sub(2),
        if status.is_empty() {
            "↑/↓ choose · Enter browse · q quit"
        } else {
            status
        },
        if status.is_empty() {
            Color::DarkGrey
        } else {
            Color::Red
        },
        false,
        width,
    )
}

fn read_directories(cwd: &Path, show_hidden: bool) -> io::Result<Vec<Directory>> {
    let mut directories = Vec::new();
    for entry in fs::read_dir(cwd)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        directories.push(Directory { name, path });
    }
    directories.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(directories)
}

fn valid_new_folder(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let name = name.trim();
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "enter a simple folder name",
        ));
    }
    let path = parent.join(name);
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a file or folder with that name already exists",
        ));
    }
    Ok(path)
}

fn styled_line(
    stdout: &mut Stdout,
    row: usize,
    text: &str,
    color: Color,
    bold: bool,
    width: usize,
) -> io::Result<()> {
    queue!(stdout, MoveTo(0, to_u16(row)), SetForegroundColor(color))?;
    if bold {
        queue!(stdout, SetAttribute(Attribute::Bold))?;
    }
    queue!(
        stdout,
        Print(fit(text, width)),
        SetAttribute(Attribute::Reset),
        ResetColor
    )
}

fn plain_line(stdout: &mut Stdout, row: usize, text: &str, width: usize) -> io::Result<()> {
    queue!(stdout, MoveTo(0, to_u16(row)), Print(fit(text, width)))
}

fn fit(text: &str, width: usize) -> String {
    let mut result = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = character.width().unwrap_or_default();
        if used + character_width > width {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result
}

fn to_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn actionable_key(kind: KeyEventKind) -> bool {
    matches!(kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

struct SetupTerminal {
    stdout: Stdout,
}

impl SetupTerminal {
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

impl Drop for SetupTerminal {
    fn drop(&mut self) {
        let _ = execute!(
            self.stdout,
            ResetColor,
            SetAttribute(Attribute::Reset),
            Show,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_include_home_and_only_existing_provider_folders() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("Dropbox")).unwrap();

        let locations = known_locations(root.path());

        assert_eq!(locations[0].name, "Home");
        assert!(locations.iter().any(|location| location.name == "Dropbox"));
        assert!(!locations.iter().any(|location| location.name == "OneDrive"));
    }

    #[test]
    fn new_folder_names_cannot_escape_the_current_directory() {
        let root = tempfile::tempdir().unwrap();
        assert!(valid_new_folder(root.path(), "Vrac").is_ok());
        for name in ["", ".", "..", "../Vrac", "one/two", "one\\two"] {
            assert!(valid_new_folder(root.path(), name).is_err(), "{name}");
        }
    }

    #[test]
    fn directory_listing_is_sorted_and_excludes_files_and_hidden_folders() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("zeta")).unwrap();
        fs::create_dir(root.path().join("Alpha")).unwrap();
        fs::create_dir(root.path().join(".hidden")).unwrap();
        fs::write(root.path().join("note.txt"), "note").unwrap();

        let visible = read_directories(root.path(), false).unwrap();
        assert_eq!(
            visible
                .iter()
                .map(|directory| directory.name.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "zeta"]
        );
        let all = read_directories(root.path(), true).unwrap();
        assert!(all.iter().any(|directory| directory.name == ".hidden"));
        assert!(!all.iter().any(|directory| directory.name == "note.txt"));
    }
}
