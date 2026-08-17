//! Local workspace lifecycle and synchronization for Vrac clients.
//!
//! This crate keeps the active SQLite database on local storage while a
//! provider folder contains only durable checkpoints and immutable sync
//! packages. It performs no user interaction and contains no presentation
//! logic.

#![deny(missing_docs)]

use std::error::Error as StdError;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use vrac_engine::{
    Engine, Error as EngineError, OutgoingSyncPackage, SyncApply, SyncDeviceId, WorkspaceId,
};

const WORKSPACE_ID_FILE: &str = "workspace-id";
const CHECKPOINT_FILE: &str = "checkpoint.vrac";
const CHANGES_DIRECTORY: &str = "changes";
const CONFIG_FILE: &str = "workspace-folder";

/// A local engine paired with its validated provider workspace.
pub struct OpenedWorkspace {
    /// Active engine backed by a database in local application data.
    pub engine: Engine,
    /// Provider workspace used to exchange immutable synchronization packages.
    pub workspace: Workspace,
    /// Synchronization performed while opening the workspace.
    pub initial_sync: SyncReport,
}

/// A validated provider folder associated with one Vrac workspace.
pub struct Workspace {
    folder: PathBuf,
}

/// Outcome of one synchronization round.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SyncReport {
    /// Packages imported into the active local database.
    pub imported: usize,
    /// Packages published to the provider folder.
    pub published: usize,
}

/// Error returned while opening, configuring, or synchronizing a workspace.
#[derive(Debug)]
pub enum Error {
    /// Failure reported by the storage engine.
    Engine(EngineError),
    /// Failure reported by the local filesystem.
    Io(std::io::Error),
    /// Invalid or inconsistent workspace configuration.
    InvalidWorkspace(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
            Self::InvalidWorkspace(message) => formatter.write_str(message),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Engine(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::InvalidWorkspace(_) => None,
        }
    }
}

impl From<EngineError> for Error {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::InvalidWorkspace(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::InvalidWorkspace(message.into())
    }
}

/// Result returned by workspace operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Reads the previously selected provider folder from local application data.
pub fn configured_folder(data_directory: &Path) -> Result<Option<PathBuf>> {
    let path = data_directory.join(CONFIG_FILE);
    let value = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let value = value.strip_suffix('\n').unwrap_or(&value);
    folder_path(value).map(Some)
}

/// Atomically remembers the selected provider folder in local application data.
pub fn remember_folder(data_directory: &Path, folder: &Path) -> Result<()> {
    fs::create_dir_all(data_directory)?;
    let value = folder
        .to_str()
        .ok_or_else(|| "the workspace folder path is not valid Unicode".to_string())?;
    if value.contains(['\n', '\r']) {
        return Err("the workspace folder path contains an unsupported line break".into());
    }
    atomic_write(
        &data_directory.join(CONFIG_FILE),
        format!("{value}\n").as_bytes(),
    )
}

impl Workspace {
    /// Opens or creates a workspace and performs its initial synchronization.
    pub fn open(folder: &Path, data_directory: &Path) -> Result<OpenedWorkspace> {
        let folder = folder.canonicalize()?;
        if !folder.is_dir() {
            return Err("the workspace path is not a directory".into());
        }

        fs::create_dir_all(data_directory.join("workspaces"))?;
        let local_data = data_directory.canonicalize()?;
        if folder.starts_with(&local_data) || local_data.starts_with(&folder) {
            return Err(
                "the selected workspace folder must be separate from Vrac's local application data"
                    .into(),
            );
        }
        let device_id = load_device_id(data_directory)?;
        let workspace_id = if folder.join(WORKSPACE_ID_FILE).exists() {
            read_provider_id(&folder)?
        } else {
            create_workspace(&folder, data_directory, device_id)?
        };
        validate_provider(&folder, workspace_id)?;

        let database = local_database(data_directory, workspace_id);
        if !database.is_file() {
            install_checkpoint(&folder, &database, workspace_id, device_id)?;
        }
        let mut engine = Engine::open_synced(&database, device_id)?;
        if engine.workspace_id()? != workspace_id {
            return Err("the local database belongs to another workspace".into());
        }

        let workspace = Self { folder };
        let initial_sync = workspace.sync(&mut engine)?;
        Ok(OpenedWorkspace {
            engine,
            workspace,
            initial_sync,
        })
    }

