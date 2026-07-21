//! Vrac's local-first engine.
//!
//! The library owns the model, business rules, and SQLite storage. It is
//! synchronous and performs no user-facing input or output.
//!
//! # Example
//!
//! ```
//! use vrac::{CreateNode, Engine, Page};
//!
//! let mut engine = Engine::open(":memory:")?;
//! let meeting = engine.create_node(CreateNode::new("Meeting about project X"))?;
//! let mut decision = CreateNode::new("Ship the first version on Friday");
//! decision.parent_id = Some(meeting.id);
//! engine.create_node(decision)?;
//!
//! let children = engine.children(Some(meeting.id), Page::default())?;
//! assert_eq!(children.nodes.len(), 1);
//! assert_eq!(children.nodes[0].text, "Ship the first version on Friday");
//! # Ok::<(), vrac::Error>(())
//! ```

#![deny(missing_docs)]

mod backlinks;
mod checkpoint;
mod clipboard;
mod content;
mod db;
mod history;
mod journal;
mod nodes;
mod order;
mod schema;
mod sync;

use std::error::Error as StdError;
use std::fmt;
use std::str::FromStr;

pub use db::Engine;

/// Number of bytes in a node identifier.
pub const NODE_ID_LENGTH: usize = 16;

/// Number of bytes in a synchronization device identifier.
pub const SYNC_DEVICE_ID_LENGTH: usize = 16;

/// Number of bytes in a workspace identifier.
pub const WORKSPACE_ID_LENGTH: usize = 16;

/// Default result page size.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// Maximum accepted result page size.
pub const MAX_PAGE_SIZE: usize = 1_000;

fn decode_hex<const LENGTH: usize>(value: &str) -> Option<[u8; LENGTH]> {
    if value.len() != LENGTH * 2 {
        return None;
    }
    let mut decoded = [0_u8; LENGTH];
    for (byte, pair) in decoded.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(decoded)
}

/// Opaque, stable node identifier.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId([u8; NODE_ID_LENGTH]);

impl NodeId {
    /// Creates an identifier from its canonical 16-byte representation.
    pub fn from_bytes(bytes: [u8; NODE_ID_LENGTH]) -> Self {
        Self(bytes)
    }

    /// Returns the canonical 16-byte representation.
    pub fn as_bytes(&self) -> &[u8; NODE_ID_LENGTH] {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "NodeId(\"{self}\")")
    }
}

impl FromStr for NodeId {
    type Err = ParseNodeIdError;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        decode_hex(value).map(Self).ok_or(ParseNodeIdError)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Error returned when parsing a textual node identifier.
pub struct ParseNodeIdError;

impl fmt::Display for ParseNodeIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a node identifier must contain exactly 32 hexadecimal characters")
    }
}

impl StdError for ParseNodeIdError {}

/// Opaque identity shared by every local copy of one workspace.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkspaceId([u8; WORKSPACE_ID_LENGTH]);

impl WorkspaceId {
    /// Creates an identifier from its canonical bytes.
    pub fn from_bytes(bytes: [u8; WORKSPACE_ID_LENGTH]) -> Self {
        Self(bytes)
    }

    /// Returns the canonical bytes.
    pub fn as_bytes(&self) -> &[u8; WORKSPACE_ID_LENGTH] {
        &self.0
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for WorkspaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "WorkspaceId(\"{self}\")")
    }
}

impl FromStr for WorkspaceId {
    type Err = ParseWorkspaceIdError;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        decode_hex(value).map(Self).ok_or(ParseWorkspaceIdError)
    }
}

/// Error returned when parsing a textual workspace identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseWorkspaceIdError;

impl fmt::Display for ParseWorkspaceIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a workspace identifier must contain exactly 32 hexadecimal characters")
    }
}

impl StdError for ParseWorkspaceIdError {}

/// Stable identity of one local application installation.
///
/// Clients generate it once and keep it in their local application data. It
/// must not be copied with a workspace or stored in a synchronized folder.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SyncDeviceId([u8; SYNC_DEVICE_ID_LENGTH]);

impl SyncDeviceId {
    /// Generates a cryptographically random device identifier.
    pub fn generate() -> Result<Self> {
        let mut bytes = [0_u8; SYNC_DEVICE_ID_LENGTH];
        getrandom::fill(&mut bytes).map_err(|error| Error::Randomness(error.to_string()))?;
        Ok(Self(bytes))
    }

    /// Creates an identifier from its canonical bytes.
    pub fn from_bytes(bytes: [u8; SYNC_DEVICE_ID_LENGTH]) -> Self {
        Self(bytes)
    }

    /// Returns the canonical bytes.
    pub fn as_bytes(&self) -> &[u8; SYNC_DEVICE_ID_LENGTH] {
        &self.0
    }
}

