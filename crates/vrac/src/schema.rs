use rusqlite::Connection;

use crate::{Error, Result};

pub(crate) const APPLICATION_ID: i64 = 0x5652_4143;
pub(crate) const CURRENT_SCHEMA_VERSION: i64 = 2;

const SCHEMA_SQL: &str = include_str!("../schema.sql");

#[derive(Clone, Copy)]
struct SchemaObject {
    kind: &'static str,
    name: &'static str,
    table: &'static str,
    statement_index: usize,
}

const SCHEMA_OBJECTS: &[SchemaObject] = &[
    SchemaObject {
        kind: "table",
        name: "node_references",
        table: "node_references",
        statement_index: 4,
    },
    SchemaObject {
        kind: "index",
        name: "node_references_by_target",
        table: "node_references",
        statement_index: 5,
    },
    SchemaObject {
        kind: "table",
        name: "node_search",
        table: "node_search",
        statement_index: 10,
    },
    SchemaObject {
        kind: "trigger",
        name: "node_search_delete",
        table: "nodes",
        statement_index: 12,
    },
    SchemaObject {
        kind: "trigger",
        name: "node_search_insert",
        table: "nodes",
        statement_index: 11,
    },
    SchemaObject {
        kind: "trigger",
        name: "node_search_update",
        table: "nodes",
        statement_index: 13,
    },
    SchemaObject {
        kind: "table",
        name: "node_tags",
        table: "node_tags",
        statement_index: 2,
    },
    SchemaObject {
        kind: "index",
        name: "node_tags_by_tag",
        table: "node_tags",
        statement_index: 3,
    },
    SchemaObject {
        kind: "table",
        name: "nodes",
        table: "nodes",
        statement_index: 0,
    },
    SchemaObject {
        kind: "index",
        name: "nodes_by_parent",
        table: "nodes",
        statement_index: 1,
    },
    SchemaObject {
        kind: "table",
        name: "sync_batch",
        table: "sync_batch",
        statement_index: 9,
    },
    SchemaObject {
        kind: "table",
        name: "sync_devices",
        table: "sync_devices",
        statement_index: 7,
    },
    SchemaObject {
        kind: "table",
        name: "sync_outbox",
        table: "sync_outbox",
        statement_index: 8,
    },
    SchemaObject {
        kind: "table",
        name: "workspace",
        table: "workspace",
        statement_index: 6,
    },
];

pub(crate) fn prepare_database(connection: &mut Connection) -> Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let application_id: i64 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;

    if application_id != 0 && application_id != APPLICATION_ID {
        return Err(Error::InvalidDatabase(format!(
            "application ID 0x{application_id:08x} does not identify a Vrac database"
        )));
    }

    match version {
        0 if application_id == 0 && database_is_empty(connection)? => create_database(connection),
        0 => Err(Error::InvalidDatabase(
            "the database has no schema version but is not an empty, unmarked file".into(),
        )),
        CURRENT_SCHEMA_VERSION => {
            validate_schema(
                connection,
                CURRENT_SCHEMA_VERSION,
                SCHEMA_SQL,
                SCHEMA_OBJECTS,
            )?;
            validate_workspace_identity(connection)?;
            mark_database(connection, application_id)
        }
        version => Err(Error::UnsupportedSchemaVersion(version)),
    }
}

fn create_database(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(SCHEMA_SQL)?;
    transaction.execute(
        "INSERT INTO workspace (singleton, workspace_id) VALUES (1, randomblob(16))",
        [],
    )?;
    crate::journal::create_journal(&transaction)?;
    transaction.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
    transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
    validate_schema(
        &transaction,
        CURRENT_SCHEMA_VERSION,
        SCHEMA_SQL,
        SCHEMA_OBJECTS,
    )?;
    validate_workspace_identity(&transaction)?;
    transaction.commit()?;
    Ok(())
}

fn validate_workspace_identity(connection: &Connection) -> Result<()> {
    let identities: i64 = connection.query_row(
        "SELECT COUNT(*) FROM workspace
         WHERE singleton = 1 AND typeof(workspace_id) = 'blob' AND length(workspace_id) = 16",
        [],
        |row| row.get(0),
    )?;
    if identities != 1 {
        return Err(Error::InvalidDatabase(
            "the workspace identity is missing or invalid".into(),
        ));
    }
    Ok(())
}

fn mark_database(connection: &mut Connection, application_id: i64) -> Result<()> {
    if application_id == APPLICATION_ID {
        return Ok(());
    }
    let transaction = connection.transaction()?;
    transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
    transaction.commit()?;
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

fn validate_schema(
    connection: &Connection,
    version: i64,
    schema_sql: &str,
    expected_objects: &[SchemaObject],
) -> Result<()> {
    let expected_statements: Vec<String> = schema_statements(schema_sql)
        .into_iter()
        .map(normalize_sql)
        .collect();
    if expected_statements.len() != expected_objects.len() {
        return Err(Error::InvalidDatabase(
            "the embedded schema definition is invalid".into(),
        ));
    }

    let mut statement = connection.prepare(
        "SELECT type, name, tbl_name, sql
         FROM sqlite_schema
         WHERE name NOT LIKE 'sqlite_%'
           AND name NOT IN ('node_search_config', 'node_search_data',
                            'node_search_docsize', 'node_search_idx')
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

    if actual.len() != expected_objects.len() {
        return Err(Error::InvalidDatabase(format!(
            "schema version {version} contains {} application objects instead of {}",
            actual.len(),
            expected_objects.len()
        )));
    }

    for (expected, (actual_kind, actual_name, actual_table, actual_sql)) in
        expected_objects.iter().zip(actual)
    {
        if actual_kind != expected.kind
            || actual_name != expected.name
            || actual_table != expected.table
        {
            return Err(Error::InvalidDatabase(format!(
                "schema object {actual_name:?} does not belong to schema version {version}"
            )));
        }
        let expected_sql = &expected_statements[expected.statement_index];
        if actual_sql.as_deref().map(normalize_sql).as_ref() != Some(expected_sql) {
            return Err(Error::InvalidDatabase(format!(
                "schema object {:?} does not match schema version {version}",
                expected.name
            )));
        }
    }

    Ok(())
}

fn normalize_sql(sql: &str) -> String {
    sql.trim_end_matches(';')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn schema_statements(sql: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    let mut in_trigger = false;
    for line in sql.split_inclusive('\n') {
        offset += line.len();
        let trimmed = line.trim();
        if trimmed.starts_with("CREATE TRIGGER") {
            in_trigger = true;
        }
        if (!in_trigger && trimmed.ends_with(';')) || (in_trigger && trimmed == "END;") {
            statements.push(sql[start..offset].trim());
            start = offset;
            in_trigger = false;
        }
    }
    if !sql[start..].trim().is_empty() {
        statements.push(sql[start..].trim());
    }
    statements
}
