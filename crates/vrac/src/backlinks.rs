use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, params, params_from_iter};

use crate::content::{canonicalize_tag, hydrate_nodes};
use crate::db::Engine;
use crate::nodes::{RawNode, decode_id, decode_node, node_exists, node_id_bytes};
use crate::{
    BacklinkContext, BacklinkPage, BacklinkTag, Cursor, Error, MAX_PAGE_SIZE, Node, NodeId, Page,
    Result,
};

impl Engine {
    /// Reads contextual backlinks to `target`.
    ///
    /// Without a tag, each match is a node that directly references `target`.
    /// With a tag, each match is a tagged node in the subtree of a direct
    /// reference. This lets an ancestor reference provide context to its
    /// descendants without copying or propagating metadata.
    ///
    /// Results include the complete ancestor path, are ordered by newest
    /// journal day first, and use bounded cursor pagination.
    pub fn backlinks(&self, target: NodeId, tag: Option<&str>, page: Page) -> Result<BacklinkPage> {
        validate_page(page)?;
        if !node_exists(&self.connection, target)? {
            return Err(Error::NodeNotFound(target));
        }
        let tag = tag.map(canonicalize_tag).transpose()?;
        let query_limit = i64::try_from(page.limit + 1).map_err(|_| Error::InvalidPageLimit {
            limit: page.limit,
            maximum: MAX_PAGE_SIZE,
        })?;
        let mut matches = matching_nodes(
            &self.connection,
            target,
            tag.as_deref(),
            page.after,
            query_limit,
        )?;

        let has_more = matches.len() > page.limit;
        if has_more {
            matches.pop();
        }
        let next = if has_more {
            let (id, sort_key) = matches.last().ok_or_else(|| {
                Error::InvalidDatabase("a backlink page lost its continuation row".into())
            })?;
            Some(Cursor {
                position: *sort_key,
                id: *id,
            })
        } else {
            None
        };
        let match_ids = matches.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        let contexts = contextual_paths(&self.connection, &match_ids)?;
        Ok(BacklinkPage { contexts, next })
    }

    /// Returns the most frequent tags inside `target`'s backlink scopes.
    ///
    /// A direct reference opens a downward scope. Counts include the source
    /// node and its descendants, deduplicate overlapping scopes, and never
    /// include tags elsewhere in the workspace. Results are ordered by
    /// descending count and then canonical tag value.
    pub fn backlink_tags(&self, target: NodeId, limit: usize) -> Result<Vec<BacklinkTag>> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(Error::InvalidPageLimit {
                limit,
                maximum: MAX_PAGE_SIZE,
            });
        }
        if !node_exists(&self.connection, target)? {
            return Err(Error::NodeNotFound(target));
        }
        let limit = i64::try_from(limit).map_err(|_| Error::InvalidPageLimit {
            limit,
            maximum: MAX_PAGE_SIZE,
        })?;
        let mut statement = self.connection.prepare_cached(
            "WITH RECURSIVE scope(id) AS (
                 SELECT source_id
                 FROM node_references INDEXED BY node_references_by_target
                 WHERE target_id = ?1
                 UNION
                 SELECT nodes.id
                 FROM nodes INDEXED BY nodes_by_parent
                 JOIN scope ON nodes.parent_id = scope.id
             )
             SELECT tags.tag, COUNT(*) AS occurrence_count
             FROM scope
             JOIN node_tags AS tags ON tags.node_id = scope.id
             GROUP BY tags.tag
             ORDER BY occurrence_count DESC, tags.tag
             LIMIT ?2",
        )?;
        let mut rows = statement.query(params![node_id_bytes(&target), limit])?;
        let mut tags = Vec::new();
        while let Some(row) = rows.next()? {
            let count: i64 = row.get(1)?;
            tags.push(BacklinkTag {
                tag: row.get(0)?,
                count: u64::try_from(count).map_err(|_| {
                    Error::InvalidDatabase("SQLite returned a negative backlink tag count".into())
                })?,
            });
        }
        Ok(tags)
    }
}