impl fmt::Display for SyncDeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for SyncDeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SyncDeviceId(\"{self}\")")
    }
}

/// One immutable opaque package ready for a synchronization provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutgoingSyncPackage {
    device_id: SyncDeviceId,
    first_sequence: u64,
    last_sequence: u64,
    id: [u8; 32],
    bytes: Vec<u8>,
}

impl OutgoingSyncPackage {
    /// Stable filename suitable for an immutable provider object.
    pub fn file_name(&self) -> String {
        format!(
            "{}-{:020}-{:020}.vrac-sync",
            self.device_id, self.first_sequence, self.last_sequence
        )
    }

    /// Complete opaque package bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Outcome of importing one synchronization package.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncApply {
    /// The package was applied atomically.
    Applied,
    /// The package was already represented by the local workspace.
    AlreadyApplied,
}

/// A text item in the Vrac tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    /// Stable identity of the node.
    pub id: NodeId,
    /// Parent identity, or `None` for a root node.
    pub parent_id: Option<NodeId>,
    /// Whether the node currently has at least one child.
    pub has_children: bool,
    /// Plain text stored by the node.
    pub text: String,
    /// Product-owned structural identity, when this is a protected system node.
    pub system: Option<SystemNode>,
    /// Canonical tags in deterministic lexical order.
    pub tags: Vec<String>,
    /// Inline references ordered by their label range.
    pub references: Vec<NodeReference>,
}

/// Product-owned structural identity attached to a protected node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SystemNode {
    /// Visible container of every journal day.
    Journal,
    /// One local calendar day in the journal.
    JournalDay {
        /// Canonical ISO calendar date (`YYYY-MM-DD`).
        date: String,
    },
}

/// One resolved inline reference returned with a node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeReference {
    /// Inclusive UTF-8 byte offset of the label after `[[`.
    pub label_start: usize,
    /// Exclusive UTF-8 byte offset of the label before `]]`.
    pub label_end: usize,
    /// Stable identity of the referenced node.
    pub target_id: NodeId,
    /// Current plain text of the referenced node.
    pub target_text: String,
}

/// Result of atomically replacing a node's text and references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContentUpdate {
    /// Complete resolved references now stored on the edited node.
    pub references: Vec<NodeReference>,
    /// Concept or Journal nodes created from previously unresolved `[[labels]]`.
    pub materialized_nodes: Vec<Node>,
    /// Empty ordinary roots removed after their final reference disappeared.
    pub pruned_roots: Vec<NodeId>,
}

/// Result of deleting a requested subtree and cleaning detached empty roots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteOutcome {
    /// Number of nodes in the explicitly deleted subtree.
    pub deleted_nodes: u64,
    /// Empty ordinary roots removed after losing references from that subtree.
    pub pruned_roots: Vec<NodeId>,
}

/// One inline reference supplied while creating or editing content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReferenceInput {
    /// Inclusive UTF-8 byte offset of the label after `[[`.
    pub label_start: usize,
    /// Exclusive UTF-8 byte offset of the label before `]]`.
    pub label_end: usize,
    /// Stable identity of the referenced node.
    pub target_id: NodeId,
}

/// Data required to create a node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateNode {
    /// Parent identity, or `None` to create a root node.
    pub parent_id: Option<NodeId>,
    /// Placement within the destination sibling list.
    pub placement: Placement,
    /// Plain text for the new node.
    pub text: String,
    /// Tags for the new node. They are canonicalized by the engine.
    pub tags: Vec<String>,
    /// Inline references for the new node.
    pub references: Vec<ReferenceInput>,
}

impl CreateNode {
    /// Creates a root-node request placed after existing roots.
    ///
    /// Set [`CreateNode::parent_id`] or [`CreateNode::placement`] before calling
    /// [`Engine::create_node`] when another destination is required.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            parent_id: None,
            placement: Placement::Last,
            text: text.into(),
            tags: Vec::new(),
            references: Vec::new(),
        }
    }
}

/// Destination of a move operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Destination {
    /// Parent identity, or `None` to move the node to the root.
    pub parent_id: Option<NodeId>,
    /// Placement within the destination sibling list.
    pub placement: Placement,
}

/// Relative placement of a node within its sibling list.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Placement {
    /// Before every existing sibling.
    First,
    /// After every existing sibling.
    #[default]
    Last,
    /// Immediately before the referenced sibling.
    Before(NodeId),
    /// Immediately after the referenced sibling.
    After(NodeId),
}

/// Opaque cursor returned by a paginated read.
///
/// Cursors are continuation state, not a persistent storage format. Clients
/// may pass their textual representation unchanged across an IPC boundary.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Cursor {
    position: i64,
    id: NodeId,
}

impl fmt::Debug for Cursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Cursor(..)")
    }
}

