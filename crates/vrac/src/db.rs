use std::path::Path;

use rusqlite::Connection;

use crate::{CheckIssue, CheckReport, Error, Result};

const SCHEMA_VERSION: i64 = 1;
const APPLICATION_ID: i64 = 0x5652_4143;
const MAX_REPORTED_ISSUES: usize = 100;
const SCHEMA_SQL: &str = include_str!("../schema.sql");

/// Synchronous owner of one Vrac workspace connection.
///
/// Business writes are atomic and methods never print or terminate the
/// process. A graphical client should run the engine outside its UI thread.
pub struct Engine {
    pub(crate) connection: Connection,
}

impl Engine {
    /// Opens an existing Vrac workspace or creates a new one.
    ///
    /// Existing files are accepted only when their application identifier,
    /// schema version, and canonical schema match Vrac's format. File-backed
    /// workspaces use foreign keys, WAL journaling, and `synchronous = FULL`.
    ///
    /// # Example
    ///
    /// ```
    /// let engine = vrac::Engine::open(":memory:")?;
    /// assert!(engine.check()?.is_ok());
    /// # Ok::<(), vrac::Error>(())
    /// ```
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut connection = Connection::open(path)?;
        enable_foreign_keys(&connection)?;
        prepare_database(&mut connection)?;
        configure_persistence(&connection)?;

        Ok(Self { connection })
    }

    /// Checks SQLite integrity, foreign keys, and root reachability.
    ///
    /// This operation traverses the complete workspace and is intentionally
    /// more expensive than normal reads and mutations.
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

fn prepare_database(connection: &mut Connection) -> Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let application_id: i64 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;

    if application_id != 0 && application_id != APPLICATION_ID {
        return Err(Error::InvalidDatabase(format!(
            "application ID 0x{application_id:08x} does not identify a Vrac database"
        )));
    }

    match version {
        0 if application_id == 0 && database_is_empty(connection)? => {
            let transaction = connection.transaction()?;
            transaction.execute_batch(SCHEMA_SQL)?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
            transaction.commit()?;
            Ok(())
        }
        0 => Err(Error::InvalidDatabase(
            "the database has no schema version but is not an empty, unmarked file".into(),
        )),
        SCHEMA_VERSION => {
            validate_schema(connection)?;
            if application_id == 0 {
                let transaction = connection.transaction()?;
                transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
                transaction.commit()?;
            }
            Ok(())
        }
        version => Err(Error::UnsupportedSchemaVersion(version)),
    }
}

fn enable_foreign_keys(connection: &Connection) -> Result<()> {
    connection.pragma_update(None, "foreign_keys", true)?;
    let enabled: i64 = connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    if enabled != 1 {
        return Err(Error::StorageConfiguration(
            "SQLite foreign key enforcement could not be enabled".into(),
        ));
    }
    Ok(())
}

fn configure_persistence(connection: &Connection) -> Result<()> {
    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") && !journal_mode.eq_ignore_ascii_case("memory") {
        return Err(Error::StorageConfiguration(format!(
            "SQLite selected journal mode {journal_mode:?} instead of WAL"
        )));
    }

    connection.pragma_update(None, "synchronous", "FULL")?;
    let synchronous: i64 = connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    if synchronous != 2 {
        return Err(Error::StorageConfiguration(format!(
            "SQLite selected synchronous level {synchronous} instead of FULL"
        )));
    }
    Ok(())
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
    let expected_statements: Vec<String> = SCHEMA_SQL
        .split(';')
        .filter_map(|statement| {
            let statement = statement.trim();
            (!statement.is_empty()).then(|| normalize_sql(statement))
        })
        .collect();
    if expected_statements.len() != 2 {
        return Err(Error::InvalidDatabase(
            "the embedded schema definition is invalid".into(),
        ));
    }

    let expected = [
        ("table", "nodes", "nodes", expected_statements[0].as_str()),
        (
            "index",
            "nodes_by_parent",
            "nodes",
            expected_statements[1].as_str(),
        ),
    ];
    let mut statement = connection.prepare(
        "SELECT type, name, tbl_name, sql
         FROM sqlite_schema
         WHERE name NOT LIKE 'sqlite_%'
         ORDER BY name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    let actual: Vec<_> = rows.collect::<rusqlite::Result<_>>()?;

    if actual.len() != expected.len() {
        return Err(Error::InvalidDatabase(format!(
            "schema version {SCHEMA_VERSION} contains {} application objects instead of {}",
            actual.len(),
            expected.len()
        )));
    }
    for ((kind, name, table, sql), (actual_kind, actual_name, actual_table, actual_sql)) in
        expected.into_iter().zip(actual)
    {
        if actual_kind != kind || actual_name != name || actual_table != table {
            return Err(Error::InvalidDatabase(format!(
                "schema object {actual_name:?} does not belong to schema version {SCHEMA_VERSION}"
            )));
        }
        if actual_sql.as_deref().map(normalize_sql).as_deref() != Some(sql) {
            return Err(Error::InvalidDatabase(format!(
                "schema object {name:?} does not match schema version {SCHEMA_VERSION}"
            )));
        }
    }

    connection
        .prepare("SELECT id, parent_id, position, text FROM nodes LIMIT 0")
        .map_err(|error| Error::InvalidDatabase(error.to_string()))?;
    Ok(())
}

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_connections_enforce_required_pragmas() {
        let engine = Engine::open(":memory:").expect("open in-memory database");
        let foreign_keys: i64 = engine
            .connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("read foreign_keys");
        let journal_mode: String = engine
            .connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("read journal_mode");
        let synchronous: i64 = engine
            .connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .expect("read synchronous");
        let application_id: i64 = engine
            .connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .expect("read application_id");

        assert_eq!(foreign_keys, 1);
        assert_eq!(journal_mode, "memory");
        assert_eq!(synchronous, 2);
        assert_eq!(application_id, APPLICATION_ID);
    }

    #[test]
    fn file_connections_enforce_required_pragmas() {
        let directory = tempfile::tempdir().expect("create temporary directory");
        let path = directory.path().join("pragmas.vrac");
        let engine = Engine::open(path).expect("open file database");
        let foreign_keys: i64 = engine
            .connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("read foreign_keys");
        let journal_mode: String = engine
            .connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("read journal_mode");
        let synchronous: i64 = engine
            .connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .expect("read synchronous");

        assert_eq!(foreign_keys, 1);
        assert_eq!(journal_mode, "wal");
        assert_eq!(synchronous, 2);
    }
}
