use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use rusqlite::{Connection, MAIN_DB, OpenFlags};
use tempfile::NamedTempFile;

use crate::db::Engine;
use crate::schema::prepare_database;
use crate::{Error, Result};

impl Engine {
    /// Creates a validated, standalone checkpoint at a new path.
    ///
    /// The active database is copied through SQLite's online backup API. The
    /// copy is checked completely before it becomes visible at `destination`.
    /// An existing destination is never overwritten. This is a background
    /// operation whose cost is proportional to the complete workspace size.
    ///
    /// # Example
    ///
    /// ```
    /// let directory = tempfile::tempdir()?;
    /// let checkpoint = directory.path().join("checkpoint.vrac");
    /// let engine = vrac::Engine::open(":memory:")?;
    /// engine.checkpoint(&checkpoint)?;
    /// assert!(vrac::Engine::open(checkpoint)?.check()?.is_ok());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn checkpoint(&self, destination: impl AsRef<Path>) -> Result<()> {
        let destination = destination.as_ref();
        if destination.exists() {
            return Err(Error::CheckpointDestinationExists);
        }

        if self.sync_device_id.is_some() {
            self.next_sync_package()?;
        }

        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let temporary = NamedTempFile::new_in(parent)?.into_temp_path();
        self.connection.backup(MAIN_DB, &temporary, None)?;
        remove_local_sync_queue(&temporary)?;

        let report = validate_checkpoint(&temporary)?;
        if !report.is_ok() {
            return Err(Error::InvalidCheckpoint(report));
        }

        match fs::hard_link(&temporary, destination) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                Err(Error::CheckpointDestinationExists)
            }
            Err(error) => Err(error.into()),
        }
    }
}

fn remove_local_sync_queue(path: &Path) -> Result<()> {
    let mut connection = Connection::open(path)?;
    let transaction = connection.transaction()?;
    transaction.execute(
        "UPDATE sync_devices
         SET applied_sequence = (
                 SELECT MAX(outbox.sequence)
                 FROM sync_outbox AS outbox
                 WHERE outbox.device_id = sync_devices.device_id
             ),
             applied_package = NULL
         WHERE EXISTS (
             SELECT 1 FROM sync_outbox AS outbox
             WHERE outbox.device_id = sync_devices.device_id
         )",
        [],
    )?;
    transaction.execute("DELETE FROM sync_batch", [])?;
    transaction.execute("DELETE FROM sync_outbox", [])?;
    transaction.execute("UPDATE sync_devices SET next_sequence = NULL", [])?;
    transaction.commit()?;
    Ok(())
}

fn validate_checkpoint(path: &Path) -> Result<crate::CheckReport> {
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    prepare_database(&mut connection)?;
    Engine {
        connection,
        sync_device_id: None,
    }
    .check()
}
