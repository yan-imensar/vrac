use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fallible_streaming_iterator::FallibleStreamingIterator;
use rusqlite::session::{Changegroup, ChangesetIter, ConflictAction, Session};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::content::reference_range_is_valid;
use crate::db::Engine;
use crate::nodes::{decode_id, node_id_bytes};
use crate::{Error, OutgoingSyncPackage, Result, SYNC_DEVICE_ID_LENGTH, SyncApply, SyncDeviceId};

const PACKAGE_MAGIC: &[u8; 8] = b"VRACSYNC";
const PACKAGE_VERSION: u8 = 1;
const PACKAGE_HEADER_LENGTH: usize = 65;
const PACKAGE_HASH_LENGTH: usize = 32;
const CAPTURED_TABLES: [&str; 3] = ["nodes", "node_tags", "node_references"];

struct ParsedPackage<'a> {
    workspace_id: [u8; 16],
    device_id: SyncDeviceId,
    first_sequence: u64,
    last_sequence: u64,
    id: [u8; PACKAGE_HASH_LENGTH],
    payload: &'a [u8],
}

pub(crate) fn register_device(connection: &mut Connection, device_id: SyncDeviceId) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO sync_devices (device_id, next_sequence)
         VALUES (?1, 1)
         ON CONFLICT(device_id) DO UPDATE SET
             next_sequence = COALESCE(sync_devices.next_sequence,
                                      sync_devices.applied_sequence + 1)",
        params![device_id.as_bytes().as_slice()],
    )?;
    transaction.commit()?;
    Ok(())
}

pub(crate) fn capture_session(
    connection: &Connection,
    device_id: Option<SyncDeviceId>,
) -> Result<Option<Session<'_>>> {
    if device_id.is_none() {
        return Ok(None);
    }
    let mut session = Session::new(connection)?;
    for table in CAPTURED_TABLES {
        session.attach(Some(table))?;
    }
    Ok(Some(session))
}

