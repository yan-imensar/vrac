use std::error::Error as StdError;
use std::fmt;
use std::str::FromStr;

use crate::NodeId;

/// Default result page size.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// Maximum accepted result page size.
pub const MAX_PAGE_SIZE: usize = 1_000;

/// Opaque cursor returned by a paginated read.
///
/// Cursors are continuation state, not a persistent storage format. Clients
/// may pass their textual representation unchanged across an IPC boundary.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Cursor {
    pub(crate) position: i64,
    pub(crate) id: NodeId,
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

/// Request for one bounded page of children.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
