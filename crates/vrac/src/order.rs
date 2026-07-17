use rusqlite::{Connection, OptionalExtension, params};

use crate::{Error, NodeId, Placement, Result};

pub(crate) const POSITION_STEP: i64 = 1_024;

enum PositionDecision {
    Available(i64),
    Renumber,
}

pub(crate) fn position_for_placement(
    connection: &Connection,
    parent_id: Option<NodeId>,
    placement: Placement,
    excluded_id: Option<NodeId>,
) -> Result<i64> {
    match try_position_for_placement(connection, parent_id, placement, excluded_id)? {
        PositionDecision::Available(position) => Ok(position),
        PositionDecision::Renumber => {
            renumber_siblings(connection, parent_id, excluded_id)?;
            match try_position_for_placement(connection, parent_id, placement, excluded_id)? {
                PositionDecision::Available(position) => Ok(position),
                PositionDecision::Renumber => Err(Error::PositionOverflow),
            }
        }
    }
}

fn try_position_for_placement(
    connection: &Connection,
    parent_id: Option<NodeId>,
    placement: Placement,
    excluded_id: Option<NodeId>,
) -> Result<PositionDecision> {
    match placement {
        Placement::First => match edge_position(connection, parent_id, excluded_id, true)? {
            Some(first) => Ok(position_before(first)),
            None => Ok(PositionDecision::Available(0)),
        },
        Placement::Last => match edge_position(connection, parent_id, excluded_id, false)? {
            Some(last) => Ok(position_after(last)),
            None => Ok(PositionDecision::Available(0)),
        },
        Placement::Before(reference) => {
            let reference_position = reference_position(connection, parent_id, reference)?;
            match adjacent_position(
                connection,
                parent_id,
                reference,
                reference_position,
                excluded_id,
                true,
            )? {
                Some(previous) => Ok(position_between(previous, reference_position)),
                None => Ok(position_before(reference_position)),
            }
        }
        Placement::After(reference) => {
            let reference_position = reference_position(connection, parent_id, reference)?;
            match adjacent_position(
                connection,
                parent_id,
                reference,
                reference_position,
                excluded_id,
                false,
            )? {
                Some(next) => Ok(position_between(reference_position, next)),
                None => Ok(position_after(reference_position)),
            }
        }
    }
}

fn edge_position(
    connection: &Connection,
    parent_id: Option<NodeId>,
    excluded_id: Option<NodeId>,
    first: bool,
) -> Result<Option<i64>> {
    let sql = if first {
        "SELECT position
         FROM nodes
         WHERE parent_id IS ?1 AND id IS NOT ?2
         ORDER BY position, id
         LIMIT 1"
    } else {
        "SELECT position
         FROM nodes
         WHERE parent_id IS ?1 AND id IS NOT ?2
         ORDER BY position DESC, id DESC
         LIMIT 1"
    };
    let parent_bytes = parent_id.as_ref().map(node_id_bytes);
    let excluded_bytes = excluded_id.as_ref().map(node_id_bytes);
    connection
        .query_row(sql, params![parent_bytes, excluded_bytes], |row| row.get(0))
        .optional()
        .map_err(Error::from)
}

fn reference_position(
    connection: &Connection,
    parent_id: Option<NodeId>,
    reference: NodeId,
) -> Result<i64> {
    let raw: Option<(Option<Vec<u8>>, i64)> = connection
        .query_row(
            "SELECT parent_id, position FROM nodes WHERE id = ?1",
            params![node_id_bytes(&reference)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((actual_parent_id, position)) = raw else {
        return Err(Error::NodeNotFound(reference));
    };
    let expected_parent_id = parent_id.as_ref().map(node_id_bytes);
    if actual_parent_id.as_deref() != expected_parent_id {
        return Err(Error::PlacementReferenceNotSibling {
            reference,
            parent_id,
        });
    }
    Ok(position)
}

fn adjacent_position(
    connection: &Connection,
    parent_id: Option<NodeId>,
    reference_id: NodeId,
    reference_position: i64,
    excluded_id: Option<NodeId>,
    previous: bool,
) -> Result<Option<i64>> {
    let sql = if previous {
        "SELECT position
         FROM nodes
         WHERE parent_id IS ?1
           AND id IS NOT ?2
           AND (position, id) < (?3, ?4)
         ORDER BY position DESC, id DESC
         LIMIT 1"
    } else {
        "SELECT position
         FROM nodes
         WHERE parent_id IS ?1
           AND id IS NOT ?2
           AND (position, id) > (?3, ?4)
         ORDER BY position, id
         LIMIT 1"
    };
    let parent_bytes = parent_id.as_ref().map(node_id_bytes);
    let excluded_bytes = excluded_id.as_ref().map(node_id_bytes);
    connection
        .query_row(
            sql,
            params![
                parent_bytes,
                excluded_bytes,
                reference_position,
                node_id_bytes(&reference_id)
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(Error::from)
}

fn position_before(first: i64) -> PositionDecision {
    first
        .checked_sub(POSITION_STEP)
        .map_or(PositionDecision::Renumber, PositionDecision::Available)
}

fn position_after(last: i64) -> PositionDecision {
    last.checked_add(POSITION_STEP)
        .map_or(PositionDecision::Renumber, PositionDecision::Available)
}

fn position_between(previous: i64, next: i64) -> PositionDecision {
    let gap = i128::from(next) - i128::from(previous);
    if gap <= 1 {
        return PositionDecision::Renumber;
    }
    let position = i128::from(previous) + gap / 2;
    match i64::try_from(position) {
        Ok(position) => PositionDecision::Available(position),
        Err(_) => PositionDecision::Renumber,
    }
}

fn renumber_siblings(
    connection: &Connection,
    parent_id: Option<NodeId>,
    excluded_id: Option<NodeId>,
) -> Result<()> {
    let parent_bytes = parent_id.as_ref().map(node_id_bytes);
    let excluded_bytes = excluded_id.as_ref().map(node_id_bytes);
    let mut select = connection.prepare(
        "SELECT id
         FROM nodes
         WHERE parent_id IS ?1 AND id IS NOT ?2
         ORDER BY position, id",
    )?;
    let mut rows = select.query(params![parent_bytes, excluded_bytes])?;
    let mut sibling_ids = Vec::new();
    while let Some(row) = rows.next()? {
        sibling_ids.push(row.get::<_, Vec<u8>>(0)?);
    }
    drop(rows);
    drop(select);

    let mut update = connection.prepare("UPDATE nodes SET position = ?1 WHERE id = ?2")?;
    for (index, id) in sibling_ids.into_iter().enumerate() {
        update.execute(params![position_at(0, index)?, id])?;
    }
    Ok(())
}

pub(crate) fn position_at(start: i64, index: usize) -> Result<i64> {
    let index = i64::try_from(index).map_err(|_| Error::PositionOverflow)?;
    let offset = index
        .checked_mul(POSITION_STEP)
        .ok_or(Error::PositionOverflow)?;
    start.checked_add(offset).ok_or(Error::PositionOverflow)
}

fn node_id_bytes(id: &NodeId) -> &[u8] {
    &id.as_bytes()[..]
}
