use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::content::{
    canonicalize_tags, hydrate_nodes, replace_references, replace_tags, validate_references,
};
use crate::db::Engine;
use crate::journal::{decode_system_key, journal_container, protected_node};
use crate::order::{POSITION_STEP, position_at, position_for_placement};
use crate::sync::{capture_session, commit_mutation};
use crate::{
    CreateNode, Cursor, Destination, Error, GenerateShape, MAX_PAGE_SIZE, NODE_ID_LENGTH, Node,
    NodeId, NodePage, Page, Placement, Result,
};

pub(crate) type RawNode = (Vec<u8>, Option<Vec<u8>>, i64, String, Option<String>);

struct StoredNode {
    node: Node,
    position: i64,
}

impl Engine {
    /// Creates one node atomically.
    ///
    /// The parent and any relative placement reference are validated inside
    /// the same transaction as the insertion.
    pub fn create_node(&mut self, input: CreateNode) -> Result<Node> {
        let CreateNode {
            parent_id,
            placement,
            text,
            tags,
            references,
        } = input;
        let tags = canonicalize_tags(tags)?;

        let captured = capture_session(&self.connection)?;
        let transaction = self.connection.unchecked_transaction()?;
        ensure_parent_exists(&transaction, parent_id)?;
        if let Some(parent_id) = parent_id
            && journal_container(&transaction, parent_id)?
        {
            return Err(Error::SystemNodeProtected(parent_id));
        }

        let position = position_for_placement(&transaction, parent_id, placement, None)?;
        let id = random_node_id(&transaction)?;
        let parent_bytes = parent_id.as_ref().map(node_id_bytes);

        transaction.execute(
            "INSERT INTO nodes (id, parent_id, position, text) VALUES (?1, ?2, ?3, ?4)",
            params![node_id_bytes(&id), parent_bytes, position, &text],
        )?;
        let references = validate_references(&transaction, &text, references)?;
        replace_tags(&transaction, id, &tags)?;
        replace_references(&transaction, id, &references)?;
        let changeset = commit_mutation(transaction, captured, self.sync_device_id)?;
        self.history.record(changeset);

        self.node(id)?
            .ok_or_else(|| Error::InvalidDatabase("a newly created node could not be read".into()))
    }

    /// Reads one node by its stable identifier.
    pub fn node(&self, id: NodeId) -> Result<Option<Node>> {
        let Some(stored) = stored_node(&self.connection, id)? else {
            return Ok(None);
        };
        let mut nodes = vec![stored.node];
        hydrate_nodes(&self.connection, &mut nodes)?;
        Ok(nodes.pop())
    }

    /// Reads one ordered page of children for a parent or the root.
    ///
    /// Continue with the opaque [`Cursor`] returned in [`NodePage::next`].
    /// Numeric storage positions are never exposed.
    ///
    /// # Example
    ///
    /// ```
    /// use vrac::{CreateNode, Engine, Page};
    ///
    /// let mut engine = Engine::open(":memory:")?;
    /// engine.create_node(CreateNode::new("First"))?;
    /// engine.create_node(CreateNode::new("Second"))?;
    ///
    /// let first = engine.children(None, Page { limit: 2, after: None })?;
    /// let second = engine.children(None, Page { limit: 2, after: first.next })?;
    /// let first = first.nodes.into_iter().find(|node| node.system.is_none()).unwrap();
    /// let second = second.nodes.into_iter().find(|node| node.system.is_none()).unwrap();
    /// assert_eq!(first.text, "First");
    /// assert_eq!(second.text, "Second");
    /// # Ok::<(), vrac::Error>(())
    /// ```
    pub fn children(&self, parent_id: Option<NodeId>, page: Page) -> Result<NodePage> {
        validate_page(page)?;
        ensure_parent_exists(&self.connection, parent_id)?;

        let parent_bytes = parent_id.as_ref().map(node_id_bytes);
        let query_limit = i64::try_from(page.limit + 1).map_err(|_| Error::InvalidPageLimit {
            limit: page.limit,
            maximum: MAX_PAGE_SIZE,
        })?;
        let mut stored_nodes = Vec::with_capacity(page.limit + 1);

        match page.after {
            Some(Cursor { position, id }) => {
                let mut statement = self.connection.prepare_cached(
                    "SELECT id, parent_id, position, text, system_key
                     FROM nodes
                     WHERE parent_id IS ?1 AND (position, id) > (?2, ?3)
                     ORDER BY position, id
                     LIMIT ?4",
                )?;
                let mut rows = statement.query(params![
                    parent_bytes,
                    position,
                    node_id_bytes(&id),
                    query_limit
                ])?;
                while let Some(row) = rows.next()? {
                    stored_nodes.push(decode_stored_node(raw_node(row)?)?);
                }
            }
            None => {
                let mut statement = self.connection.prepare_cached(
                    "SELECT id, parent_id, position, text, system_key
                     FROM nodes
                     WHERE parent_id IS ?1
                     ORDER BY position, id
                     LIMIT ?2",
                )?;
                let mut rows = statement.query(params![parent_bytes, query_limit])?;
                while let Some(row) = rows.next()? {
                    stored_nodes.push(decode_stored_node(raw_node(row)?)?);
                }
            }
        }

        let has_more = stored_nodes.len() > page.limit;
        if has_more {
            stored_nodes.pop();
        }
        let next = if has_more {
            stored_nodes.last().map(|last| Cursor {
                position: last.position,
                id: last.node.id,
            })
        } else {
            None
        };
        let mut nodes: Vec<Node> = stored_nodes.into_iter().map(|stored| stored.node).collect();
        hydrate_nodes(&self.connection, &mut nodes)?;

        Ok(NodePage { nodes, next })
    }

