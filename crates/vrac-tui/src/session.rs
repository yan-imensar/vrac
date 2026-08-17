//! Terminal frontend launch and session lifecycle.

use std::error::Error;
use std::io::{self, IsTerminal, Stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind};
use crossterm::execute;
use crossterm::style::{Attribute, ResetColor, SetAttribute};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use vrac_workspace::{OpenedWorkspace, Workspace, configured_folder, remember_folder};

use super::config::Config;
use super::model::{Action, App, SessionExit};
use super::setup::{
    choose_workspace_folder as pick_workspace_folder,
    choose_workspace_folder_with_status as pick_workspace_folder_with_status,
};
use super::ui::draw;

const SYNC_INTERVAL: Duration = Duration::from_secs(2);

/// Context supplied by the product binary when launching the terminal frontend.
#[derive(Debug)]
pub struct LaunchOptions {
    pub data_directory: PathBuf,
    pub workspace: WorkspaceSelection,
}

/// How the terminal frontend chooses its initial workspace.
#[derive(Debug)]
pub enum WorkspaceSelection {
    Remembered,
    Folder(PathBuf),
    Select,
}

/// Runs the terminal frontend with launch context resolved by the product.
pub fn run(options: LaunchOptions) -> Result<(), Box<dyn Error>> {
    let mut folder = choose_workspace_folder(options.workspace, &options.data_directory)?;
    run_with_folder(&options.data_directory, &mut folder)
}

pub(super) fn run_with_folder(
    data_directory: &Path,
    folder: &mut PathBuf,
) -> Result<(), Box<dyn Error>> {
    let mut config = Config::load()?;
    loop {
        let opened = open_workspace(data_directory, folder, |status| {
            if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
                return Ok(None);
            }
            pick_workspace_folder_with_status(status.into())
        })?;
        remember_folder(data_directory, opened.workspace.folder())?;
        match run_workspace(opened, &mut config)? {
            SessionExit::Quit => return Ok(()),
            SessionExit::ChooseWorkspace => {
                if let Some(selected) = pick_workspace_folder()? {
                    *folder = selected;
                }
            }
        }
    }
}

pub(super) fn open_workspace(
    data_directory: &Path,
    folder: &mut PathBuf,
    mut pick_after_error: impl FnMut(&str) -> Result<Option<PathBuf>, Box<dyn Error>>,
) -> Result<OpenedWorkspace, Box<dyn Error>> {
    loop {
        match Workspace::open(folder, data_directory) {
            Ok(opened) => return Ok(opened),
            Err(error) => {
                let status =
                    format!("Current workspace cannot be opened ({error}). Choose another folder.");
                match pick_after_error(&status)? {
                    Some(selected) => *folder = selected,
                    None => return Err(error.into()),
                }
            }
        }
    }
}

