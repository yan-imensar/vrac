use std::collections::HashMap;

use rusqlite::{Connection, params, params_from_iter};

use crate::db::Engine;
use crate::nodes::{decode_id, node_exists, node_id_bytes};
use crate::sync::{capture_session, commit_mutation};
use crate::{
    CheckIssue, Error, MAX_PAGE_SIZE, Node, NodeId, NodeReference, ReferenceInput, Result,
};

const HYDRATION_BATCH_SIZE: usize = 1_000;

impl Engine {
    /// Replaces a node's text and outgoing references atomically.
    ///
    /// Reference ranges address the labels between `[[` and `]]` in `text`.
    /// Existing tags are preserved.
    ///
    /// # Example
    ///
    /// ```
    /// use vrac::{CreateNode, Engine, ReferenceInput};
    ///
    /// let mut engine = Engine::open(":memory:")?;
    /// let target = engine.create_node(CreateNode::new("Project X"))?;
    /// let source = engine.create_node(CreateNode::new("Draft"))?;
    /// let text = "Point on [[Project X]]";
    /// let start = text.find("Project X").unwrap();
    /// engine.set_content(source.id, text.into(), vec![ReferenceInput {
    ///     label_start: start,
    ///     label_end: start + "Project X".len(),
    ///     target_id: target.id,
    /// }])?;
    /// assert_eq!(engine.node(source.id)?.unwrap().references[0].target_text, "Project X");
    /// # Ok::<(), vrac::Error>(())
    /// ```
    pub fn set_content(
        &mut self,
        id: NodeId,
        text: String,
        references: Vec<ReferenceInput>,
    ) -> Result<()> {
        let captured = capture_session(&self.connection, self.sync_device_id)?;
        let transaction = self.connection.unchecked_transaction()?;
        if !node_exists(&transaction, id)? {
            return Err(Error::NodeNotFound(id));
        }
        let references = validate_references(&transaction, &text, references)?;
        transaction.execute(
            "UPDATE nodes SET text = ?1 WHERE id = ?2",
            params![&text, node_id_bytes(&id)],
        )?;
        replace_references(&transaction, id, &references)?;
        commit_mutation(transaction, captured, self.sync_device_id)?;
        Ok(())
    }

    /// Replaces a node's unordered tag set atomically.
    ///
    /// Tags are trimmed, converted to Unicode lowercase, sorted, and
    /// deduplicated. Empty tags, whitespace, and `#` are rejected.
    pub fn set_tags(&mut self, id: NodeId, tags: Vec<String>) -> Result<()> {
        let tags = canonicalize_tags(tags)?;
        let captured = capture_session(&self.connection, self.sync_device_id)?;
        let transaction = self.connection.unchecked_transaction()?;
        if !node_exists(&transaction, id)? {
            return Err(Error::NodeNotFound(id));
        }
        replace_tags(&transaction, id, &tags)?;
        commit_mutation(transaction, captured, self.sync_device_id)?;
        Ok(())
    }

    /// Returns canonical tags beginning with `prefix` in lexical order.
    ///
    /// The result is bounded and uses the inverse tag index. An empty prefix
    /// returns the first tags, which is useful when opening a completion list.
    pub fn tags(&self, prefix: &str, limit: usize) -> Result<Vec<String>> {
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(Error::InvalidPageLimit {
                limit,
                maximum: MAX_PAGE_SIZE,
            });
        }
        let prefix = canonicalize_tag_prefix(prefix)?;
        let mut tags = Vec::with_capacity(limit);
        if prefix.is_empty() {
            let mut statement = self.connection.prepare_cached(
                "SELECT DISTINCT tag
                 FROM node_tags INDEXED BY node_tags_by_tag
                 ORDER BY tag
                 LIMIT ?1",
            )?;
            let rows = statement.query_map([limit as i64], |row| row.get(0))?;
            for tag in rows {
                tags.push(tag?);
            }
            return Ok(tags);
        }

        let upper = lexical_successor(&prefix);
        let sql = if upper.is_some() {
            "SELECT DISTINCT tag
             FROM node_tags INDEXED BY node_tags_by_tag
             WHERE tag >= ?1 AND tag < ?2
             ORDER BY tag
             LIMIT ?3"
        } else {
            "SELECT DISTINCT tag
             FROM node_tags INDEXED BY node_tags_by_tag
             WHERE tag >= ?1
             ORDER BY tag
             LIMIT ?2"
        };
        let mut statement = self.connection.prepare_cached(sql)?;
        let mut rows = match upper {
            Some(upper) => statement.query(params![prefix, upper, limit as i64])?,
            None => statement.query(params![prefix, limit as i64])?,
        };
        while let Some(row) = rows.next()? {
            tags.push(row.get(0)?);
        }
        Ok(tags)
    }
}

pub(crate) fn canonicalize_tags(tags: Vec<String>) -> Result<Vec<String>> {
    let mut canonical = Vec::with_capacity(tags.len());
    for original in tags {
        canonical.push(canonicalize_tag(&original)?);
    }
    canonical.sort_unstable();
    canonical.dedup();
    Ok(canonical)
}

