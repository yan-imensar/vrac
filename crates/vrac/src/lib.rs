//! Vrac's local-first engine.
//!
//! The library owns the model, business rules, and SQLite storage. It is
//! synchronous and performs no user-facing input or output.

mod db;

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
    pub fn from_bytes(bytes: [u8; NODE_ID_LENGTH]) -> Self {
        Self(bytes)
    }

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
pub struct ParseNodeIdError;

impl fmt::Display for ParseNodeIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a node identifier must contain exactly 32 hexadecimal characters")
    }
}

impl StdError for ParseNodeIdError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub parent_id: Option<NodeId>,
    pub position: i64,
    pub text: String,
}

/// Data required to create a node.
///
/// When no position is supplied, the node is appended to its siblings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateNode {
    pub parent_id: Option<NodeId>,
    pub position: Option<i64>,
    pub text: String,
}

impl CreateNode {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            parent_id: None,
            position: None,
            text: text.into(),
        }
    }
}

/// Destination of a move operation.
///
/// When no position is supplied, the node is appended to its new siblings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Destination {
    pub parent_id: Option<NodeId>,
    pub position: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cursor {
    pub position: i64,
    pub id: NodeId,
}

impl From<&Node> for Cursor {
    fn from(node: &Node) -> Self {
        Self {
            position: node.position,
            id: node.id,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Page {
    pub limit: usize,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerateShape {
    /// All nodes are placed at the root.
    Wide,
    /// Every node is a child of the preceding node.
    Deep,
    /// Balanced tree with at most ten children per node.
    Mixed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckReport {
    pub node_count: u64,
    pub issues: Vec<CheckIssue>,
}

impl CheckReport {
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckIssue {
    SqliteIntegrity(String),
    ForeignKey {
        table: String,
        rowid: Option<i64>,
        parent: String,
        foreign_key_index: i64,
    },
    UnreachableNodes(u64),
    AdditionalIssuesOmitted,
}

#[derive(Debug)]
pub enum Error {
    Sqlite(rusqlite::Error),
    InvalidDatabase(String),
    UnsupportedSchemaVersion(i64),
    NodeNotFound(NodeId),
    ParentNotFound(NodeId),
    Cycle,
    InvalidPageLimit { limit: usize, maximum: usize },
    PositionOverflow,
    GenerationTooLarge(u64),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
            Self::InvalidDatabase(reason) => write!(formatter, "invalid Vrac database: {reason}"),
            Self::UnsupportedSchemaVersion(version) => {
                write!(formatter, "unsupported schema version: {version}")
            }
            Self::NodeNotFound(id) => write!(formatter, "node not found: {id}"),
            Self::ParentNotFound(id) => write!(formatter, "parent not found: {id}"),
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

pub type Result<T> = std::result::Result<T, Error>;
