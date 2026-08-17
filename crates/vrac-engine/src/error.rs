use std::error::Error as StdError;
use std::fmt;

use crate::{CheckReport, NodeId, SyncDeviceId};

/// Error returned by the Vrac engine.
#[derive(Debug)]
pub enum Error {
    /// Failure reported directly by SQLite.
    Sqlite(rusqlite::Error),
    /// File contents do not match a valid Vrac workspace.
    InvalidDatabase(String),
    /// Required SQLite connection guarantees could not be enabled.
    StorageConfiguration(String),
    /// Workspace schema version is newer or otherwise unsupported.
    UnsupportedSchemaVersion(i64),
    /// Requested node does not exist.
    NodeNotFound(NodeId),
    /// Requested parent does not exist.
    ParentNotFound(NodeId),
    /// Relative placement references a node outside the destination siblings.
    PlacementReferenceNotSibling {
        /// Referenced node.
        reference: NodeId,
        /// Expected parent, or `None` for root siblings.
        parent_id: Option<NodeId>,
    },
    /// Requested move would create a cycle.
    Cycle,
    /// Requested page size is zero or exceeds [`crate::MAX_PAGE_SIZE`].
    InvalidPageLimit {
        /// Requested page size.
        limit: usize,
        /// Largest accepted page size.
        maximum: usize,
    },
    /// No safe integer remains for a storage position.
    PositionOverflow,
    /// Requested performance dataset cannot be represented in memory.
    GenerationTooLarge(u64),
    /// Performance data generation is disabled on a synchronized workspace.
    GenerationOnSynchronizedWorkspace,
    /// A tag is empty or contains whitespace or `#`.
    InvalidTag(String),
    /// Clipboard text cannot be interpreted as a node outline.
    InvalidClipboard(String),
    /// An inline reference does not cover a valid `[[label]]` range.
    InvalidReferenceRange {
        /// Inclusive UTF-8 byte offset supplied by the client.
        start: usize,
        /// Exclusive UTF-8 byte offset supplied by the client.
        end: usize,
    },
    /// Two inline reference ranges overlap.
    OverlappingReferences,
    /// An inline reference target does not exist.
    ReferenceTargetNotFound(NodeId),
    /// A plain text replacement would discard outgoing references.
    NodeHasReferences(NodeId),
    /// A node in the requested subtree is referenced from outside it.
    NodeReferenced(NodeId),
    /// A product-owned structural node cannot be edited, moved, or removed.
    SystemNodeProtected(NodeId),
    /// A journal day is not a real ISO calendar date.
    InvalidJournalDate(String),
    /// The checkpoint destination already exists and was not modified.
    CheckpointDestinationExists,
    /// A generated checkpoint failed its complete integrity validation.
    InvalidCheckpoint(CheckReport),
    /// A restore checkpoint belongs to another workspace.
    CheckpointWorkspaceMismatch,
    /// The active workspace was supplied as its own restore checkpoint.
    RestoreSourceIsActiveWorkspace,
    /// Synchronization was requested on an engine opened without a device ID.
    SyncNotEnabled,
    /// A synchronized workspace is already active under another device ID.
    SyncDeviceMismatch {
        /// Device currently capturing mutations in this workspace file.
        active: SyncDeviceId,
        /// Device requested by the client.
        requested: SyncDeviceId,
    },
    /// A synchronization package is malformed or corrupted.
    InvalidSyncPackage(String),
    /// A package belongs to another workspace.
    SyncWorkspaceMismatch,
    /// A package is missing an earlier package from the same device.
    SyncPackageOutOfOrder {
        /// Device whose stream has a gap.
        device_id: SyncDeviceId,
        /// Next sequence expected locally.
        expected: u64,
        /// First sequence in the received package.
        received: u64,
    },
    /// A package depends on a package from another device not yet applied.
    SyncDependencyMissing {
        /// Device that produced the deferred package.
        device_id: SyncDeviceId,
        /// First transaction in the deferred package.
        first_sequence: u64,
        /// Last transaction in the deferred package.
        last_sequence: u64,
    },
    /// Applying a package would overwrite a concurrent local change.
    SyncConflict {
        /// Device that produced the conflicting package.
        device_id: SyncDeviceId,
        /// First transaction in the conflicting package.
        first_sequence: u64,
        /// Last transaction in the conflicting package.
        last_sequence: u64,
    },
    /// Undo or redo no longer matches the current workspace state.
    HistoryConflict,
    /// Secure random bytes could not be obtained.
    Randomness(String),
    /// A filesystem operation outside SQLite failed.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
            Self::InvalidDatabase(reason) => write!(formatter, "invalid Vrac database: {reason}"),
            Self::StorageConfiguration(reason) => {
                write!(formatter, "invalid SQLite configuration: {reason}")
            }
            Self::UnsupportedSchemaVersion(version) => {
                write!(formatter, "unsupported schema version: {version}")
            }
            Self::NodeNotFound(id) => write!(formatter, "node not found: {id}"),
            Self::ParentNotFound(id) => write!(formatter, "parent not found: {id}"),
            Self::PlacementReferenceNotSibling {
                reference,
                parent_id: Some(parent_id),
            } => write!(
                formatter,
                "placement reference {reference} is not a child of {parent_id}"
            ),
            Self::PlacementReferenceNotSibling {
                reference,
                parent_id: None,
            } => write!(
                formatter,
                "placement reference {reference} is not a root node"
            ),
            Self::Cycle => formatter.write_str("this move would create a cycle"),
            Self::InvalidPageLimit { limit, maximum } => {
                write!(formatter, "invalid page size ({limit}), maximum: {maximum}")
            }
            Self::PositionOverflow => formatter.write_str("no integer position remains available"),
            Self::GenerationTooLarge(count) => {
                write!(
                    formatter,
                    "the generator cannot create {count} nodes at once"
                )
            }
            Self::GenerationOnSynchronizedWorkspace => formatter
                .write_str("performance data generation is disabled on synchronized workspaces"),
            Self::InvalidTag(tag) => write!(formatter, "invalid tag: {tag:?}"),
            Self::InvalidClipboard(reason) => {
                write!(formatter, "invalid clipboard outline: {reason}")
            }
            Self::InvalidReferenceRange { start, end } => {
                write!(formatter, "invalid reference range: {start}..{end}")
            }
            Self::OverlappingReferences => formatter.write_str("reference ranges overlap"),
            Self::ReferenceTargetNotFound(id) => {
                write!(formatter, "reference target not found: {id}")
            }
            Self::NodeHasReferences(id) => write!(
                formatter,
                "node {id} has outgoing references; replace its content instead"
            ),
            Self::NodeReferenced(id) => {
                write!(
                    formatter,
                    "node {id} is referenced from outside its subtree"
                )
            }
            Self::SystemNodeProtected(id) => write!(formatter, "system node is protected: {id}"),
            Self::InvalidJournalDate(date) => write!(formatter, "invalid journal date: {date:?}"),
            Self::CheckpointDestinationExists => {
                formatter.write_str("checkpoint destination already exists")
            }
            Self::InvalidCheckpoint(report) => write!(
                formatter,
                "checkpoint validation reported {} integrity issues",
                report.issues.len()
            ),
            Self::CheckpointWorkspaceMismatch => {
                formatter.write_str("checkpoint belongs to another workspace")
            }
            Self::RestoreSourceIsActiveWorkspace => {
                formatter.write_str("the active workspace cannot restore itself")
            }
            Self::SyncNotEnabled => formatter.write_str(
                "synchronization requires opening the workspace with a device identifier",
            ),
            Self::SyncDeviceMismatch { active, requested } => write!(
                formatter,
                "workspace synchronization is active for device {active}, not {requested}"
            ),
            Self::InvalidSyncPackage(reason) => {
                write!(formatter, "invalid synchronization package: {reason}")
            }
            Self::SyncWorkspaceMismatch => {
                formatter.write_str("synchronization package belongs to another workspace")
            }
            Self::SyncPackageOutOfOrder {
                device_id,
                expected,
                received,
            } => write!(
                formatter,
                "synchronization package from {device_id} starts at {received}, expected {expected}"
            ),
            Self::SyncDependencyMissing {
                device_id,
                first_sequence,
                last_sequence,
            } => write!(
                formatter,
                "synchronization package {first_sequence}..={last_sequence} from {device_id} depends on changes not yet applied"
            ),
            Self::SyncConflict {
                device_id,
                first_sequence,
                last_sequence,
            } => write!(
                formatter,
                "synchronization conflict with {device_id} package {first_sequence}..={last_sequence}"
            ),
            Self::HistoryConflict => {
                formatter.write_str("undo history no longer matches the workspace state")
            }
            Self::Randomness(reason) => write!(formatter, "secure randomness failed: {reason}"),
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Result type returned by Vrac engine operations.
pub type Result<T> = std::result::Result<T, Error>;