fn matching_nodes(
    connection: &Connection,
    target: NodeId,
    tag: Option<&str>,
    after: Option<Cursor>,
    limit: i64,
) -> Result<Vec<(NodeId, i64)>> {
    let matches = if tag.is_some() {
        "scope(id) AS (
             SELECT source_id
             FROM node_references INDEXED BY node_references_by_target
             WHERE target_id = ?1
             UNION
             SELECT nodes.id
             FROM nodes INDEXED BY nodes_by_parent
             JOIN scope ON nodes.parent_id = scope.id
         ),
         matches(id) AS (
             SELECT scope.id
             FROM node_tags AS tags INDEXED BY node_tags_by_tag
             JOIN scope ON scope.id = tags.node_id
             WHERE tags.tag = ?2
         )"
    } else {
        "matches(id) AS (
             SELECT DISTINCT source_id
             FROM node_references INDEXED BY node_references_by_target
             WHERE target_id = ?1
         )"
    };
    let sql = format!(
        "WITH RECURSIVE {matches},
         ancestors(match_id, id, parent_id, system_key) AS (
             SELECT matches.id, nodes.id, nodes.parent_id, nodes.system_key
             FROM matches JOIN nodes ON nodes.id = matches.id
             UNION ALL
             SELECT ancestors.match_id, nodes.id, nodes.parent_id, nodes.system_key
             FROM nodes JOIN ancestors ON nodes.id = ancestors.parent_id
         ),
         ranked(match_id, sort_key) AS (
             SELECT match_id,
                    COALESCE(MAX(
                        CASE WHEN system_key GLOB 'journal-day:????-??-??'
                             THEN CAST(REPLACE(SUBSTR(system_key, 13), '-', '') AS INTEGER)
                        END
                    ), 0)
             FROM ancestors
             GROUP BY match_id
         )
         SELECT match_id, sort_key
         FROM ranked
         WHERE (?3 IS NULL OR sort_key < ?3 OR (sort_key = ?3 AND match_id > ?4))
         ORDER BY sort_key DESC, match_id
         LIMIT ?5"
    );
    let after_key = after.map(|cursor| cursor.position);
    let after_id = after.map(|cursor| node_id_bytes(&cursor.id).to_vec());
    let mut statement = connection.prepare(&sql)?;
    let mut rows = if let Some(tag) = tag {
        statement.query(params![
            node_id_bytes(&target),
            tag,
            after_key,
            after_id,
            limit
        ])?
    } else {
        statement.query(params![
            node_id_bytes(&target),
            rusqlite::types::Null,
            after_key,
            after_id,
            limit
        ])?
    };
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        result.push((decode_id(row.get(0)?)?, row.get(1)?));
    }
    Ok(result)
}

fn contextual_paths(connection: &Connection, match_ids: &[NodeId]) -> Result<Vec<BacklinkContext>> {
    if match_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", match_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "WITH RECURSIVE ancestors(match_id, id, parent_id, position, text, system_key) AS (
             SELECT nodes.id, nodes.id, nodes.parent_id, nodes.position, nodes.text, nodes.system_key
             FROM nodes WHERE nodes.id IN ({placeholders})
             UNION ALL
             SELECT ancestors.match_id, nodes.id, nodes.parent_id, nodes.position, nodes.text,
                    nodes.system_key
             FROM nodes JOIN ancestors ON nodes.id = ancestors.parent_id
         )
         SELECT match_id, id, parent_id, position, text, system_key FROM ancestors"
    );
    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query(params_from_iter(match_ids.iter().map(node_id_bytes)))?;
    let mut parents_by_match: HashMap<NodeId, HashMap<NodeId, Node>> = HashMap::new();
    let mut unique_nodes = HashMap::new();
    while let Some(row) = rows.next()? {
        let match_id = decode_id(row.get(0)?)?;
        let node = decode_node(raw_node_at_offset(row, 1)?)?;
        unique_nodes.entry(node.id).or_insert_with(|| node.clone());
        parents_by_match
            .entry(match_id)
            .or_default()
            .insert(node.id, node);
    }

    let mut hydrated = unique_nodes.into_values().collect::<Vec<_>>();
    hydrate_nodes(connection, &mut hydrated)?;
    let hydrated = hydrated
        .into_iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();

    match_ids
        .iter()
        .map(|match_id| {
            let nodes = parents_by_match.remove(match_id).ok_or_else(|| {
                Error::InvalidDatabase("a backlink match has no readable path".into())
            })?;
            let mut current = *match_id;
            let mut visited = HashSet::new();
            let mut path = Vec::with_capacity(nodes.len());
            loop {
                if !visited.insert(current) {
                    return Err(Error::InvalidDatabase(
                        "a cycle exists in a backlink context path".into(),
                    ));
                }
                let node = nodes.get(&current).ok_or_else(|| {
                    Error::InvalidDatabase("a backlink context has a missing parent".into())
                })?;
                path.push(hydrated.get(&current).cloned().ok_or_else(|| {
                    Error::InvalidDatabase("a backlink context could not be hydrated".into())
                })?);
                match node.parent_id {
                    Some(parent_id) => current = parent_id,
                    None => break,
                }
            }
            path.reverse();
            Ok(BacklinkContext { path })
        })
        .collect()
}

fn raw_node_at_offset(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<RawNode> {
    Ok((
        row.get(offset)?,
        row.get(offset + 1)?,
        row.get(offset + 2)?,
        row.get(offset + 3)?,
        row.get(offset + 4)?,
    ))
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
