use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::db::Engine;
use crate::order::{POSITION_STEP, position_at, position_for_placement};
use crate::{
    CreateNode, Cursor, Destination, Error, GenerateShape, MAX_PAGE_SIZE, NODE_ID_LENGTH, Node,
    NodeId, NodePage, Page, Placement, Result,
};

type RawNode = (Vec<u8>, Option<Vec<u8>>, i64, String);

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
        } = input;

        let transaction = self.connection.transaction()?;
        ensure_parent_exists(&transaction, parent_id)?;

        let position = position_for_placement(&transaction, parent_id, placement, None)?;
        let id = random_node_id(&transaction)?;
        let parent_bytes = parent_id.as_ref().map(node_id_bytes);

        transaction.execute(
            "INSERT INTO nodes (id, parent_id, position, text) VALUES (?1, ?2, ?3, ?4)",
            params![node_id_bytes(&id), parent_bytes, position, &text],
        )?;
        transaction.commit()?;

        Ok(Node {
            id,
            parent_id,
            text,
        })
    }

    /// Reads one node by its stable identifier.
    pub fn node(&self, id: NodeId) -> Result<Option<Node>> {
        stored_node(&self.connection, id).map(|node| node.map(|stored| stored.node))
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
    /// let first = engine.children(None, Page { limit: 1, after: None })?;
    /// let second = engine.children(None, Page { limit: 1, after: first.next })?;
    /// assert_eq!(first.nodes[0].text, "First");
    /// assert_eq!(second.nodes[0].text, "Second");
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
                    "SELECT id, parent_id, position, text
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
                    "SELECT id, parent_id, position, text
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
        let nodes = stored_nodes.into_iter().map(|stored| stored.node).collect();

        Ok(NodePage { nodes, next })
    }

    /// Replaces a node's text atomically.
    pub fn set_text(&mut self, id: NodeId, text: String) -> Result<()> {
        let transaction = self.connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE nodes SET text = ?1 WHERE id = ?2",
            params![text, node_id_bytes(&id)],
        )?;
        if changed == 0 {
            return Err(Error::NodeNotFound(id));
        }
        transaction.commit()?;
        Ok(())
    }

    /// Moves a node and its subtree atomically.
    ///
    /// Descendants are not rewritten. The engine validates the destination,
    /// rejects cycles, and preserves deterministic sibling order.
    pub fn move_node(&mut self, id: NodeId, destination: Destination) -> Result<()> {
        let transaction = self.connection.transaction()?;
        let moved = stored_node(&transaction, id)?.ok_or(Error::NodeNotFound(id))?;
        ensure_parent_exists(&transaction, destination.parent_id)?;
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
            transaction.commit()?;
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
        transaction.commit()?;
        Ok(())
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

fn raw_node(row: &Row<'_>) -> rusqlite::Result<RawNode> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

fn decode_stored_node((id, parent_id, position, text): RawNode) -> Result<StoredNode> {
    Ok(StoredNode {
        node: Node {
            id: decode_id(id)?,
            parent_id: parent_id.map(decode_id).transpose()?,
            text,
        },
        position,
    })
}

fn decode_id(bytes: Vec<u8>) -> Result<NodeId> {
    let bytes: [u8; NODE_ID_LENGTH] = bytes.try_into().map_err(|bytes: Vec<u8>| {
        Error::InvalidDatabase(format!(
            "an identifier contains {} bytes instead of {NODE_ID_LENGTH}",
            bytes.len()
        ))
    })?;
    Ok(NodeId::from_bytes(bytes))
}

fn random_node_id(connection: &Connection) -> Result<NodeId> {
    let bytes: Vec<u8> = connection.query_row("SELECT randomblob(16)", [], |row| row.get(0))?;
    decode_id(bytes)
}

fn node_id_bytes(id: &NodeId) -> &[u8] {
    &id.as_bytes()[..]
}

fn node_exists(connection: &Connection, id: NodeId) -> Result<bool> {
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
            "SELECT id, parent_id, position, text FROM nodes WHERE id = ?1",
            params![node_id_bytes(&id)],
            raw_node,
        )
        .optional()?;
    raw.map(decode_stored_node).transpose()
}

fn ensure_parent_exists(connection: &Connection, parent_id: Option<NodeId>) -> Result<()> {
    if let Some(parent_id) = parent_id
        && !node_exists(connection, parent_id)?
    {
        return Err(Error::ParentNotFound(parent_id));
    }
    Ok(())
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