fn canonicalize_tag(original: &str) -> Result<String> {
    let tag: String = original
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .collect();
    if tag.is_empty()
        || tag
            .chars()
            .any(|character| character.is_whitespace() || character == '#')
    {
        return Err(Error::InvalidTag(original.into()));
    }
    Ok(tag)
}

fn canonicalize_tag_prefix(original: &str) -> Result<String> {
    let prefix: String = original
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .collect();
    if prefix
        .chars()
        .any(|character| character.is_whitespace() || character == '#')
    {
        return Err(Error::InvalidTag(original.into()));
    }
    Ok(prefix)
}

fn lexical_successor(value: &str) -> Option<String> {
    let mut characters: Vec<char> = value.chars().collect();
    for index in (0..characters.len()).rev() {
        let mut scalar = characters[index] as u32 + 1;
        if scalar == 0xd800 {
            scalar = 0xe000;
        }
        if let Some(next) = char::from_u32(scalar) {
            characters[index] = next;
            characters.truncate(index + 1);
            return Some(characters.into_iter().collect());
        }
    }
    None
}

pub(crate) fn check_content(
    connection: &Connection,
    maximum: usize,
) -> Result<(Vec<CheckIssue>, bool)> {
    let mut issues = Vec::new();
    let mut omitted = false;

    let mut tags =
        connection.prepare("SELECT node_id, tag FROM node_tags ORDER BY node_id, tag")?;
    let mut rows = tags.query([])?;
    while let Some(row) = rows.next()? {
        let node_id = decode_id(row.get(0)?)?;
        let tag: String = row.get(1)?;
        let valid = canonicalize_tag(&tag).is_ok_and(|canonical| canonical == tag);
        if !valid
            && !push_issue(
                &mut issues,
                maximum,
                CheckIssue::NonCanonicalTag { node_id, tag },
            )
        {
            omitted = true;
            break;
        }
    }
    drop(rows);
    drop(tags);

    if !omitted {
        let mut references = connection.prepare(
            "SELECT links.source_id, links.start_byte, links.end_byte, sources.text
             FROM node_references AS links
             LEFT JOIN nodes AS sources ON sources.id = links.source_id
             ORDER BY links.source_id, links.start_byte",
        )?;
        let mut rows = references.query([])?;
        let mut previous_source = None;
        let mut previous_end = 0_i64;
        while let Some(row) = rows.next()? {
            let source_id = decode_id(row.get(0)?)?;
            let start: i64 = row.get(1)?;
            let end: i64 = row.get(2)?;
            let text: Option<String> = row.get(3)?;
            let overlaps = previous_source == Some(source_id) && start < previous_end;
            let valid_range = usize::try_from(start)
                .ok()
                .zip(usize::try_from(end).ok())
                .zip(text.as_deref())
                .is_some_and(|((start, end), text)| reference_range_is_valid(text, start, end));
            if (overlaps || !valid_range)
                && !push_issue(
                    &mut issues,
                    maximum,
                    CheckIssue::InvalidReference {
                        source_id,
                        start,
                        end,
                    },
                )
            {
                omitted = true;
                break;
            }
            previous_source = Some(source_id);
            previous_end = end;
        }
    }

    Ok((issues, omitted))
}

fn push_issue(issues: &mut Vec<CheckIssue>, maximum: usize, issue: CheckIssue) -> bool {
    if issues.len() == maximum {
        return false;
    }
    issues.push(issue);
    true
}

pub(crate) fn validate_references(
    connection: &Connection,
    text: &str,
    mut references: Vec<ReferenceInput>,
) -> Result<Vec<ReferenceInput>> {
    references.sort_unstable_by_key(|reference| {
        (
            reference.label_start,
            reference.label_end,
            reference.target_id,
        )
    });

    let mut previous_end = 0;
    for (index, reference) in references.iter().enumerate() {
        if !reference_range_is_valid(text, reference.label_start, reference.label_end) {
            return Err(Error::InvalidReferenceRange {
                start: reference.label_start,
                end: reference.label_end,
            });
        }
        if index > 0 && reference.label_start < previous_end {
            return Err(Error::OverlappingReferences);
        }
        if !node_exists(connection, reference.target_id)? {
            return Err(Error::ReferenceTargetNotFound(reference.target_id));
        }
        previous_end = reference.label_end;
    }
    Ok(references)
}

pub(crate) fn replace_tags(
    connection: &Connection,
    node_id: NodeId,
    tags: &[String],
) -> Result<()> {
    connection.execute(
        "DELETE FROM node_tags WHERE node_id = ?1",
        params![node_id_bytes(&node_id)],
    )?;
    let mut insert = connection.prepare("INSERT INTO node_tags (node_id, tag) VALUES (?1, ?2)")?;
    for tag in tags {
        insert.execute(params![node_id_bytes(&node_id), tag])?;
    }
    Ok(())
}

