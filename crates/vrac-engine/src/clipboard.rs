use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params};

use crate::content::{
    canonicalize_tags, hydrate_nodes, materialize_references, push_history_step,
    replace_references, replace_tags,
};
use crate::db::Engine;
use crate::journal::journal_container;
use crate::nodes::{
    decode_id, decode_node, ensure_parent_exists, node_id_bytes, random_node_id, raw_node,
};
use crate::order::position_for_placement;
use crate::sync::{capture_session, commit_mutation};
use crate::{Destination, Error, Node, NodeId, Placement, Result};

struct StoredCopy {
    node: Node,
    position: i64,
}

struct ParsedNode {
    depth: usize,
    text: String,
    tags: Vec<String>,
}

impl Engine {
    /// Serializes the selected subtrees as portable indented plain-text bullets.
    ///
    /// Descendants of another selected node are emitted only once. Work and
    /// memory are proportional to the explicit selection and its descendants.
    pub fn copy_nodes(&self, ids: &[NodeId]) -> Result<String> {
        if ids.is_empty() {
            return Err(Error::InvalidClipboard("no node is selected".into()));
        }
        let selected: HashSet<NodeId> = ids.iter().copied().collect();
        let mut roots = Vec::new();
        let mut seen_roots = HashSet::new();
        for &id in ids {
            let parent_id = self
                .connection
                .query_row(
                    "SELECT parent_id FROM nodes WHERE id = ?1",
                    params![node_id_bytes(&id)],
                    |row| row.get::<_, Option<Vec<u8>>>(0),
                )
                .optional()?
                .ok_or(Error::NodeNotFound(id))?
                .map(decode_id)
                .transpose()?;
            if !has_selected_ancestor(&self.connection, parent_id, &selected)?
                && seen_roots.insert(id)
            {
                roots.push(id);
            }
        }
        if roots.is_empty() {
            return Err(Error::InvalidDatabase(
                "selected nodes do not contain an acyclic root".into(),
            ));
        }

        let mut output = String::new();
        for root in roots {
            let mut stored = load_subtree(&self.connection, root)?;
            let mut nodes = stored
                .iter()
                .map(|item| item.node.clone())
                .collect::<Vec<_>>();
            hydrate_nodes(&self.connection, &mut nodes)?;
            for (item, node) in stored.iter_mut().zip(nodes) {
                item.node = node;
            }
            let by_id = stored
                .iter()
                .map(|item| (item.node.id, item.node.clone()))
                .collect::<HashMap<_, _>>();
            let mut children: HashMap<NodeId, Vec<(i64, NodeId)>> = HashMap::new();
            for item in &stored {
                if let Some(parent_id) = item.node.parent_id {
                    children
                        .entry(parent_id)
                        .or_default()
                        .push((item.position, item.node.id));
                }
            }
            for siblings in children.values_mut() {
                siblings.sort_unstable_by_key(|(position, id)| (*position, *id));
            }
            write_subtree(root, &by_id, &children, &mut output)?;
        }
        output.pop();
        Ok(output)
    }

    /// Creates an outline from portable indented clipboard text atomically.
    ///
    /// Returned nodes are the newly created top-level roots in input order.
    pub fn paste_nodes(&mut self, destination: Destination, text: &str) -> Result<Vec<Node>> {
        let parsed = parse_outline(text)?;
        let captured = capture_session(&self.connection)?;
        let transaction = self.connection.unchecked_transaction()?;
        ensure_parent_exists(&transaction, destination.parent_id)?;
        if let Some(parent_id) = destination.parent_id
            && journal_container(&transaction, parent_id)?
        {
            return Err(Error::SystemNodeProtected(parent_id));
        }

        let mut parents = Vec::<NodeId>::new();
        let mut roots = Vec::new();
        let mut previous_root = None;
        let mut history = Vec::new();
        for item in parsed {
            parents.truncate(item.depth);
            let parent_id = if item.depth == 0 {
                destination.parent_id
            } else {
                Some(parents.get(item.depth - 1).copied().ok_or_else(|| {
                    Error::InvalidClipboard("indentation skips a parent level".into())
                })?)
            };
            let placement = if item.depth == 0 {
                previous_root.map_or(destination.placement, Placement::After)
            } else {
                Placement::Last
            };
            let position = position_for_placement(&transaction, parent_id, placement, None)?;
            let id = random_node_id(&transaction)?;
            let parent_bytes = parent_id.as_ref().map(node_id_bytes);
            let node_step = capture_session(&transaction)?;
            transaction.execute(
                "INSERT INTO nodes (id, parent_id, position, text) VALUES (?1, ?2, ?3, ?4)",
                params![node_id_bytes(&id), parent_bytes, position, &item.text],
            )?;
            push_history_step(node_step, &mut history)?;

            let materialize_step = capture_session(&transaction)?;
            let (references, _) = materialize_references(&transaction, &item.text, Vec::new())?;
            push_history_step(materialize_step, &mut history)?;
            let reference_step = capture_session(&transaction)?;
            replace_references(&transaction, id, &references)?;
            push_history_step(reference_step, &mut history)?;

            let tags = canonicalize_tags(item.tags)?;
            if !tags.is_empty() {
                let tag_step = capture_session(&transaction)?;
                replace_tags(&transaction, id, &tags)?;
                push_history_step(tag_step, &mut history)?;
            }
            if item.depth == 0 {
                roots.push(id);
                previous_root = Some(id);
            }
            parents.push(id);
        }

        let changeset = commit_mutation(transaction, captured, self.sync_device_id)?;
        if changeset.is_some() {
            self.history.record_group(history);
        }
        roots
            .into_iter()
            .map(|id| {
                self.node(id)?
                    .ok_or_else(|| Error::InvalidDatabase("a pasted node could not be read".into()))
            })
            .collect()
    }
}