    /// Returns the provider folder associated with this workspace.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Imports and publishes every currently available synchronization package.
    pub fn sync(&self, engine: &mut Engine) -> Result<SyncReport> {
        let workspace_id = engine.workspace_id()?;
        validate_provider(&self.folder, workspace_id)?;
        let changes = self.folder.join(CHANGES_DIRECTORY);
        Ok(SyncReport {
            imported: import_packages(engine, &changes)?,
            published: publish_packages(engine, &changes)?,
        })
    }
}

fn create_workspace(
    folder: &Path,
    data_directory: &Path,
    device_id: SyncDeviceId,
) -> Result<WorkspaceId> {
    for name in [CHECKPOINT_FILE, CHANGES_DIRECTORY] {
        if folder.join(name).exists() {
            return Err("the selected folder contains incomplete Vrac workspace data".into());
        }
    }

    let partial = data_directory.join("workspaces").join("new.partial");
    if partial.exists() {
        return Err("an incomplete local workspace creation already exists".into());
    }
    let candidate = Engine::open_synced(&partial, device_id)?;
    let workspace_id = candidate.workspace_id()?;
    let destination = local_database(data_directory, workspace_id);
    if destination.exists() {
        drop(candidate);
        let _ = fs::remove_file(&partial);
        return Err("the local workspace already exists".into());
    }
    let created = create_provider(folder, workspace_id, |path| {
        candidate.checkpoint(path)?;
        Ok(())
    });
    drop(candidate);
    if let Err(error) = created {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    fs::rename(partial, destination)?;
    Ok(workspace_id)
}

fn create_provider(
    folder: &Path,
    workspace_id: WorkspaceId,
    checkpoint: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let checkpoint_path = folder.join(CHECKPOINT_FILE);
    let changes = folder.join(CHANGES_DIRECTORY);
    let identity = folder.join(WORKSPACE_ID_FILE);
    if checkpoint_path.exists() || changes.exists() || identity.exists() {
        return Err("the selected folder already contains Vrac workspace data".into());
    }

    fs::create_dir(&changes)?;
    let partial = folder.join("checkpoint.partial");
    let result = (|| {
        checkpoint(&partial)?;
        fs::rename(&partial, &checkpoint_path)?;
        atomic_write(&identity, workspace_id.to_string().as_bytes())
    })();
    if result.is_err() {
        let _ = fs::remove_file(partial);
        let _ = fs::remove_file(checkpoint_path);
        let _ = fs::remove_file(identity);
        let _ = fs::remove_dir(changes);
    }
    result
}

fn install_checkpoint(
    folder: &Path,
    destination: &Path,
    workspace_id: WorkspaceId,
    device_id: SyncDeviceId,
) -> Result<()> {
    let source = folder.join(CHECKPOINT_FILE);
    if !source.is_file() {
        return Err("the workspace folder has no usable checkpoint".into());
    }
    let partial = destination.with_extension("partial");
    if partial.exists() {
        return Err("an incomplete local workspace import already exists".into());
    }
    copy_durable(&source, &partial)?;
    let candidate = match Engine::open_synced(&partial, device_id) {
        Ok(candidate) => candidate,
        Err(error) => {
            let _ = fs::remove_file(&partial);
            return Err(error.into());
        }
    };
    let valid_identity = candidate
        .workspace_id()
        .is_ok_and(|candidate_id| candidate_id == workspace_id);
    let report = candidate.check();
    drop(candidate);
    match report {
        Ok(report) if report.is_ok() && valid_identity => {}
        Ok(_) => {
            let _ = fs::remove_file(&partial);
            return Err("the workspace checkpoint failed integrity validation".into());
        }
        Err(error) => {
            let _ = fs::remove_file(&partial);
            return Err(error.into());
        }
    }
    fs::rename(partial, destination)?;
    Ok(())
}

fn validate_provider(folder: &Path, workspace_id: WorkspaceId) -> Result<()> {
    let provider_id = read_provider_id(folder);
    if !folder.is_dir()
        || !folder.join(CHECKPOINT_FILE).is_file()
        || !folder.join(CHANGES_DIRECTORY).is_dir()
        || provider_id.is_err()
        || provider_id? != workspace_id
    {
        return Err("the workspace folder is incomplete or has a different identity".into());
    }
    Ok(())
}

fn read_provider_id(folder: &Path) -> Result<WorkspaceId> {
    fs::read_to_string(folder.join(WORKSPACE_ID_FILE))?
        .trim()
        .parse()
        .map_err(|error: vrac_engine::ParseWorkspaceIdError| error.to_string().into())
}

fn local_database(data_directory: &Path, workspace_id: WorkspaceId) -> PathBuf {
    data_directory
        .join("workspaces")
        .join(format!("{workspace_id}.vrac"))
}

fn load_device_id(data_directory: &Path) -> Result<SyncDeviceId> {
    let path = data_directory.join("device-id");
    if path.is_file() {
        return parse_device_id(fs::read_to_string(path)?.trim());
    }
    let id = SyncDeviceId::generate()?;
    atomic_write(&path, id.to_string().as_bytes())?;
    Ok(id)
}

fn parse_device_id(value: &str) -> Result<SyncDeviceId> {
    if value.len() != 32 {
        return Err("the local synchronization device identifier is invalid".into());
    }
    let mut bytes = [0_u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let pair = &value.as_bytes()[index * 2..index * 2 + 2];
        *byte = u8::from_str_radix(
            std::str::from_utf8(pair)
                .map_err(|_| "the local synchronization device identifier is invalid")?,
            16,
        )
        .map_err(|_| "the local synchronization device identifier is invalid")?;
    }
    Ok(SyncDeviceId::from_bytes(bytes))
}

fn import_packages(engine: &mut Engine, provider: &Path) -> Result<usize> {
    let mut pending: Vec<PathBuf> = fs::read_dir(provider)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("vrac-sync"))
        .collect();
    pending.sort();

    let mut imported = 0;
    while !pending.is_empty() {
        let count_before = pending.len();
        let mut deferred = Vec::new();
        let mut dependency_error = None;
        for path in pending {
            let bytes = fs::read(&path)?;
            match engine.apply_sync_package(&bytes) {
                Ok(SyncApply::Applied) => imported += 1,
                Ok(SyncApply::AlreadyApplied) => {}
                Err(EngineError::SyncDependencyMissing { .. }) => {
                    dependency_error =
                        Some("a sync package is waiting for an earlier package".to_string());
                    deferred.push(path);
                }
                Err(error) => return Err(error.into()),
            }
        }
        if deferred.len() == count_before {
            return Err(dependency_error
                .unwrap_or_else(|| "synchronization packages could not make progress".to_string())
                .into());
        }
        pending = deferred;
    }
    Ok(imported)
}