impl fmt::Display for Cursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let position = u64::from_be_bytes(self.position.to_be_bytes());
        write!(formatter, "v1:{position:016x}:{}", self.id)
    }
}

impl FromStr for Cursor {
    type Err = ParseCursorError;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let mut parts = value.split(':');
        let version = parts.next();
        let position = parts.next();
        let id = parts.next();
        if version != Some("v1") || parts.next().is_some() {
            return Err(ParseCursorError);
        }
        let position = position
            .filter(|value| value.len() == 16)
            .and_then(|value| u64::from_str_radix(value, 16).ok())
            .ok_or(ParseCursorError)?;
        let id = id
            .ok_or(ParseCursorError)?
            .parse()
            .map_err(|_| ParseCursorError)?;
        Ok(Self {
            position: i64::from_be_bytes(position.to_be_bytes()),
            id,
        })
    }
}

/// Error returned when parsing a textual pagination cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseCursorError;

impl fmt::Display for ParseCursorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid pagination cursor")
    }
}

impl StdError for ParseCursorError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Request for one bounded page of children.
pub struct Page {
    /// Maximum number of nodes to return.
    pub limit: usize,
    /// Opaque continuation returned by the preceding page.
    pub after: Option<Cursor>,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            limit: DEFAULT_PAGE_SIZE,
            after: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// One ordered page of sibling nodes.
pub struct NodePage {
    /// Nodes in deterministic sibling order.
    pub nodes: Vec<Node>,
    /// Continuation for the next page, or `None` at the end.
    pub next: Option<Cursor>,
}

/// One backlink together with the ancestors that give it meaning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BacklinkContext {
    /// Path from the root to the matching node, inclusive.
    pub path: Vec<Node>,
}

/// One deterministic page of contextual backlinks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BacklinkPage {
    /// Matching nodes with their complete ancestor paths.
    pub contexts: Vec<BacklinkContext>,
    /// Continuation for the next page, or `None` at the end.
    pub next: Option<Cursor>,
}

/// One tag found inside the contextual scope of a backlink target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BacklinkTag {
    /// Canonical tag value without the visual `#` marker.
    pub tag: String,
    /// Number of distinct matching nodes in the contextual scope.
    pub count: u64,
}

/// Shape used when generating performance and test data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerateShape {
    /// All nodes are placed at the root.
    Wide,
    /// Every node is a child of the preceding node.
    Deep,
    /// Balanced tree with at most ten children per node.
    Mixed,
}

/// Result of a complete workspace integrity check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckReport {
    /// Total number of canonical nodes inspected.
    pub node_count: u64,
    /// Integrity issues found during the check.
    pub issues: Vec<CheckIssue>,
}

impl CheckReport {
    /// Returns `true` when no integrity issue was found.
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Integrity issue reported by [`Engine::check`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckIssue {
    /// Message returned by SQLite's integrity check.
    SqliteIntegrity(String),
    /// Broken SQLite foreign-key relationship.
    ForeignKey {
        /// Table containing the invalid relationship.
        table: String,
        /// SQLite row identifier when one is available.
        rowid: Option<i64>,
        /// Referenced parent table.
        parent: String,
        /// Index of the failed foreign-key constraint.
        foreign_key_index: i64,
    },
    /// Number of nodes that cannot be reached from a root.
    UnreachableNodes(u64),
    /// A stored tag is not in canonical form.
    NonCanonicalTag {
        /// Node carrying the invalid tag.
        node_id: NodeId,
        /// Invalid stored value.
        tag: String,
    },
    /// A stored inline reference has an invalid range.
    InvalidReference {
        /// Node containing the invalid reference.
        source_id: NodeId,
        /// Stored inclusive start byte.
        start: i64,
        /// Stored exclusive end byte.
        end: i64,
    },
    /// A protected Journal node no longer matches its structural identity.
    InvalidSystemNode {
        /// Invalid protected node.
        node_id: NodeId,
    },
    /// The required Journal container is missing.
    MissingJournal,
    /// Synchronization metadata or pending changes are inconsistent.
    InvalidSyncState(String),
    /// Marker indicating that additional issues were not included in the report.
    AdditionalIssuesOmitted,
}

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
    /// Requested page size is zero or exceeds [`MAX_PAGE_SIZE`].
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
            Self::NodeHasReferences(id) => {
                write!(
                    formatter,
                    "node {id} has outgoing references; replace its content instead"
                )
            }
            Self::NodeReferenced(id) => {
                write!(
                    formatter,
                    "node {id} is referenced from outside its subtree"
                )
            }
            Self::SystemNodeProtected(id) => {
                write!(formatter, "system node is protected: {id}")
            }
            Self::InvalidJournalDate(date) => {
                write!(formatter, "invalid journal date: {date:?}")
            }
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