fn has_selected_ancestor(
    connection: &Connection,
    mut parent_id: Option<NodeId>,
    selected: &HashSet<NodeId>,
) -> Result<bool> {
    let mut visited = HashSet::new();
    while let Some(id) = parent_id {
        if !visited.insert(id) {
            return Err(Error::InvalidDatabase(
                "a cycle exists in a copied node path".into(),
            ));
        }
        if selected.contains(&id) {
            return Ok(true);
        }
        parent_id = connection
            .query_row(
                "SELECT parent_id FROM nodes WHERE id = ?1",
                params![node_id_bytes(&id)],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )?
            .map(decode_id)
            .transpose()?;
    }
    Ok(false)
}

fn load_subtree(connection: &Connection, root: NodeId) -> Result<Vec<StoredCopy>> {
    let mut statement = connection.prepare(
        "WITH RECURSIVE subtree(id, parent_id, position, text, system_key) AS (
             SELECT id, parent_id, position, text, system_key FROM nodes WHERE id = ?1
             UNION
             SELECT nodes.id, nodes.parent_id, nodes.position, nodes.text, nodes.system_key
             FROM nodes JOIN subtree ON nodes.parent_id = subtree.id
         )
         SELECT id, parent_id, position, text, system_key FROM subtree",
    )?;
    let rows = statement.query_map(params![node_id_bytes(&root)], raw_node)?;
    rows.map(|row| {
        let raw = row?;
        let position = raw.2;
        Ok(StoredCopy {
            node: decode_node(raw)?,
            position,
        })
    })
    .collect()
}

fn write_subtree(
    root: NodeId,
    nodes: &HashMap<NodeId, Node>,
    children: &HashMap<NodeId, Vec<(i64, NodeId)>>,
    output: &mut String,
) -> Result<()> {
    let mut pending = vec![(root, 0_usize)];
    let mut visited = HashSet::new();
    while let Some((id, depth)) = pending.pop() {
        if !visited.insert(id) {
            return Err(Error::InvalidDatabase(
                "a cycle exists in a copied subtree".into(),
            ));
        }
        let node = nodes.get(&id).ok_or_else(|| {
            Error::InvalidDatabase("a copied subtree contains a missing node".into())
        })?;
        if node.text.contains('\r') || node.text.contains('\n') {
            return Err(Error::InvalidClipboard(format!(
                "node {id} contains a line break"
            )));
        }
        output.push_str(&"  ".repeat(depth));
        output.push_str("- ");
        output.push_str(&node.text);
        for tag in &node.tags {
            output.push_str(" #");
            output.push_str(tag);
        }
        output.push('\n');
        if let Some(child_ids) = children.get(&id) {
            pending.extend(
                child_ids
                    .iter()
                    .rev()
                    .map(|(_, child_id)| (*child_id, depth + 1)),
            );
        }
    }
    Ok(())
}

fn parse_outline(text: &str) -> Result<Vec<ParsedNode>> {
    let mut result = Vec::new();
    let mut indents = Vec::<usize>::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let indent = line
            .chars()
            .take_while(|character| matches!(character, ' ' | '\t'))
            .map(|character| if character == '\t' { 4 } else { 1 })
            .sum::<usize>();
        let mut content = line.trim_start();
        if content.starts_with("- ") || content.starts_with("* ") || content.starts_with("+ ") {
            content = &content[2..];
        }

        if indents.last().is_none_or(|current| indent > *current) {
            indents.push(indent);
        } else {
            while indents.last().is_some_and(|current| *current > indent) {
                indents.pop();
            }
            if indents.last() != Some(&indent) {
                return Err(Error::InvalidClipboard(
                    "indentation does not match an earlier level".into(),
                ));
            }
        }

        let mut tags = Vec::new();
        let mut cursor = content.len();
        let mut tag_boundary = None;
        loop {
            let trimmed = content[..cursor].trim_end();
            let token_start = trimmed
                .char_indices()
                .rev()
                .find(|(_, character)| character.is_whitespace())
                .map_or(0, |(index, character)| index + character.len_utf8());
            let token = &trimmed[token_start..];
            if !token.starts_with('#') || token.len() == 1 {
                break;
            }
            tags.push(token[1..].to_owned());
            tag_boundary = Some(token_start);
            cursor = token_start;
        }
        tags.reverse();
        let text_end = tag_boundary.map_or(content.len(), |boundary| boundary.saturating_sub(1));
        let text = content[..text_end].to_owned();
        result.push(ParsedNode {
            depth: indents.len() - 1,
            text,
            tags,
        });
    }
    if result.is_empty() {
        return Err(Error::InvalidClipboard("the clipboard is empty".into()));
    }
    Ok(result)
}
