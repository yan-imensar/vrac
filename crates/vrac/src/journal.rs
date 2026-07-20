use rusqlite::{Connection, OptionalExtension, params};

use crate::content::replace_tags;
use crate::db::Engine;
use crate::nodes::{decode_id, node_id_bytes, random_node_id};
use crate::order::position_for_placement;
use crate::sync::{capture_session, commit_mutation};
use crate::{CheckIssue, Error, Node, NodeId, Placement, Result, SystemNode};

const JOURNAL_KEY: &str = "journal";
const JOURNAL_DAY_PREFIX: &str = "journal-day:";

impl Engine {
    /// Returns the requested journal day, creating it atomically when needed.
    ///
    /// Journal days are protected structural nodes carrying the canonical
    /// `journal` tag. `date` must use the ISO `YYYY-MM-DD` calendar format.
    pub fn journal_day(&mut self, date: &str) -> Result<Node> {
        validate_date(date)?;
        let key = format!("{JOURNAL_DAY_PREFIX}{date}");
        if let Some(id) = system_node_id(&self.connection, &key)? {
            return self
                .node(id)?
                .ok_or_else(|| Error::InvalidDatabase("a journal day could not be read".into()));
        }

        let journal_id = journal_id(&self.connection)?;
        let captured = capture_session(&self.connection)?;
        let transaction = self.connection.unchecked_transaction()?;
        let position =
            position_for_placement(&transaction, Some(journal_id), Placement::Last, None)?;
        let id = random_node_id(&transaction)?;
        transaction.execute(
            "INSERT INTO nodes (id, parent_id, position, text, system_key)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                node_id_bytes(&id),
                node_id_bytes(&journal_id),
                position,
                date,
                key
            ],
        )?;
        replace_tags(&transaction, id, &["journal".into()])?;
        commit_mutation(transaction, captured, self.sync_device_id)?;
        self.history.clear();
        self.node(id)?.ok_or_else(|| {
            Error::InvalidDatabase("a newly created journal day could not be read".into())
        })
    }
}

pub(crate) fn create_journal(connection: &Connection) -> Result<()> {
    let id = random_node_id(connection)?;
    connection.execute(
        "INSERT INTO nodes (id, parent_id, position, text, system_key)
         VALUES (?1, NULL, 0, 'Journal', ?2)",
        params![node_id_bytes(&id), JOURNAL_KEY],
    )?;
    Ok(())
}

pub(crate) fn decode_system_key(key: Option<String>) -> Result<Option<SystemNode>> {
    match key.as_deref() {
        None => Ok(None),
        Some(JOURNAL_KEY) => Ok(Some(SystemNode::Journal)),
        Some(value) if value.starts_with(JOURNAL_DAY_PREFIX) => {
            let date = &value[JOURNAL_DAY_PREFIX.len()..];
            validate_date(date)?;
            Ok(Some(SystemNode::JournalDay { date: date.into() }))
        }
        Some(value) => Err(Error::InvalidDatabase(format!(
            "unknown system node key: {value:?}"
        ))),
    }
}

pub(crate) fn protected_node(connection: &Connection, id: NodeId) -> Result<bool> {
    connection
        .query_row(
            "SELECT system_key IS NOT NULL FROM nodes WHERE id = ?1",
            params![node_id_bytes(&id)],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(Error::NodeNotFound(id))
}

pub(crate) fn journal_container(connection: &Connection, id: NodeId) -> Result<bool> {
    connection
        .query_row(
            "SELECT COALESCE(system_key = ?2, 0) FROM nodes WHERE id = ?1",
            params![node_id_bytes(&id), JOURNAL_KEY],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(Error::NodeNotFound(id))
}

pub(crate) fn validate_system_node(connection: &Connection, id: NodeId) -> Result<bool> {
    let Some((parent, text, key)) = connection
        .query_row(
            "SELECT parent_id, text, system_key FROM nodes WHERE id = ?1",
            params![node_id_bytes(&id)],
            |row| {
                Ok((
                    row.get::<_, Option<Vec<u8>>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(true);
    };
    let system = match key.as_deref() {
        None => return Ok(true),
        Some(JOURNAL_KEY) => SystemNode::Journal,
        Some(value) if value.starts_with(JOURNAL_DAY_PREFIX) => {
            let date = &value[JOURNAL_DAY_PREFIX.len()..];
            if validate_date(date).is_err() {
                return Ok(false);
            }
            SystemNode::JournalDay { date: date.into() }
        }
        Some(_) => return Ok(false),
    };
    match system {
        SystemNode::Journal => Ok(parent.is_none() && text == "Journal"),
        SystemNode::JournalDay { date } => {
            let Some(parent) = parent else {
                return Ok(false);
            };
            let parent_key: Option<String> = connection
                .query_row(
                    "SELECT system_key FROM nodes WHERE id = ?1",
                    params![parent],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            let tagged: bool = connection.query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM node_tags WHERE node_id = ?1 AND tag = 'journal'
                 )",
                params![node_id_bytes(&id)],
                |row| row.get(0),
            )?;
            Ok(parent_key.as_deref() == Some(JOURNAL_KEY) && text == date && tagged)
        }
    }
}

pub(crate) fn check_system_nodes(
    connection: &Connection,
    maximum: usize,
) -> Result<(Vec<CheckIssue>, bool)> {
    let journal_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM nodes WHERE system_key = ?1)",
        [JOURNAL_KEY],
        |row| row.get(0),
    )?;
    let mut issues = if journal_exists {
        Vec::new()
    } else {
        vec![CheckIssue::MissingJournal]
    };
    if issues.len() > maximum {
        issues.clear();
        return Ok((issues, true));
    }
    let mut statement =
        connection.prepare("SELECT id FROM nodes WHERE system_key IS NOT NULL ORDER BY id")?;
    let ids = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
    for bytes in ids {
        let id = decode_id(bytes?)?;
        if !validate_system_node(connection, id)? {
            if issues.len() == maximum {
                return Ok((issues, true));
            }
            issues.push(CheckIssue::InvalidSystemNode { node_id: id });
        }
    }
    Ok((issues, false))
}

fn journal_id(connection: &Connection) -> Result<NodeId> {
    let bytes: Vec<u8> = connection.query_row(
        "SELECT id FROM nodes WHERE system_key = ?1",
        [JOURNAL_KEY],
        |row| row.get(0),
    )?;
    decode_id(bytes)
}

fn system_node_id(connection: &Connection, key: &str) -> Result<Option<NodeId>> {
    connection
        .query_row("SELECT id FROM nodes WHERE system_key = ?1", [key], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .optional()?
        .map(decode_id)
        .transpose()
}

fn validate_date(date: &str) -> Result<()> {
    let bytes = date.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(Error::InvalidJournalDate(date.into()));
    }
    let number = |range: std::ops::Range<usize>| -> Option<u32> {
        bytes[range].iter().try_fold(0, |value, byte| {
            byte.is_ascii_digit()
                .then_some(value * 10 + u32::from(byte - b'0'))
        })
    };
    let year = number(0..4).ok_or_else(|| Error::InvalidJournalDate(date.into()))?;
    let month = number(5..7).ok_or_else(|| Error::InvalidJournalDate(date.into()))?;
    let day = number(8..10).ok_or_else(|| Error::InvalidJournalDate(date.into()))?;
    if year == 0 {
        return Err(Error::InvalidJournalDate(date.into()));
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let maximum = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    if day == 0 || day > maximum {
        return Err(Error::InvalidJournalDate(date.into()));
    }
    Ok(())
}