    /// Returns the path from a root node to `id`.
    ///
    /// The virtual product root is not included. Work is proportional to the
    /// node's depth and never traverses descendants.
    pub fn path(&self, id: NodeId) -> Result<Vec<Node>> {
        let mut statement = self.connection.prepare(
            "WITH RECURSIVE ancestors(id, parent_id, position, text, system_key) AS (
                 SELECT id, parent_id, position, text, system_key FROM nodes WHERE id = ?1
                 UNION
                 SELECT nodes.id, nodes.parent_id, nodes.position, nodes.text, nodes.system_key
                 FROM nodes
                 JOIN ancestors ON nodes.id = ancestors.parent_id
             )
             SELECT id, parent_id, position, text, system_key FROM ancestors",
        )?;
        let rows = statement.query_map(params![node_id_bytes(&id)], raw_node)?;
        let stored: Vec<StoredNode> = rows
            .map(|row| row.map_err(Error::from).and_then(decode_stored_node))
            .collect::<Result<_>>()?;
        if stored.is_empty() {
            return Err(Error::NodeNotFound(id));
        }

        let mut nodes_by_id: HashMap<NodeId, Node> = stored
            .into_iter()
            .map(|stored| (stored.node.id, stored.node))
            .collect();
        let mut visited = HashSet::new();
        let mut path = Vec::with_capacity(nodes_by_id.len());
        let mut current = id;
        loop {
            if !visited.insert(current) {
                return Err(Error::InvalidDatabase(
                    "a cycle exists in the requested node path".into(),
                ));
            }
            let node = nodes_by_id.remove(&current).ok_or_else(|| {
                Error::InvalidDatabase("the requested node path has a missing parent".into())
            })?;
            let parent_id = node.parent_id;
            path.push(node);
            match parent_id {
                Some(parent_id) => current = parent_id,
                None => break,
            }
        }
        path.reverse();
        hydrate_nodes(&self.connection, &mut path)?;
        Ok(path)
    }

    /// Replaces a node's text atomically.
    pub fn set_text(&mut self, id: NodeId, text: String) -> Result<()> {
        let captured = capture_session(&self.connection)?;
        let transaction = self.connection.unchecked_transaction()?;
        if protected_node(&transaction, id)? {
            return Err(Error::SystemNodeProtected(id));
        }
        let has_references: bool = transaction.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM node_references WHERE source_id = ?1
             )",
            params![node_id_bytes(&id)],
            |row| row.get(0),
        )?;
        if has_references {
            return Err(Error::NodeHasReferences(id));
        }
        let changed = transaction.execute(
            "UPDATE nodes SET text = ?1 WHERE id = ?2 AND text <> ?1",
            params![text, node_id_bytes(&id)],
        )?;
        if changed == 0 && !node_exists(&transaction, id)? {
            return Err(Error::NodeNotFound(id));
        }
        let changeset = commit_mutation(transaction, captured, self.sync_device_id)?;
        self.history.record(changeset);
        Ok(())
    }

    /// Moves a node and its subtree atomically.
    ///
    /// Descendants are not rewritten. The engine validates the destination,
    /// rejects cycles, and preserves deterministic sibling order.
    pub fn move_node(&mut self, id: NodeId, destination: Destination) -> Result<()> {
        let captured = capture_session(&self.connection)?;
        let transaction = self.connection.unchecked_transaction()?;
        let moved = stored_node(&transaction, id)?.ok_or(Error::NodeNotFound(id))?;
        if moved.node.system.is_some() {
            return Err(Error::SystemNodeProtected(id));
        }
        ensure_parent_exists(&transaction, destination.parent_id)?;
        if let Some(parent_id) = destination.parent_id
            && journal_container(&transaction, parent_id)?
        {
            return Err(Error::SystemNodeProtected(parent_id));
        }
        ensure_move_is_acyclic(&transaction, id, destination.parent_id)?;

        let references_itself = match destination.placement {
            Placement::Before(reference) | Placement::After(reference) => reference == id,
            Placement::First | Placement::Last => false,
        };
        if references_itself {
            if moved.node.parent_id != destination.parent_id {
                return Err(Error::PlacementReferenceNotSibling {
                    reference: id,
                    parent_id: destination.parent_id,
                });
            }
            let changeset = commit_mutation(transaction, captured, self.sync_device_id)?;
            self.history.record(changeset);
            return Ok(());
        }

        let position = position_for_placement(
            &transaction,
            destination.parent_id,
            destination.placement,
            Some(id),
        )?;
        let parent_bytes = destination.parent_id.as_ref().map(node_id_bytes);

        transaction.execute(
            "UPDATE nodes SET parent_id = ?1, position = ?2 WHERE id = ?3",
            params![parent_bytes, position, node_id_bytes(&id)],
        )?;
        let changeset = commit_mutation(transaction, captured, self.sync_device_id)?;
        self.history.record(changeset);
        Ok(())
    }

    /// Deletes a node and all its descendants atomically.
    ///
    /// The operation is rejected when a node in the subtree is referenced by
    /// a node outside it. References contained inside the subtree disappear
    /// with their source nodes.
    pub fn delete_node(&mut self, id: NodeId) -> Result<u64> {
        let captured = capture_session(&self.connection)?;
        let transaction = self.connection.unchecked_transaction()?;
        if !node_exists(&transaction, id)? {
            return Err(Error::NodeNotFound(id));
        }
        if protected_node(&transaction, id)? {
            return Err(Error::SystemNodeProtected(id));
        }

        let external_target = transaction
            .query_row(
                "WITH RECURSIVE subtree(id) AS (
                     SELECT ?1
                     UNION
                     SELECT nodes.id FROM nodes JOIN subtree ON nodes.parent_id = subtree.id
                 )
                 SELECT refs.target_id
                 FROM node_references AS refs
                 JOIN subtree AS targets ON targets.id = refs.target_id
                 LEFT JOIN subtree AS sources ON sources.id = refs.source_id
                 WHERE sources.id IS NULL
                 LIMIT 1",
                params![node_id_bytes(&id)],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(decode_id)
            .transpose()?;
        if let Some(target_id) = external_target {
            return Err(Error::NodeReferenced(target_id));
        }

        let count: i64 = transaction.query_row(
            "WITH RECURSIVE subtree(id) AS (
                 SELECT ?1
                 UNION
                 SELECT nodes.id FROM nodes JOIN subtree ON nodes.parent_id = subtree.id
             )
             SELECT COUNT(*) FROM subtree",
            params![node_id_bytes(&id)],
            |row| row.get(0),
        )?;
        transaction.execute(
            "WITH RECURSIVE subtree(id) AS (
                 SELECT ?1
                 UNION
                 SELECT nodes.id FROM nodes JOIN subtree ON nodes.parent_id = subtree.id
             )
             DELETE FROM nodes WHERE id IN (SELECT id FROM subtree)",
            params![node_id_bytes(&id)],
        )?;
        let changeset = commit_mutation(transaction, captured, self.sync_device_id)?;
        self.history.record(changeset);
        u64::try_from(count)
            .map_err(|_| Error::InvalidDatabase("SQLite returned a negative delete count".into()))
    }

    /// Searches node text using a bounded full-text prefix query.
    ///
    /// Empty or punctuation-only queries return no results. FTS stops as soon
    /// as the requested result limit is reached.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Node>> {
        validate_search_limit(limit)?;
        let Some(query) = search_query(query) else {
            return Ok(Vec::new());
        };
        let limit = i64::try_from(limit).map_err(|_| Error::InvalidPageLimit {
            limit,
            maximum: MAX_PAGE_SIZE,
        })?;
        let mut statement = self.connection.prepare_cached(
            "SELECT nodes.id, nodes.parent_id, nodes.position, nodes.text, nodes.system_key
             FROM node_search
             JOIN nodes ON nodes.rowid = node_search.rowid
             WHERE node_search MATCH ?1
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![query, limit], raw_node)?;
        let stored = rows
            .map(|row| row.map_err(Error::from).and_then(decode_stored_node))
            .collect::<Result<Vec<_>>>()?;
        let mut nodes = stored
            .into_iter()
            .map(|stored| stored.node)
            .collect::<Vec<_>>();
        hydrate_nodes(&self.connection, &mut nodes)?;
        Ok(nodes)
    }

    /// Generates performance or test data in a single transaction.
    ///
    /// This utility does not participate in checkpoint restoration or normal
    /// product behavior.
    pub fn generate_nodes(&mut self, count: u64, shape: GenerateShape) -> Result<()> {
        if count == 0 {
            return Ok(());
        }

        let count_usize = usize::try_from(count).map_err(|_| Error::GenerationTooLarge(count))?;
        let transaction = self.connection.transaction()?;
        let first_root_position = next_position(&transaction, None)?;

        let mut generated_ids = Vec::new();
        if shape == GenerateShape::Mixed {
            generated_ids
                .try_reserve_exact(count_usize)
                .map_err(|_| Error::GenerationTooLarge(count))?;
        }
        let mut previous_id = None;

        let mut insert = transaction.prepare(
            "INSERT INTO nodes (id, parent_id, position, text)
             VALUES (randomblob(16), ?1, ?2, ?3)
             RETURNING id",
        )?;

        for index in 0..count_usize {
            let (parent_id, position) = match shape {
                GenerateShape::Wide => (None, position_at(first_root_position, index)?),
                GenerateShape::Deep => {
                    let position = if index == 0 { first_root_position } else { 0 };
                    (previous_id, position)
                }
                GenerateShape::Mixed => {
                    if index == 0 {
                        (None, first_root_position)
                    } else {
                        let parent_index = (index - 1) / 10;
                        let sibling_index = (index - 1) % 10;
                        (
                            Some(generated_ids[parent_index]),
                            position_at(0, sibling_index)?,
                        )
                    }
                }
            };

            let parent_bytes = parent_id.as_ref().map(node_id_bytes);
            let text = format!("Generated node {}", index + 1);
            let raw_id: Vec<u8> =
                insert.query_row(params![parent_bytes, position, text], |row| row.get(0))?;
            let id = decode_id(raw_id)?;

            if shape == GenerateShape::Mixed {
                generated_ids.push(id);
            }
            previous_id = Some(id);
        }

        drop(insert);
        transaction.commit()?;
        Ok(())
    }
}

