use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use fallible_streaming_iterator::FallibleStreamingIterator;
use rusqlite::session::{Changegroup, ChangesetIter, ConflictAction, ConflictType, Session};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::content::reference_range_is_valid;
use crate::db::Engine;
use crate::nodes::{decode_id, node_id_bytes};
use crate::{
    CheckIssue, Error, OutgoingSyncPackage, Result, SYNC_DEVICE_ID_LENGTH, SyncApply, SyncDeviceId,
    WorkspaceId,
};

const PACKAGE_MAGIC: &[u8; 8] = b"VRACSYNC";
const PACKAGE_VERSION: u8 = 1;
const PACKAGE_HEADER_LENGTH: usize = 65;
const PACKAGE_HASH_LENGTH: usize = 32;
const CAPTURED_TABLES: [&str; 3] = ["nodes", "node_tags", "node_references"];

struct ParsedPackage<'a> {
    workspace_id: WorkspaceId,
    device_id: SyncDeviceId,
    first_sequence: u64,
    last_sequence: u64,
    id: [u8; PACKAGE_HASH_LENGTH],
    payload: &'a [u8],
}

struct DeviceState {
    next: Option<u64>,
    applied: u64,
    outbox_count: u64,
    first_outbox: Option<u64>,
    last_outbox: Option<u64>,
}

