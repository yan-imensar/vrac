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

mod db;
mod nodes;
mod order;

use std::error::Error as StdError;
use std::fmt;
use std::str::FromStr;

pub use db::Engine;

/// Number of bytes in a node identifier.
pub const NODE_ID_LENGTH: usize = 16;

/// Default result page size.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// Maximum accepted result page size.
pub const MAX_PAGE_SIZE: usize = 1_000;

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
        if value.len() != NODE_ID_LENGTH * 2 {
            return Err(ParseNodeIdError);
        }

        let mut bytes = [0_u8; NODE_ID_LENGTH];
        for (index, byte) in bytes.iter_mut().enumerate() {
            let start = index * 2;
            *byte =
                u8::from_str_radix(&value[start..start + 2], 16).map_err(|_| ParseNodeIdError)?;
        }
        Ok(Self(bytes))
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

/// A text item in the Vrac tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    /// Stable identity of the node.
    pub id: NodeId,
    /// Parent identity, or `None` for a root node.
    pub parent_id: Option<NodeId>,
    /// Plain text stored by the node.
    pub text: String,
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
/// Cursors are continuation state, not a persistent or exchange format. A
/// client may retain one while loading a sibling list but cannot inspect or
/// construct it.
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
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// Result type returned by Vrac engine operations.
pub type Result<T> = std::result::Result<T, Error>;
