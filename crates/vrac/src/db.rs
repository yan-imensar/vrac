use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{
    CheckIssue, CheckReport, CreateNode, Cursor, Destination, Error, GenerateShape, MAX_PAGE_SIZE,
    NODE_ID_LENGTH, Node, NodeId, Page, Result,
};

const SCHEMA_VERSION: i64 = 1;
const POSITION_STEP: i64 = 1_024;
const MAX_REPORTED_ISSUES: usize = 100;

type RawNode = (Vec<u8>, Option<Vec<u8>>, i64, String);

pub struct Engine {
    connection: Connection,
}

impl Engine {
    /// Opens an existing Vrac database or creates a new one.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut connection = Connection::open(path)?;
        connection.pragma_update(None, "foreign_keys", true)?;

        migrate(&mut connection)?;
        validate_schema(&connection)?;

        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;

        Ok(Self { connection })
    }

    pub fn create_node(&mut self, input: CreateNode) -> Result<Node> {
        let CreateNode {
            parent_id,
            position,
            text,
        } = input;

        let transaction = self.connection.transaction()?;
        ensure_parent_exists(&transaction, parent_id)?;

        let position = match position {
            Some(position) => position,
            None => next_position(&transaction, parent_id)?,
        };
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
            position,
            text,
        })
    }

    pub fn node(&self, id: NodeId) -> Result<Option<Node>> {
        let raw = self
            .connection
            .query_row(
                "SELECT id, parent_id, position, text FROM nodes WHERE id = ?1",
                params![node_id_bytes(&id)],
                raw_node,
            )
            .optional()?;

        raw.map(decode_node).transpose()
    }

    pub fn children(&self, parent_id: Option<NodeId>, page: Page) -> Result<Vec<Node>> {
        validate_page(page)?;
        ensure_parent_exists(&self.connection, parent_id)?;

        let parent_bytes = parent_id.as_ref().map(node_id_bytes);
        let limit = i64::try_from(page.limit).map_err(|_| Error::InvalidPageLimit {
            limit: page.limit,
            maximum: MAX_PAGE_SIZE,
        })?;
        let mut nodes = Vec::with_capacity(page.limit);

        match page.after {
            Some(Cursor { position, id }) => {
                let mut statement = self.connection.prepare_cached(
                    "SELECT id, parent_id, position, text
                     FROM nodes
                     WHERE parent_id IS ?1 AND (position, id) > (?2, ?3)
                     ORDER BY position, id
                     LIMIT ?4",
                )?;
                let mut rows =
                    statement.query(params![parent_bytes, position, node_id_bytes(&id), limit])?;
                while let Some(row) = rows.next()? {
                    nodes.push(decode_node(raw_node(row)?)?);
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
                let mut rows = statement.query(params![parent_bytes, limit])?;
                while let Some(row) = rows.next()? {
                    nodes.push(decode_node(raw_node(row)?)?);
                }
            }
        }

        Ok(nodes)
    }

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

    pub fn move_node(&mut self, id: NodeId, destination: Destination) -> Result<()> {
        let transaction = self.connection.transaction()?;
        if !node_exists(&transaction, id)? {
            return Err(Error::NodeNotFound(id));
        }
        ensure_parent_exists(&transaction, destination.parent_id)?;
        ensure_move_is_acyclic(&transaction, id, destination.parent_id)?;

        let position = match destination.position {
            Some(position) => position,
            None => next_position(&transaction, destination.parent_id)?,
        };
        let parent_bytes = destination.parent_id.as_ref().map(node_id_bytes);

        transaction.execute(
            "UPDATE nodes SET parent_id = ?1, position = ?2 WHERE id = ?3",
            params![parent_bytes, position, node_id_bytes(&id)],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Generates test data in a single transaction.
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

    /// Checks SQLite, foreign keys, and the absence of rootless components.
    pub fn check(&self) -> Result<CheckReport> {
        let mut issues = Vec::new();

        let mut integrity = self.connection.prepare("PRAGMA integrity_check")?;
        let messages = integrity.query_map([], |row| row.get::<_, String>(0))?;
        for message in messages {
            let message = message?;
            if message != "ok" {
                issues.push(CheckIssue::SqliteIntegrity(message));
            }
        }
        drop(integrity);

        let mut foreign_keys = self.connection.prepare("PRAGMA foreign_key_check")?;
        let mut rows = foreign_keys.query([])?;
        let mut foreign_key_issue_count = 0_usize;
        while let Some(row) = rows.next()? {
            if foreign_key_issue_count == MAX_REPORTED_ISSUES {
                issues.push(CheckIssue::AdditionalIssuesOmitted);
                break;
            }
            issues.push(CheckIssue::ForeignKey {
                table: row.get(0)?,
                rowid: row.get(1)?,
                parent: row.get(2)?,
                foreign_key_index: row.get(3)?,
            });
            foreign_key_issue_count += 1;
        }
        drop(rows);
        drop(foreign_keys);

        let (node_count, reachable_count): (i64, i64) = self.connection.query_row(
            "WITH RECURSIVE reachable(id) AS (
                 SELECT id FROM nodes WHERE parent_id IS NULL
                 UNION ALL
                 SELECT nodes.id
                 FROM nodes
                 JOIN reachable ON nodes.parent_id = reachable.id
             )
             SELECT
                 (SELECT COUNT(*) FROM nodes),
                 (SELECT COUNT(*) FROM reachable)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let node_count = u64::try_from(node_count)
            .map_err(|_| Error::InvalidDatabase("SQLite returned a negative node count".into()))?;
        let reachable_count = u64::try_from(reachable_count).map_err(|_| {
            Error::InvalidDatabase("SQLite returned a negative reachable node count".into())
        })?;
        if reachable_count > node_count {
            return Err(Error::InvalidDatabase(
                "the integrity check found more reachable nodes than total nodes".into(),
            ));
        }
        if reachable_count < node_count {
            issues.push(CheckIssue::UnreachableNodes(node_count - reachable_count));
        }

        Ok(CheckReport { node_count, issues })
    }
}

fn migrate(connection: &mut Connection) -> Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;

    match version {
        0 if database_is_empty(connection)? => {
            let transaction = connection.transaction()?;
            transaction.execute_batch(include_str!("../schema.sql"))?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.commit()?;
            Ok(())
        }
        0 => Err(Error::InvalidDatabase(
            "the database already contains objects but has no schema version".into(),
        )),
        SCHEMA_VERSION => Ok(()),
        version => Err(Error::UnsupportedSchemaVersion(version)),
    }
}

fn database_is_empty(connection: &Connection) -> Result<bool> {
    let object_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    Ok(object_count == 0)
}

fn validate_schema(connection: &Connection) -> Result<()> {
    connection
        .prepare("SELECT id, parent_id, position, text FROM nodes LIMIT 0")
        .map_err(|error| Error::InvalidDatabase(error.to_string()))?;

    let has_parent_index: bool = connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_schema
             WHERE type = 'index'
               AND name = 'nodes_by_parent'
               AND tbl_name = 'nodes'
         )",
        [],
        |row| row.get(0),
    )?;
    if !has_parent_index {
        return Err(Error::InvalidDatabase(
            "the nodes_by_parent index is missing".into(),
        ));
    }
    Ok(())
}

fn raw_node(row: &Row<'_>) -> rusqlite::Result<RawNode> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

fn decode_node((id, parent_id, position, text): RawNode) -> Result<Node> {
    Ok(Node {
        id: decode_id(id)?,
        parent_id: parent_id.map(decode_id).transpose()?,
        position,
        text,
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

fn position_at(start: i64, index: usize) -> Result<i64> {
    let index = i64::try_from(index).map_err(|_| Error::PositionOverflow)?;
    let offset = index
        .checked_mul(POSITION_STEP)
        .ok_or(Error::PositionOverflow)?;
    start.checked_add(offset).ok_or(Error::PositionOverflow)
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