fn publish_packages(engine: &mut Engine, provider: &Path) -> Result<usize> {
    let mut published = 0;
    while let Some(package) = engine.next_sync_package()? {
        publish_package(provider, &package)?;
        engine.confirm_sync_package(&package)?;
        published += 1;
    }
    Ok(published)
}

fn publish_package(provider: &Path, package: &OutgoingSyncPackage) -> Result<()> {
    let destination = provider.join(package.file_name());
    if destination.exists() {
        if fs::read(&destination)? == package.bytes() {
            return Ok(());
        }
        return Err(format!(
            "the immutable sync package {} has different contents",
            package.file_name()
        )
        .into());
    }
    let partial = destination.with_extension("partial");
    if partial.exists() {
        if fs::read(&partial)? != package.bytes() {
            return Err("an incomplete sync package has different contents".into());
        }
    } else {
        write_new(&partial, package.bytes())?;
    }
    fs::rename(partial, destination)?;
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let partial = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&partial)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(partial, path)?;
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn copy_durable(source: &Path, destination: &Path) -> Result<()> {
    let mut source = fs::File::open(source)?;
    let mut destination = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    std::io::copy(&mut source, &mut destination)?;
    destination.sync_all()?;
    Ok(())
}

fn folder_path(value: &str) -> Result<PathBuf> {
    if value.contains(['\n', '\r']) {
        return Err("the workspace folder path contains an unsupported line break".into());
    }
    let folder = PathBuf::from(value);
    if !folder.is_absolute() {
        return Err("the configured workspace folder path must be absolute".into());
    }
    Ok(folder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vrac_engine::{CreateNode, Page, Placement};

    fn create_root(engine: &mut Engine, text: &str) {
        let mut input = CreateNode::new(text);
        input.placement = Placement::Last;
        engine.create_node(input).expect("create root");
    }

    #[test]
    fn configuration_round_trips_an_absolute_folder() {
        let data = tempfile::tempdir().expect("create data directory");
        let folder = tempfile::tempdir().expect("create workspace folder");
        assert_eq!(configured_folder(data.path()).unwrap(), None);
        remember_folder(data.path(), folder.path()).expect("remember folder");
        assert_eq!(
            configured_folder(data.path()).unwrap().as_deref(),
            Some(folder.path())
        );
    }

    #[test]
    fn two_installations_exchange_changes_through_the_provider_folder() {
        let provider = tempfile::tempdir().expect("create provider directory");
        let data_a = tempfile::tempdir().expect("create first data directory");
        let data_b = tempfile::tempdir().expect("create second data directory");

        let mut first = Workspace::open(provider.path(), data_a.path()).expect("open first");
        create_root(&mut first.engine, "from A");
        assert_eq!(
            first.workspace.sync(&mut first.engine).unwrap().published,
            1
        );

        let mut second = Workspace::open(provider.path(), data_b.path()).expect("open second");
        let texts: Vec<_> = second
            .engine
            .children(None, Page::default())
            .unwrap()
            .nodes
            .into_iter()
            .map(|node| node.text)
            .collect();
        assert!(texts.iter().any(|text| text == "from A"));

        create_root(&mut second.engine, "from B");
        second.workspace.sync(&mut second.engine).unwrap();
        assert_eq!(first.workspace.sync(&mut first.engine).unwrap().imported, 1);
        let texts: Vec<_> = first
            .engine
            .children(None, Page::default())
            .unwrap()
            .nodes
            .into_iter()
            .map(|node| node.text)
            .collect();
        assert!(texts.iter().any(|text| text == "from B"));
    }

    #[test]
    fn an_unmanaged_local_database_is_ignored_when_creating_a_workspace() {
        let provider = tempfile::tempdir().expect("create provider directory");
        let data = tempfile::tempdir().expect("create data directory");
        let unmanaged = data.path().join("vrac.vrac");
        let mut unmanaged_engine = Engine::open(&unmanaged).expect("open unmanaged database");
        create_root(&mut unmanaged_engine, "kept note");
        drop(unmanaged_engine);

        let opened = Workspace::open(provider.path(), data.path()).expect("attach workspace");
        let texts: Vec<_> = opened
            .engine
            .children(None, Page::default())
            .unwrap()
            .nodes
            .into_iter()
            .map(|node| node.text)
            .collect();
        assert!(!texts.iter().any(|text| text == "kept note"));
        assert!(unmanaged.is_file());
    }

    #[test]
    fn incomplete_provider_data_is_not_overwritten() {
        let provider = tempfile::tempdir().expect("create provider directory");
        let data = tempfile::tempdir().expect("create data directory");
        fs::write(provider.path().join(CHECKPOINT_FILE), b"not a database")
            .expect("write existing checkpoint");

        assert!(Workspace::open(provider.path(), data.path()).is_err());
        assert_eq!(
            fs::read(provider.path().join(CHECKPOINT_FILE)).unwrap(),
            b"not a database"
        );
    }

    #[test]
    fn provider_data_cannot_contain_the_active_local_database() {
        let data = tempfile::tempdir().expect("create data directory");
        let provider = data.path().join("provider");
        fs::create_dir(&provider).expect("create provider directory");

        assert!(Workspace::open(&provider, data.path()).is_err());
        assert!(!provider.join(WORKSPACE_ID_FILE).exists());
    }

    #[test]
    fn invalid_unicode_device_ids_are_rejected() {
        assert!(parse_device_id(&"é".repeat(16)).is_err());
    }
}
