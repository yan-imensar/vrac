use std::error::Error as StdError;
use std::fmt;
use std::str::FromStr;

use crate::error::{Error, Result};

/// Number of bytes in a node identifier.
pub const NODE_ID_LENGTH: usize = 16;

/// Number of bytes in a synchronization device identifier.
pub const SYNC_DEVICE_ID_LENGTH: usize = 16;

/// Number of bytes in a workspace identifier.
pub const WORKSPACE_ID_LENGTH: usize = 16;

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

/// Error returned when parsing a textual node identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
