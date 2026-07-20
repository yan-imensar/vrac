use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use rusqlite::{Connection, MAIN_DB, OpenFlags};
use tempfile::NamedTempFile;

use crate::db::Engine;
use crate::schema::prepare_database;
use crate::sync::{capture_session, commit_mutation};
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

    /// Restores canonical workspace content from a validated checkpoint.
    ///
    /// `recovery` receives a complete checkpoint of the current state before
    /// any content changes. The restored content is committed atomically as a
    /// normal mutation, so a synchronized engine can publish the restoration
    /// to its other devices. Both checkpoints must belong to this workspace.
    ///
    /// This is a background operation proportional to workspace size.
    pub fn restore_checkpoint(
        &mut self,
        checkpoint: impl AsRef<Path>,
        recovery: impl AsRef<Path>,
    ) -> Result<()> {
        let checkpoint = checkpoint.as_ref();
        let active_path: String = self.connection.query_row(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
            [],
            |row| row.get(0),
        )?;
        if !active_path.is_empty()
            && fs::canonicalize(checkpoint)? == fs::canonicalize(active_path)?
        {
            return Err(Error::RestoreSourceIsActiveWorkspace);
        }
        let candidate = checkpoint_engine(checkpoint)?;
        let report = candidate.check()?;
        if !report.is_ok() {
            return Err(Error::InvalidCheckpoint(report));
        }
        if candidate.workspace_id()? != self.workspace_id()? {
            return Err(Error::CheckpointWorkspaceMismatch);
        }
        drop(candidate);

        self.checkpoint(recovery)?;
        let path = checkpoint.to_str().ok_or_else(|| {
            Error::Io(std::io::Error::new(
                ErrorKind::InvalidInput,
                "checkpoint path is not valid Unicode",
            ))
        })?;
        self.connection
            .execute("ATTACH DATABASE ?1 AS restore_source", [path])?;
        let restored = (|| {
            let captured = capture_session(&self.connection)?;
            let transaction = self.connection.unchecked_transaction()?;
            transaction.pragma_update(None, "defer_foreign_keys", true)?;
            transaction.execute("DELETE FROM node_references", [])?;
            transaction.execute("DELETE FROM node_tags", [])?;
            transaction.execute("DELETE FROM nodes", [])?;
            transaction.execute(
                "INSERT INTO nodes (id, parent_id, position, text, system_key)
                 SELECT id, parent_id, position, text, system_key FROM restore_source.nodes",
                [],
            )?;
            transaction.execute(
                "INSERT INTO node_tags (node_id, tag)
                 SELECT node_id, tag FROM restore_source.node_tags",
                [],
            )?;
            transaction.execute(
                "INSERT INTO node_references (source_id, start_byte, end_byte, target_id)
                 SELECT source_id, start_byte, end_byte, target_id
                 FROM restore_source.node_references",
                [],
            )?;
            commit_mutation(transaction, captured, self.sync_device_id).map(drop)
        })();
        let detached = self
            .connection
            .execute_batch("DETACH DATABASE restore_source");
        restored?;
        self.history.clear();
        detached?;
        Ok(())
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
    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("delete") {
        return Err(Error::StorageConfiguration(format!(
            "SQLite could not make the checkpoint standalone: selected journal mode {journal_mode:?}"
        )));
    }
    Ok(())
}

fn validate_checkpoint(path: &Path) -> Result<crate::CheckReport> {
    checkpoint_engine(path)?.check()
}

fn checkpoint_engine(path: &Path) -> Result<Engine> {
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    prepare_database(&mut connection)?;
    Ok(Engine {
        connection,
        sync_device_id: None,
        history: Default::default(),
    })
}