pub(crate) fn commit_mutation(
    transaction: Transaction<'_>,
    mut session: Option<Session<'_>>,
    device_id: Option<SyncDeviceId>,
) -> Result<()> {
    if let (Some(session), Some(device_id)) = (session.as_mut(), device_id)
        && !session.is_empty()
    {
        let mut changeset = Vec::new();
        session.changeset_strm(&mut changeset)?;
        let sequence: i64 = transaction.query_row(
            "UPDATE sync_devices
             SET next_sequence = next_sequence + 1
             WHERE device_id = ?1 AND next_sequence IS NOT NULL
             RETURNING next_sequence - 1",
            params![device_id.as_bytes().as_slice()],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO sync_outbox (device_id, sequence, changeset)
             VALUES (?1, ?2, ?3)",
            params![device_id.as_bytes().as_slice(), sequence, changeset],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

impl Engine {
    /// Returns the next immutable package that a client must publish.
    ///
    /// All changes currently waiting are grouped. A prepared package is
    /// retained until [`Engine::confirm_sync_package`] succeeds, so changes
    /// made later form the next package and a crash retry returns exactly the
    /// same filename and bytes.
    pub fn next_sync_package(&self) -> Result<Option<OutgoingSyncPackage>> {
        let device_id = self.sync_device_id.ok_or(Error::SyncNotEnabled)?;
        let transaction = self.connection.unchecked_transaction()?;

        if let Some(package) = stored_batch(&transaction, device_id)? {
            transaction.commit()?;
            return Ok(Some(package));
        }

        let mut statement = transaction.prepare(
            "SELECT sequence, changeset
             FROM sync_outbox
             WHERE device_id = ?1
             ORDER BY sequence",
        )?;
        let rows = statement.query_map(params![device_id.as_bytes().as_slice()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let changes: Vec<_> = rows.collect::<rusqlite::Result<_>>()?;
        drop(statement);
        if changes.is_empty() {
            transaction.commit()?;
            return Ok(None);
        }

        let first_sequence = sequence(changes[0].0)?;
        let last_sequence = sequence(changes.last().expect("non-empty changes").0)?;
        for (offset, (actual, _)) in changes.iter().enumerate() {
            let expected = first_sequence
                .checked_add(u64::try_from(offset).map_err(|_| {
                    Error::InvalidDatabase("the synchronization outbox is too large".into())
                })?)
                .ok_or_else(|| {
                    Error::InvalidDatabase("a synchronization sequence overflowed".into())
                })?;
            if sequence(*actual)? != expected {
                return Err(Error::InvalidDatabase(
                    "the local synchronization outbox contains a sequence gap".into(),
                ));
            }
        }

        let mut group = Changegroup::new()?;
        for (_, changeset) in &changes {
            group.add_stream(&mut changeset.as_slice())?;
        }
        let mut payload = Vec::new();
        group.output_strm(&mut payload)?;
        let workspace_id = workspace_id(&transaction)?;
        let package = encode_package(
            workspace_id,
            device_id,
            first_sequence,
            last_sequence,
            &payload,
        );
        transaction.execute(
            "INSERT INTO sync_batch
                 (device_id, first_sequence, last_sequence, package_id, bytes)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                device_id.as_bytes().as_slice(),
                i64::try_from(first_sequence).expect("SQLite sequence"),
                i64::try_from(last_sequence).expect("SQLite sequence"),
                package.id.as_slice(),
                package.bytes
            ],
        )?;
        transaction.commit()?;
        Ok(Some(package))
    }

    /// Confirms that a prepared package was durably published by the client.
    ///
    /// The package remains available until this call commits. Calling it does
    /// not delete any provider file.
    pub fn confirm_sync_package(&mut self, package: &OutgoingSyncPackage) -> Result<()> {
        let device_id = self.sync_device_id.ok_or(Error::SyncNotEnabled)?;
        if package.device_id != device_id {
            return Err(Error::InvalidSyncPackage(
                "the package was produced by another local device".into(),
            ));
        }
        let transaction = self.connection.transaction()?;
        let stored = stored_batch(&transaction, device_id)?;
        if stored.as_ref() != Some(package) {
            return Err(Error::InvalidSyncPackage(
                "the package is not the currently prepared package".into(),
            ));
        }

        let deleted = transaction.execute(
            "DELETE FROM sync_outbox
             WHERE device_id = ?1 AND sequence BETWEEN ?2 AND ?3",
            params![
                device_id.as_bytes().as_slice(),
                i64::try_from(package.first_sequence).expect("SQLite sequence"),
                i64::try_from(package.last_sequence).expect("SQLite sequence")
            ],
        )?;
        let expected = usize::try_from(package.last_sequence - package.first_sequence + 1)
            .expect("bounded package");
        if deleted != expected {
            return Err(Error::InvalidDatabase(
                "the prepared synchronization package does not match its outbox rows".into(),
            ));
        }
        transaction.execute(
            "DELETE FROM sync_batch WHERE device_id = ?1",
            params![device_id.as_bytes().as_slice()],
        )?;
        transaction.execute(
            "UPDATE sync_devices
             SET applied_sequence = ?2, applied_package = ?3
             WHERE device_id = ?1",
            params![
                device_id.as_bytes().as_slice(),
                i64::try_from(package.last_sequence).expect("SQLite sequence"),
                package.id.as_slice()
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Applies one opaque provider package atomically.
    ///
    /// Repeated packages are harmless. Packages from each device must be
    /// supplied in filename order. Independent changes merge; a true row
    /// conflict or a merged tree/content invariant violation aborts the whole
    /// package and leaves it available for later resolution.
    pub fn apply_sync_package(&mut self, bytes: &[u8]) -> Result<SyncApply> {
        self.sync_device_id.ok_or(Error::SyncNotEnabled)?;
        let package = parse_package(bytes)?;
        if package.workspace_id != workspace_id(&self.connection)? {
            return Err(Error::SyncWorkspaceMismatch);
        }
        if self.sync_device_id == Some(package.device_id) {
            return Ok(SyncApply::AlreadyApplied);
        }

        let applied = applied_sequence(&self.connection, package.device_id)?;
        if package.last_sequence <= applied {
            return Ok(SyncApply::AlreadyApplied);
        }
        let expected = applied.checked_add(1).ok_or_else(|| {
            Error::InvalidDatabase("a synchronization sequence overflowed".into())
        })?;
        if package.first_sequence != expected {
            return Err(Error::SyncPackageOutOfOrder {
                device_id: package.device_id,
                expected,
                received: package.first_sequence,
            });
        }

        let touched = touched_nodes(package.payload)?;
        let transaction = self.connection.transaction()?;
        let conflict = Arc::new(AtomicBool::new(false));
        let conflict_seen = Arc::clone(&conflict);
        let mut payload = package.payload;
        let applied = transaction.apply_strm(
            &mut payload,
            None::<fn(&str) -> bool>,
            move |_kind, _item| {
                conflict_seen.store(true, Ordering::Relaxed);
                ConflictAction::SQLITE_CHANGESET_ABORT
            },
        );
        if let Err(error) = applied {
            if conflict.load(Ordering::Relaxed) {
                return Err(Error::SyncConflict {
                    device_id: package.device_id,
                    first_sequence: package.first_sequence,
                    last_sequence: package.last_sequence,
                });
            }
            return Err(error.into());
        }
        validate_merged_state(
            &transaction,
            &touched,
            package.device_id,
            package.first_sequence,
            package.last_sequence,
        )?;
        transaction.execute(
            "INSERT INTO sync_devices
                 (device_id, next_sequence, applied_sequence, applied_package)
             VALUES (?1, NULL, ?2, ?3)
             ON CONFLICT(device_id) DO UPDATE SET
                 applied_sequence = excluded.applied_sequence,
                 applied_package = excluded.applied_package",
            params![
                package.device_id.as_bytes().as_slice(),
                i64::try_from(package.last_sequence).expect("SQLite sequence"),
                package.id.as_slice()
            ],
        )?;
        transaction.commit()?;
        Ok(SyncApply::Applied)
    }
}

#[derive(Default)]
struct TouchedNodes {
    paths: BTreeSet<crate::NodeId>,
    references: BTreeSet<crate::NodeId>,
}

fn touched_nodes(changeset: &[u8]) -> Result<TouchedNodes> {
    let mut input = changeset;
    let input: &mut dyn std::io::Read = &mut input;
    let mut iterator = ChangesetIter::start_strm(&input)?;
    let mut touched = TouchedNodes::default();
    while let Some(item) = iterator.next()? {
        let operation = item.op()?;
        let id = item
            .new_value(0)
            .or_else(|_| item.old_value(0))?
            .as_blob()
            .map_err(|_| Error::InvalidSyncPackage("a primary key is not a BLOB".into()))?;
        let id = decode_id(id.to_vec())?;
        match operation.table_name() {
            "nodes" => {
                touched.paths.insert(id);
                touched.references.insert(id);
            }
            "node_references" => {
                touched.references.insert(id);
            }
            "node_tags" => {}
            table => {
                return Err(Error::InvalidSyncPackage(format!(
                    "changeset contains unexpected table {table:?}"
                )));
            }
        }
    }
    Ok(touched)
}

fn validate_merged_state(
    connection: &Connection,
    touched: &TouchedNodes,
    device_id: SyncDeviceId,
    first_sequence: u64,
    last_sequence: u64,
) -> Result<()> {
    for id in &touched.paths {
        let (exists, reaches_root): (bool, bool) = connection.query_row(
            "WITH RECURSIVE ancestors(id, parent_id) AS (
                 SELECT id, parent_id FROM nodes WHERE id = ?1
                 UNION
                 SELECT nodes.id, nodes.parent_id
                 FROM nodes JOIN ancestors ON nodes.id = ancestors.parent_id
             )
             SELECT EXISTS(SELECT 1 FROM ancestors),
                    EXISTS(SELECT 1 FROM ancestors WHERE parent_id IS NULL)",
            params![node_id_bytes(id)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if exists && !reaches_root {
            return Err(Error::SyncConflict {
                device_id,
                first_sequence,
                last_sequence,
            });
        }
    }

    for id in &touched.references {
        let Some(text) = connection
            .query_row(
                "SELECT text FROM nodes WHERE id = ?1",
                params![node_id_bytes(id)],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        else {
            continue;
        };
        let mut statement = connection.prepare(
            "SELECT start_byte, end_byte FROM node_references
             WHERE source_id = ?1 ORDER BY start_byte",
        )?;
        let mut rows = statement.query(params![node_id_bytes(id)])?;
        let mut previous_end = 0;
        while let Some(row) = rows.next()? {
            let start: i64 = row.get(0)?;
            let end: i64 = row.get(1)?;
            let valid = usize::try_from(start)
                .ok()
                .zip(usize::try_from(end).ok())
                .is_some_and(|(start, end)| {
                    start >= previous_end && reference_range_is_valid(&text, start, end)
                });
            if !valid {
                return Err(Error::SyncConflict {
                    device_id,
                    first_sequence,
                    last_sequence,
                });
            }
            previous_end = usize::try_from(end).expect("validated reference end");
        }
    }
    Ok(())
}

fn stored_batch(
    connection: &Connection,
    device_id: SyncDeviceId,
) -> Result<Option<OutgoingSyncPackage>> {
    connection
        .query_row(
            "SELECT first_sequence, last_sequence, package_id, bytes
             FROM sync_batch WHERE device_id = ?1",
            params![device_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )
        .optional()?
        .map(|(first, last, id, bytes)| {
            let id: [u8; PACKAGE_HASH_LENGTH] = id.try_into().map_err(|_| {
                Error::InvalidDatabase("a synchronization package ID is invalid".into())
            })?;
            let package = OutgoingSyncPackage {
                device_id,
                first_sequence: sequence(first)?,
                last_sequence: sequence(last)?,
                id,
                bytes,
            };
            let parsed = parse_package(&package.bytes)?;
            if parsed.workspace_id != workspace_id(connection)?
                || parsed.device_id != package.device_id
                || parsed.first_sequence != package.first_sequence
                || parsed.last_sequence != package.last_sequence
                || parsed.id != package.id
            {
                return Err(Error::InvalidDatabase(
                    "a prepared synchronization package does not match its metadata".into(),
                ));
            }
            Ok(package)
        })
        .transpose()
}

fn workspace_id(connection: &Connection) -> Result<[u8; 16]> {
    let bytes: Vec<u8> = connection.query_row(
        "SELECT workspace_id FROM workspace WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    bytes
        .try_into()
        .map_err(|_| Error::InvalidDatabase("the workspace identity is invalid".into()))
}

fn applied_sequence(connection: &Connection, device_id: SyncDeviceId) -> Result<u64> {
    let value = connection
        .query_row(
            "SELECT applied_sequence FROM sync_devices WHERE device_id = ?1",
            params![device_id.as_bytes().as_slice()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);
    sequence_allow_zero(value)
}

fn sequence(value: i64) -> Result<u64> {
    let value = sequence_allow_zero(value)?;
    if value == 0 {
        return Err(Error::InvalidDatabase(
            "a synchronization sequence is zero".into(),
        ));
    }
    Ok(value)
}

fn sequence_allow_zero(value: i64) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| Error::InvalidDatabase("a synchronization sequence is negative".into()))
}

fn encode_package(
    workspace_id: [u8; 16],
    device_id: SyncDeviceId,
    first_sequence: u64,
    last_sequence: u64,
    payload: &[u8],
) -> OutgoingSyncPackage {
    let mut bytes = Vec::with_capacity(PACKAGE_HEADER_LENGTH + payload.len() + PACKAGE_HASH_LENGTH);
    bytes.extend_from_slice(PACKAGE_MAGIC);
    bytes.push(PACKAGE_VERSION);
    bytes.extend_from_slice(&workspace_id);
    bytes.extend_from_slice(device_id.as_bytes());
    bytes.extend_from_slice(&first_sequence.to_be_bytes());
    bytes.extend_from_slice(&last_sequence.to_be_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    bytes.extend_from_slice(payload);
    let id = *blake3::hash(&bytes).as_bytes();
    bytes.extend_from_slice(&id);
    OutgoingSyncPackage {
        device_id,
        first_sequence,
        last_sequence,
        id,
        bytes,
    }
}

fn parse_package(bytes: &[u8]) -> Result<ParsedPackage<'_>> {
    if bytes.len() < PACKAGE_HEADER_LENGTH + PACKAGE_HASH_LENGTH {
        return Err(Error::InvalidSyncPackage("package is truncated".into()));
    }
    let (signed, hash) = bytes.split_at(bytes.len() - PACKAGE_HASH_LENGTH);
    if blake3::hash(signed).as_bytes() != hash {
        return Err(Error::InvalidSyncPackage("checksum does not match".into()));
    }
    let mut header = &signed[..PACKAGE_HEADER_LENGTH];
    if take::<8>(&mut header)? != *PACKAGE_MAGIC {
        return Err(Error::InvalidSyncPackage("unknown package marker".into()));
    }
    if take::<1>(&mut header)?[0] != PACKAGE_VERSION {
        return Err(Error::InvalidSyncPackage(
            "unsupported package version".into(),
        ));
    }
    let workspace_id = take::<16>(&mut header)?;
    let device_id = SyncDeviceId::from_bytes(take::<SYNC_DEVICE_ID_LENGTH>(&mut header)?);
    let first_sequence = u64::from_be_bytes(take::<8>(&mut header)?);
    let last_sequence = u64::from_be_bytes(take::<8>(&mut header)?);
    let payload_length = u64::from_be_bytes(take::<8>(&mut header)?);
    if first_sequence == 0 || last_sequence < first_sequence || last_sequence > i64::MAX as u64 {
        return Err(Error::InvalidSyncPackage("invalid sequence range".into()));
    }
    let payload = &signed[PACKAGE_HEADER_LENGTH..];
    if usize::try_from(payload_length).ok() != Some(payload.len()) {
        return Err(Error::InvalidSyncPackage(
            "payload length does not match".into(),
        ));
    }
    let id = hash.try_into().expect("fixed package hash");
    Ok(ParsedPackage {
        workspace_id,
        device_id,
        first_sequence,
        last_sequence,
        id,
        payload,
    })
}

fn take<const N: usize>(input: &mut &[u8]) -> Result<[u8; N]> {
    if input.len() < N {
        return Err(Error::InvalidSyncPackage(
            "package header is truncated".into(),
        ));
    }
    let (value, remaining) = input.split_at(N);
    *input = remaining;
    Ok(value.try_into().expect("fixed header field"))
}
