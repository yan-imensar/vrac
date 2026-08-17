use crate::{Cursor, NodeId};

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
    /// [`crate::Engine::create_node`] when another destination is required.
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

/// Result of creating a node and resolving complete unbound references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateOutcome {
    /// The newly created node with its canonical tags and resolved references.
    pub node: Node,
    /// New concept or Journal nodes in first-occurrence order.
    pub materialized_nodes: Vec<Node>,
}

/// Content captured as one ordinary bullet in a Journal day.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalEntry {
    /// Plain node text.
    pub text: String,
    /// Tags for the captured node. They are canonicalized by the engine.
    pub tags: Vec<String>,
    /// Stable inline references already selected by the client.
    pub references: Vec<ReferenceInput>,
}

impl JournalEntry {
    /// Creates an untagged entry with no pre-resolved references.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
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

/// One ordered page of sibling nodes.
#[derive(Clone, Debug, Eq, PartialEq)]
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

/// Integrity issue reported by [`crate::Engine::check`].
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
