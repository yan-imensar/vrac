//! Vrac's local-first engine.
//!
//! The library owns the model, business rules, and SQLite storage. It is
//! synchronous and performs no user-facing input or output.
//!
//! # Example
//!
//! ```
//! use vrac_engine::{CreateNode, Engine, Page};
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
//! # Ok::<(), vrac_engine::Error>(())
//! ```

#![deny(missing_docs)]

mod backlinks;
mod checkpoint;
mod clipboard;
mod content;
mod db;
mod error;
mod history;
mod identity;
mod journal;
mod model;
mod nodes;
mod order;
mod pagination;
mod schema;
mod sync;

pub use db::Engine;
pub use error::{Error, Result};
pub use identity::{
    NODE_ID_LENGTH, NodeId, ParseNodeIdError, ParseWorkspaceIdError, SYNC_DEVICE_ID_LENGTH,
    SyncDeviceId, WORKSPACE_ID_LENGTH, WorkspaceId,
};
pub use model::{
    BacklinkContext, BacklinkPage, BacklinkTag, CheckIssue, CheckReport, ContentUpdate, CreateNode,
    DeleteOutcome, Destination, GenerateShape, JournalEntry, Node, NodePage, NodeReference,
    Placement, ReferenceInput, SystemNode,
};
pub use pagination::{Cursor, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, Page, ParseCursorError};
pub use sync::{OutgoingSyncPackage, SyncApply};
