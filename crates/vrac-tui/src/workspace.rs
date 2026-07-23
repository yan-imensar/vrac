use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use vrac::{Engine, Error, OutgoingSyncPackage, SyncApply, SyncDeviceId, WorkspaceId};

const WORKSPACE_ID_FILE: &str = "workspace-id";
const CHECKPOINT_FILE: &str = "checkpoint.vrac";
const CHANGES_DIRECTORY: &str = "changes";
const CONFIG_FILE: &str = "workspace-folder";
const LEGACY_DATABASE: &str = "vrac.vrac";

pub(crate) struct OpenedWorkspace {
    pub(crate) engine: Engine,
    pub(crate) workspace: Workspace,
    pub(crate) initial_sync: SyncReport,
}

pub(crate) struct Workspace {
    folder: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SyncReport {
    pub(crate) imported: usize,
    pub(crate) published: usize,
}

pub(crate) fn configured_folder(data_directory: &Path) -> Result<Option<PathBuf>, String> {
    let path = data_directory.join(CONFIG_FILE);
    let value = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let value = value.strip_suffix('\n').unwrap_or(&value);
    folder_path(value).map(Some)
}

pub(crate) fn remember_folder(data_directory: &Path, folder: &Path) -> Result<(), String> {
    fs::create_dir_all(data_directory).map_err(io_error)?;
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
    pub(crate) fn open(folder: &Path, data_directory: &Path) -> Result<OpenedWorkspace, String> {
        let folder = folder.canonicalize().map_err(io_error)?;
        if !folder.is_dir() {
            return Err("the workspace path is not a directory".into());
        }

        fs::create_dir_all(data_directory.join("workspaces")).map_err(io_error)?;
        let local_data = data_directory.canonicalize().map_err(io_error)?;
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
        let mut engine = Engine::open_synced(&database, device_id).map_err(engine_error)?;
        if engine.workspace_id().map_err(engine_error)? != workspace_id {
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

    pub(crate) fn folder(&self) -> &Path {
        &self.folder
    }

    pub(crate) fn sync(&self, engine: &mut Engine) -> Result<SyncReport, String> {
        let workspace_id = engine.workspace_id().map_err(engine_error)?;
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
) -> Result<WorkspaceId, String> {
    for name in [CHECKPOINT_FILE, CHANGES_DIRECTORY] {
        if folder.join(name).exists() {
            return Err("the selected folder contains incomplete Vrac workspace data".into());
        }
    }

    let legacy = data_directory.join(LEGACY_DATABASE);
    if legacy.is_file() {
        let engine = Engine::open_synced(&legacy, device_id).map_err(engine_error)?;
        let workspace_id = engine.workspace_id().map_err(engine_error)?;
        create_provider(folder, workspace_id, |path| {
            engine.checkpoint(path).map_err(engine_error)
        })?;
        drop(engine);
        let destination = local_database(data_directory, workspace_id);
        if !destination.is_file() {
            install_checkpoint(folder, &destination, workspace_id, device_id)?;
        }
        return Ok(workspace_id);
    }

    let partial = data_directory.join("workspaces").join("new.partial");
    if partial.exists() {
        return Err("an incomplete local workspace creation already exists".into());
    }
    let candidate = Engine::open_synced(&partial, device_id).map_err(engine_error)?;
    let workspace_id = candidate.workspace_id().map_err(engine_error)?;
    let destination = local_database(data_directory, workspace_id);
    if destination.exists() {
        drop(candidate);
        let _ = fs::remove_file(&partial);
        return Err("the local workspace already exists".into());
    }
    let created = create_provider(folder, workspace_id, |path| {
        candidate.checkpoint(path).map_err(engine_error)
    });
    drop(candidate);
    if let Err(error) = created {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    fs::rename(partial, destination).map_err(io_error)?;
    Ok(workspace_id)
}

fn create_provider(
    folder: &Path,
    workspace_id: WorkspaceId,
    checkpoint: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let checkpoint_path = folder.join(CHECKPOINT_FILE);
    let changes = folder.join(CHANGES_DIRECTORY);
    let identity = folder.join(WORKSPACE_ID_FILE);
    if checkpoint_path.exists() || changes.exists() || identity.exists() {
        return Err("the selected folder already contains Vrac workspace data".into());
    }

    fs::create_dir(&changes).map_err(io_error)?;
    let partial = folder.join("checkpoint.partial");
    let result = (|| {
        checkpoint(&partial)?;
        fs::rename(&partial, &checkpoint_path).map_err(io_error)?;
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
) -> Result<(), String> {
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
            return Err(error.to_string());
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
            return Err(error.to_string());
        }
    }
    fs::rename(partial, destination).map_err(io_error)
}

fn validate_provider(folder: &Path, workspace_id: WorkspaceId) -> Result<(), String> {
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

fn read_provider_id(folder: &Path) -> Result<WorkspaceId, String> {
    fs::read_to_string(folder.join(WORKSPACE_ID_FILE))
        .map_err(io_error)?
        .trim()
        .parse()
        .map_err(|error: vrac::ParseWorkspaceIdError| error.to_string())
}

fn local_database(data_directory: &Path, workspace_id: WorkspaceId) -> PathBuf {
    data_directory
        .join("workspaces")
        .join(format!("{workspace_id}.vrac"))
}

fn load_device_id(data_directory: &Path) -> Result<SyncDeviceId, String> {
    let path = data_directory.join("device-id");
    if path.is_file() {
        return parse_device_id(fs::read_to_string(path).map_err(io_error)?.trim());
    }
    let id = SyncDeviceId::generate().map_err(engine_error)?;
    atomic_write(&path, id.to_string().as_bytes())?;
    Ok(id)
}

fn parse_device_id(value: &str) -> Result<SyncDeviceId, String> {
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

fn import_packages(engine: &mut Engine, provider: &Path) -> Result<usize, String> {
    let mut pending: Vec<PathBuf> = fs::read_dir(provider)
        .map_err(io_error)?
        .map(|entry| entry.map(|entry| entry.path()).map_err(io_error))
        .collect::<Result<Vec<_>, _>>()?
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
            let bytes = fs::read(&path).map_err(io_error)?;
            match engine.apply_sync_package(&bytes) {
                Ok(SyncApply::Applied) => imported += 1,
                Ok(SyncApply::AlreadyApplied) => {}
                Err(Error::SyncDependencyMissing { .. }) => {
                    dependency_error =
                        Some("a sync package is waiting for an earlier package".to_string());
                    deferred.push(path);
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        if deferred.len() == count_before {
            return Err(dependency_error.unwrap_or_else(|| {
                "synchronization packages could not make progress".to_string()
            }));
        }
        pending = deferred;
    }
    Ok(imported)
}

fn publish_packages(engine: &mut Engine, provider: &Path) -> Result<usize, String> {
    let mut published = 0;
    while let Some(package) = engine.next_sync_package().map_err(engine_error)? {
        publish_package(provider, &package)?;
        engine
            .confirm_sync_package(&package)
            .map_err(engine_error)?;
        published += 1;
    }
    Ok(published)
}

fn publish_package(provider: &Path, package: &OutgoingSyncPackage) -> Result<(), String> {
    let destination = provider.join(package.file_name());
    if destination.exists() {
        if fs::read(&destination).map_err(io_error)? == package.bytes() {
            return Ok(());
        }
        return Err(format!(
            "the immutable sync package {} has different contents",
            package.file_name()
        ));
    }
    let partial = destination.with_extension("partial");
    if partial.exists() {
        if fs::read(&partial).map_err(io_error)? != package.bytes() {
            return Err("an incomplete sync package has different contents".into());
        }
    } else {
        write_new(&partial, package.bytes())?;
    }
    fs::rename(partial, destination).map_err(io_error)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let partial = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&partial)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    drop(file);
    fs::rename(partial, path).map_err(io_error)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

fn copy_durable(source: &Path, destination: &Path) -> Result<(), String> {
    let mut source = fs::File::open(source).map_err(io_error)?;
    let mut destination = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(io_error)?;
    std::io::copy(&mut source, &mut destination).map_err(io_error)?;
    destination.sync_all().map_err(io_error)
}

fn folder_path(value: &str) -> Result<PathBuf, String> {
    if value.contains(['\n', '\r']) {
        return Err("the workspace folder path contains an unsupported line break".into());
    }
    let folder = PathBuf::from(value);
    if !folder.is_absolute() {
        return Err("the configured workspace folder path must be absolute".into());
    }
    Ok(folder)
}

fn engine_error(error: vrac::Error) -> String {
    error.to_string()
}

fn io_error(error: std::io::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vrac::{CreateNode, Page, Placement};

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
    fn an_existing_local_database_is_attached_without_losing_its_nodes() {
        let provider = tempfile::tempdir().expect("create provider directory");
        let data = tempfile::tempdir().expect("create data directory");
        let mut legacy = Engine::open(data.path().join(LEGACY_DATABASE)).expect("open legacy");
        create_root(&mut legacy, "kept note");
        drop(legacy);

        let opened = Workspace::open(provider.path(), data.path()).expect("attach workspace");
        let texts: Vec<_> = opened
            .engine
            .children(None, Page::default())
            .unwrap()
            .nodes
            .into_iter()
            .map(|node| node.text)
            .collect();
        assert!(texts.iter().any(|text| text == "kept note"));
        assert!(data.path().join(LEGACY_DATABASE).is_file());
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