pub(super) fn run_workspace(
    opened: OpenedWorkspace,
    config: &mut Config,
) -> Result<SessionExit, Box<dyn Error>> {
    let initial_sync = opened.initial_sync;
    let workspace = opened.workspace;
    let mut app = App::open_with_settings(opened.engine, config.lines, config.backlinks)?;
    if initial_sync.imported > 0 || initial_sync.published > 0 {
        app.status = format!(
            "Synced: {} received, {} sent",
            initial_sync.imported, initial_sync.published
        );
    }
    let mut terminal = TerminalGuard::enter()?;
    let mut next_sync = Instant::now() + SYNC_INTERVAL;
    let mut refresh_after_edit = false;
    let mut quit = false;

    while !quit {
        draw(terminal.stdout(), &mut app)?;
        let timeout = next_sync.saturating_duration_since(Instant::now());
        if event::poll(timeout)? {
            let event = event::read()?;
            next_sync = Instant::now() + SYNC_INTERVAL;
            match event {
                Event::Key(key) if actionable_key(key.kind) => match app.handle_key(key) {
                    Ok(Action::Quit) => quit = true,
                    Ok(Action::Sync) => {
                        sync_workspace(&workspace, &mut app, true, &mut refresh_after_edit)
                    }
                    Ok(Action::ChooseWorkspace) => match workspace.sync(&mut app.engine) {
                        Ok(_) => return Ok(SessionExit::ChooseWorkspace),
                        Err(error) => app.status = format!("Sync error: {error}"),
                    },
                    Ok(Action::SetLines(enabled)) => match config.set_lines(enabled) {
                        Ok(()) => {
                            app.lines = enabled;
                            app.status = format!(
                                "Hierarchy lines {}",
                                if enabled { "enabled" } else { "disabled" }
                            );
                        }
                        Err(error) => app.status = format!("Config error: {error}"),
                    },
                    Ok(Action::SetBacklinks(enabled)) => match config.set_backlinks(enabled) {
                        Ok(()) => match app.set_backlinks_visible(enabled) {
                            Ok(()) => {
                                app.status = format!(
                                    "Contextual backlinks {}",
                                    if enabled { "enabled" } else { "disabled" }
                                );
                            }
                            Err(error) => app.status = error.to_string(),
                        },
                        Err(error) => app.status = format!("Config error: {error}"),
                    },
                    Ok(Action::Continue) => {}
                    Err(error) => app.status = error.to_string(),
                },
                Event::Paste(text) => {
                    if let Err(error) = app.handle_paste(&text) {
                        app.status = error.to_string();
                    }
                }
                Event::Resize(_, _) => {}
                _ => continue,
            }
        }
        if refresh_after_edit && app.editor.is_none() {
            if let Err(error) = app.reload_after_sync() {
                app.status = error.to_string();
            } else {
                refresh_after_edit = false;
            }
        }
        if Instant::now() >= next_sync {
            if app.editor.is_none() {
                sync_workspace(&workspace, &mut app, false, &mut refresh_after_edit);
            }
            next_sync = Instant::now() + SYNC_INTERVAL;
        }
    }

    Ok(SessionExit::Quit)
}

pub(super) fn choose_workspace_folder(
    selection: WorkspaceSelection,
    data_directory: &Path,
) -> Result<PathBuf, Box<dyn Error>> {
    let (folder, may_create) = match selection {
        WorkspaceSelection::Folder(folder) => (folder, true),
        WorkspaceSelection::Remembered => match configured_folder(data_directory)? {
            Some(folder) => (folder, false),
            None => (
                pick_workspace_folder()?.ok_or("workspace selection was cancelled")?,
                false,
            ),
        },
        WorkspaceSelection::Select => (
            pick_workspace_folder()?.ok_or("workspace selection was cancelled")?,
            false,
        ),
    };
    let folder = expand_home(folder)?;
    let folder = if folder.is_absolute() {
        folder
    } else {
        std::env::current_dir()?.join(folder)
    };
    if may_create {
        std::fs::create_dir_all(&folder)?;
    } else if !folder.is_dir() {
        return Err(format!(
            "the configured workspace folder is unavailable: {}\nRun `vrac workspace select` or pass another folder with `vrac --workspace`.",
            folder.display()
        )
        .into());
    }
    Ok(folder.canonicalize()?)
}

pub(super) fn expand_home(path: PathBuf) -> Result<PathBuf, Box<dyn Error>> {
    let Some(value) = path.to_str() else {
        return Ok(path);
    };
    if value == "~" {
        return dirs::home_dir().ok_or_else(|| "cannot determine the home directory".into());
    }
    if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return dirs::home_dir()
            .map(|home| home.join(rest))
            .ok_or_else(|| "cannot determine the home directory".into());
    }
    Ok(path)
}

pub(super) fn sync_workspace(
    workspace: &Workspace,
    app: &mut App,
    explicit: bool,
    refresh_after_edit: &mut bool,
) {
    match workspace.sync(&mut app.engine) {
        Ok(report) => {
            if report.imported > 0 {
                if app.editor.is_some() {
                    *refresh_after_edit = true;
                } else if let Err(error) = app.reload_after_sync() {
                    app.status = error.to_string();
                    return;
                }
            }
            if explicit || report.imported > 0 {
                app.status = format!(
                    "Synced: {} received, {} sent",
                    report.imported, report.published
                );
            }
        }
        Err(error) => app.status = format!("Sync error: {error}"),
    }
}

pub(super) fn actionable_key(kind: KeyEventKind) -> bool {
    matches!(kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

struct TerminalGuard {
    stdout: Stdout,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, Hide) {
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
            DisableBracketedPaste,
            Show
        );
        let _ = execute!(self.stdout, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}