pub(crate) fn resolve_device(
    connection: &mut Connection,
    requested: Option<SyncDeviceId>,
) -> Result<Option<SyncDeviceId>> {
    let active = {
        let mut statement = connection.prepare(
            "SELECT device_id FROM sync_devices
             WHERE next_sequence IS NOT NULL ORDER BY device_id LIMIT 2",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    if active.len() > 1 {
        return Err(Error::InvalidDatabase(
            "multiple local synchronization devices are active".into(),
        ));
    }
    let active = active
        .into_iter()
        .next()
        .map(decode_device_id)
        .transpose()?;

    match (active, requested) {
        (Some(active), Some(requested)) if active != requested => {
            return Err(Error::SyncDeviceMismatch { active, requested });
        }
        (Some(active), _) => return Ok(Some(active)),
        (None, None) => return Ok(None),
        (None, Some(_)) => {}
    }

    let device_id = requested.expect("requested device handled above");
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
    Ok(Some(device_id))
}

pub(crate) fn check_sync(
    connection: &Connection,
    maximum: usize,
) -> Result<(Vec<CheckIssue>, bool)> {
    let mut issues = Vec::new();
    let mut omitted = false;
    let mut devices = BTreeMap::new();
    let mut active_devices = 0_usize;
    let mut statement = connection.prepare(
        "SELECT devices.device_id, devices.next_sequence, devices.applied_sequence,
                COUNT(outbox.sequence), MIN(outbox.sequence), MAX(outbox.sequence)
         FROM sync_devices AS devices
         LEFT JOIN sync_outbox AS outbox ON outbox.device_id = devices.device_id
         GROUP BY devices.device_id
         ORDER BY devices.device_id",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let device_id = decode_device_id(row.get(0)?)?;
        let next = row.get::<_, Option<i64>>(1)?.map(sequence).transpose()?;
        let applied = sequence_allow_zero(row.get(2)?)?;
        let outbox_count = sequence_allow_zero(row.get(3)?)?;
        let first_outbox = row.get::<_, Option<i64>>(4)?.map(sequence).transpose()?;
        let last_outbox = row.get::<_, Option<i64>>(5)?.map(sequence).transpose()?;
        if next.is_some() {
            active_devices += 1;
        }
        let state = DeviceState {
            next,
            applied,
            outbox_count,
            first_outbox,
            last_outbox,
        };
        if !device_state_is_valid(&state) {
            record_sync_issue(
                &mut issues,
                &mut omitted,
                maximum,
                format!("device {device_id} has an inconsistent sequence or outbox frontier"),
            );
        }
        devices.insert(device_id, state);
    }
    drop(rows);
    drop(statement);

    if active_devices > 1 {
        record_sync_issue(
            &mut issues,
            &mut omitted,
            maximum,
            "multiple local synchronization devices are active".into(),
        );
    }

    let mut statement = connection.prepare(
        "SELECT device_id, sequence, changeset
         FROM sync_outbox ORDER BY device_id, sequence",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let device_id = decode_device_id(row.get(0)?)?;
        let sequence = sequence(row.get(1)?)?;
        let changeset: Vec<u8> = row.get(2)?;
        if let Err(error) = touched_nodes(&changeset) {
            record_sync_issue(
                &mut issues,
                &mut omitted,
                maximum,
                format!("device {device_id} outbox sequence {sequence} is invalid: {error}"),
            );
        }
    }
    drop(rows);
    drop(statement);

    let mut statement = connection.prepare(
        "SELECT device_id, first_sequence, last_sequence
         FROM sync_batch ORDER BY device_id",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let device_id = decode_device_id(row.get(0)?)?;
        let first = sequence(row.get(1)?)?;
        let last = sequence(row.get(2)?)?;
        let state_is_valid = devices.get(&device_id).is_some_and(|state| {
            state.next.is_some()
                && state.first_outbox == Some(first)
                && state
                    .last_outbox
                    .is_some_and(|outbox_last| last <= outbox_last)
        });
        if !state_is_valid {
            record_sync_issue(
                &mut issues,
                &mut omitted,
                maximum,
                format!("device {device_id} has a prepared package outside its outbox frontier"),
            );
        }
        if let Err(error) = stored_batch(connection, device_id) {
            record_sync_issue(
                &mut issues,
                &mut omitted,
                maximum,
                format!("device {device_id} has an invalid prepared package: {error}"),
            );
        }
    }

    Ok((issues, omitted))
}

fn device_state_is_valid(state: &DeviceState) -> bool {
    match state.next {
        Some(next) => {
            next > state.applied
                && next - state.applied - 1 == state.outbox_count
                && match state.outbox_count {
                    0 => state.first_outbox.is_none() && state.last_outbox.is_none(),
                    _ => {
                        state.first_outbox == Some(state.applied + 1)
                            && state.last_outbox == Some(next - 1)
                    }
                }
        }
        None => {
            state.outbox_count == 0 && state.first_outbox.is_none() && state.last_outbox.is_none()
        }
    }
}

fn record_sync_issue(
    issues: &mut Vec<CheckIssue>,
    omitted: &mut bool,
    maximum: usize,
    message: String,
) {
    if issues.len() < maximum {
        issues.push(CheckIssue::InvalidSyncState(message));
    } else {
        *omitted = true;
    }
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
    /// Returns the stable identity shared by every copy of this workspace.
    pub fn workspace_id(&self) -> Result<WorkspaceId> {
        workspace_id(&self.connection)
    }

    /// Returns whether this local copy still owns unpublished changes.
    pub fn has_pending_sync_changes(&self) -> Result<bool> {
        self.sync_device_id.ok_or(Error::SyncNotEnabled)?;
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_outbox UNION ALL SELECT 1 FROM sync_batch)",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

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
        const NO_APPLY_ERROR: u8 = 0;
        const MISSING_DEPENDENCY: u8 = 1;
        const TRUE_CONFLICT: u8 = 2;
        let apply_error = Arc::new(AtomicU8::new(NO_APPLY_ERROR));
        let apply_error_seen = Arc::clone(&apply_error);
        let mut payload = package.payload;
        let applied = transaction.apply_strm(
            &mut payload,
            None::<fn(&str) -> bool>,
            move |kind, _item| {
                let classification = match kind {
                    ConflictType::SQLITE_CHANGESET_NOTFOUND
                    | ConflictType::SQLITE_CHANGESET_FOREIGN_KEY => MISSING_DEPENDENCY,
                    _ => TRUE_CONFLICT,
                };
                apply_error_seen.store(classification, Ordering::Relaxed);
                ConflictAction::SQLITE_CHANGESET_ABORT
            },
        );
        if let Err(error) = applied {
            let error = match apply_error.load(Ordering::Relaxed) {
                MISSING_DEPENDENCY => Error::SyncDependencyMissing {
                    device_id: package.device_id,
                    first_sequence: package.first_sequence,
                    last_sequence: package.last_sequence,
                },
                TRUE_CONFLICT => Error::SyncConflict {
                    device_id: package.device_id,
                    first_sequence: package.first_sequence,
                    last_sequence: package.last_sequence,
                },
                _ => error.into(),
            };
            return Err(error);
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

fn workspace_id(connection: &Connection) -> Result<WorkspaceId> {
    let bytes: Vec<u8> = connection.query_row(
        "SELECT workspace_id FROM workspace WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    bytes
        .try_into()
        .map(WorkspaceId::from_bytes)
        .map_err(|_| Error::InvalidDatabase("the workspace identity is invalid".into()))
}

fn decode_device_id(bytes: Vec<u8>) -> Result<SyncDeviceId> {
    let bytes = bytes
        .try_into()
        .map_err(|_| Error::InvalidDatabase("a synchronization device ID is invalid".into()))?;
    Ok(SyncDeviceId::from_bytes(bytes))
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
    workspace_id: WorkspaceId,
    device_id: SyncDeviceId,
    first_sequence: u64,
    last_sequence: u64,
    payload: &[u8],
) -> OutgoingSyncPackage {
    let mut bytes = Vec::with_capacity(PACKAGE_HEADER_LENGTH + payload.len() + PACKAGE_HASH_LENGTH);
    bytes.extend_from_slice(PACKAGE_MAGIC);
    bytes.push(PACKAGE_VERSION);
    bytes.extend_from_slice(workspace_id.as_bytes());
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
    let workspace_id = WorkspaceId::from_bytes(take::<16>(&mut header)?);
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