pub(crate) fn replace_references(
    connection: &Connection,
    source_id: NodeId,
    references: &[ReferenceInput],
) -> Result<()> {
    connection.execute(
        "DELETE FROM node_references WHERE source_id = ?1",
        params![node_id_bytes(&source_id)],
    )?;
    let mut insert = connection.prepare(
        "INSERT INTO node_references (source_id, start_byte, end_byte, target_id)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for reference in references {
        let start =
            i64::try_from(reference.label_start).map_err(|_| Error::InvalidReferenceRange {
                start: reference.label_start,
                end: reference.label_end,
            })?;
        let end = i64::try_from(reference.label_end).map_err(|_| Error::InvalidReferenceRange {
            start: reference.label_start,
            end: reference.label_end,
        })?;
        insert.execute(params![
            node_id_bytes(&source_id),
            start,
            end,
            node_id_bytes(&reference.target_id)
        ])?;
    }
    Ok(())
}

pub(crate) fn hydrate_nodes(connection: &Connection, nodes: &mut [Node]) -> Result<()> {
    if nodes.is_empty() {
        return Ok(());
    }

    let node_indices: HashMap<NodeId, usize> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id, index))
        .collect();
    let node_ids: Vec<NodeId> = nodes.iter().map(|node| node.id).collect();
    for node_ids in node_ids.chunks(HYDRATION_BATCH_SIZE) {
        hydrate_metadata(connection, node_ids, nodes, &node_indices)?;
    }
    Ok(())
}

fn hydrate_metadata(
    connection: &Connection,
    node_ids: &[NodeId],
    nodes: &mut [Node],
    node_indices: &HashMap<NodeId, usize>,
) -> Result<()> {
    let parameters = std::iter::repeat_n("?", node_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let metadata_sql = format!(
        "SELECT tags.node_id AS owner_id, 0 AS kind,
                0 AS start_byte, 0 AS end_byte,
                NULL AS target_id, tags.tag AS value
         FROM node_tags AS tags
         WHERE tags.node_id IN ({parameters})
         UNION ALL
         SELECT links.source_id AS owner_id, 1 AS kind,
                links.start_byte, links.end_byte,
                links.target_id, targets.text AS value
         FROM node_references AS links
         LEFT JOIN nodes AS targets ON targets.id = links.target_id
         WHERE links.source_id IN ({parameters})
         UNION ALL
         SELECT children.parent_id AS owner_id, 2 AS kind,
                0 AS start_byte, 0 AS end_byte,
                NULL AS target_id, NULL AS value
         FROM nodes AS children
         WHERE children.parent_id IN ({parameters})
         GROUP BY children.parent_id
         ORDER BY owner_id, kind, start_byte, value"
    );
    let parameters = node_ids
        .iter()
        .chain(node_ids)
        .chain(node_ids)
        .map(node_id_bytes);
    let mut metadata = connection.prepare_cached(&metadata_sql)?;
    let mut rows = metadata.query(params_from_iter(parameters))?;
    while let Some(row) = rows.next()? {
        let node_id = decode_id(row.get(0)?)?;
        let index = node_indices.get(&node_id).copied().ok_or_else(|| {
            Error::InvalidDatabase("metadata belongs to an unexpected node".into())
        })?;
        let kind: i64 = row.get(1)?;
        if kind == 2 {
            nodes[index].has_children = true;
            continue;
        }
        if kind == 0 {
            nodes[index].tags.push(row.get(5)?);
            continue;
        }
        if kind != 1 {
            return Err(Error::InvalidDatabase("unknown node metadata kind".into()));
        }

        let start_value: i64 = row.get(2)?;
        let end_value: i64 = row.get(3)?;
        let label_start = usize::try_from(start_value)
            .map_err(|_| Error::InvalidDatabase("a reference has a negative start byte".into()))?;
        let label_end = usize::try_from(end_value)
            .map_err(|_| Error::InvalidDatabase("a reference has a negative end byte".into()))?;
        if !reference_range_is_valid(&nodes[index].text, label_start, label_end)
            || nodes[index]
                .references
                .last()
                .is_some_and(|previous| previous.label_end > label_start)
        {
            return Err(Error::InvalidDatabase(format!(
                "node {node_id} contains an invalid reference range"
            )));
        }
        let target_id = decode_id(row.get(4)?)?;
        let target_text: Option<String> = row.get(5)?;
        let target_text = target_text.ok_or_else(|| {
            Error::InvalidDatabase(format!(
                "node {node_id} references missing node {target_id}"
            ))
        })?;
        nodes[index].references.push(NodeReference {
            label_start,
            label_end,
            target_id,
            target_text,
        });
    }
    Ok(())
}

pub(crate) fn reference_range_is_valid(text: &str, start: usize, end: usize) -> bool {
    start < end
        && text.is_char_boundary(start)
        && text.is_char_boundary(end)
        && start >= 2
        && end.checked_add(2).is_some_and(|after| after <= text.len())
        && text.get(start - 2..start) == Some("[[")
        && text.get(end..end + 2) == Some("]]")
}