pub(crate) fn raw_node(row: &Row<'_>) -> rusqlite::Result<RawNode> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

pub(crate) fn decode_node((id, parent_id, _position, text, system_key): RawNode) -> Result<Node> {
    Ok(Node {
        id: decode_id(id)?,
        parent_id: parent_id.map(decode_id).transpose()?,
        has_children: false,
        text,
        system: decode_system_key(system_key)?,
        tags: Vec::new(),
        references: Vec::new(),
    })
}

fn decode_stored_node(raw: RawNode) -> Result<StoredNode> {
    let position = raw.2;
    Ok(StoredNode {
        node: decode_node(raw)?,
        position,
    })
}

pub(crate) fn decode_id(bytes: Vec<u8>) -> Result<NodeId> {
    let bytes: [u8; NODE_ID_LENGTH] = bytes.try_into().map_err(|bytes: Vec<u8>| {
        Error::InvalidDatabase(format!(
            "an identifier contains {} bytes instead of {NODE_ID_LENGTH}",
            bytes.len()
        ))
    })?;
    Ok(NodeId::from_bytes(bytes))
}

pub(crate) fn random_node_id(connection: &Connection) -> Result<NodeId> {
    let bytes: Vec<u8> = connection.query_row("SELECT randomblob(16)", [], |row| row.get(0))?;
    decode_id(bytes)
}

pub(crate) fn node_id_bytes(id: &NodeId) -> &[u8] {
    &id.as_bytes()[..]
}

pub(crate) fn node_exists(connection: &Connection, id: NodeId) -> Result<bool> {
    connection
        .query_row(
            "SELECT 1 FROM nodes WHERE id = ?1",
            params![node_id_bytes(&id)],
            |_| Ok(true),
        )
        .optional()
        .map(|exists| exists.unwrap_or(false))
        .map_err(Error::from)
}

fn stored_node(connection: &Connection, id: NodeId) -> Result<Option<StoredNode>> {
    let raw = connection
        .query_row(
            "SELECT id, parent_id, position, text, system_key FROM nodes WHERE id = ?1",
            params![node_id_bytes(&id)],
            raw_node,
        )
        .optional()?;
    raw.map(decode_stored_node).transpose()
}

pub(crate) fn ensure_parent_exists(
    connection: &Connection,
    parent_id: Option<NodeId>,
) -> Result<()> {
    if let Some(parent_id) = parent_id
        && !node_exists(connection, parent_id)?
    {
        return Err(Error::ParentNotFound(parent_id));
    }
    Ok(())
}

fn validate_search_limit(limit: usize) -> Result<()> {
    if limit == 0 || limit > MAX_PAGE_SIZE {
        return Err(Error::InvalidPageLimit {
            limit,
            maximum: MAX_PAGE_SIZE,
        });
    }
    Ok(())
}

fn search_query(value: &str) -> Option<String> {
    let terms = value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| term.chars().count() >= 2)
        .map(|term| format!("\"{term}\"*"))
        .collect::<Vec<_>>();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

fn next_position(connection: &Connection, parent_id: Option<NodeId>) -> Result<i64> {
    let parent_bytes = parent_id.as_ref().map(node_id_bytes);
    let maximum: Option<i64> = connection.query_row(
        "SELECT MAX(position) FROM nodes WHERE parent_id IS ?1",
        params![parent_bytes],
        |row| row.get(0),
    )?;

    match maximum {
        Some(maximum) => maximum
            .checked_add(POSITION_STEP)
            .ok_or(Error::PositionOverflow),
        None => Ok(0),
    }
}

fn ensure_move_is_acyclic(
    connection: &Connection,
    moved_id: NodeId,
    destination_parent_id: Option<NodeId>,
) -> Result<()> {
    let Some(destination_parent_id) = destination_parent_id else {
        return Ok(());
    };

    let (contains_moved_node, reaches_root): (bool, bool) = connection.query_row(
        "WITH RECURSIVE ancestors(id, parent_id) AS (
             SELECT id, parent_id FROM nodes WHERE id = ?1
             UNION
             SELECT nodes.id, nodes.parent_id
             FROM nodes
             JOIN ancestors ON nodes.id = ancestors.parent_id
         )
         SELECT
             EXISTS(SELECT 1 FROM ancestors WHERE id = ?2),
             EXISTS(SELECT 1 FROM ancestors WHERE parent_id IS NULL)",
        params![
            node_id_bytes(&destination_parent_id),
            node_id_bytes(&moved_id)
        ],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    if contains_moved_node {
        return Err(Error::Cycle);
    }
    if !reaches_root {
        return Err(Error::InvalidDatabase(
            "a cycle already exists among the destination's ancestors".into(),
        ));
    }

    Ok(())
}

fn validate_page(page: Page) -> Result<()> {
    if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
        return Err(Error::InvalidPageLimit {
            limit: page.limit,
            maximum: MAX_PAGE_SIZE,
        });
    }
    Ok(())
}
