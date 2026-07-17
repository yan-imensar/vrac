CREATE TABLE nodes (
    id        BLOB PRIMARY KEY NOT NULL
              CHECK (typeof(id) = 'blob' AND length(id) = 16),
    parent_id BLOB REFERENCES nodes(id)
              CHECK (parent_id IS NULL OR
                     (typeof(parent_id) = 'blob' AND length(parent_id) = 16)),
    position  INTEGER NOT NULL,
    text      TEXT NOT NULL
) STRICT;

CREATE INDEX nodes_by_parent
    ON nodes(parent_id, position, id);

CREATE TABLE node_tags (
    node_id BLOB NOT NULL REFERENCES nodes(id) ON DELETE CASCADE
            CHECK (typeof(node_id) = 'blob' AND length(node_id) = 16),
    tag     TEXT NOT NULL CHECK (tag <> ''),
    PRIMARY KEY (node_id, tag)
) STRICT, WITHOUT ROWID;

CREATE INDEX node_tags_by_tag
    ON node_tags(tag, node_id);

CREATE TABLE node_references (
    source_id  BLOB NOT NULL REFERENCES nodes(id) ON DELETE CASCADE
               CHECK (typeof(source_id) = 'blob' AND length(source_id) = 16),
    start_byte INTEGER NOT NULL CHECK (start_byte >= 0),
    end_byte   INTEGER NOT NULL CHECK (end_byte > start_byte),
    target_id  BLOB NOT NULL REFERENCES nodes(id)
               CHECK (typeof(target_id) = 'blob' AND length(target_id) = 16),
    PRIMARY KEY (source_id, start_byte)
) STRICT, WITHOUT ROWID;

CREATE INDEX node_references_by_target
    ON node_references(target_id, source_id, start_byte);

CREATE TABLE workspace (
    singleton    INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    workspace_id BLOB NOT NULL UNIQUE
                 CHECK (typeof(workspace_id) = 'blob' AND length(workspace_id) = 16)
) STRICT;

CREATE TABLE sync_devices (
    device_id       BLOB PRIMARY KEY NOT NULL
                    CHECK (typeof(device_id) = 'blob' AND length(device_id) = 16),
    next_sequence   INTEGER CHECK (next_sequence IS NULL OR next_sequence > 0),
    applied_sequence INTEGER NOT NULL DEFAULT 0 CHECK (applied_sequence >= 0),
    applied_package BLOB CHECK (applied_package IS NULL OR
                                (typeof(applied_package) = 'blob' AND
                                 length(applied_package) = 32))
) STRICT, WITHOUT ROWID;

CREATE TABLE sync_outbox (
    device_id BLOB NOT NULL REFERENCES sync_devices(device_id) ON DELETE CASCADE,
    sequence  INTEGER NOT NULL CHECK (sequence > 0),
    changeset BLOB NOT NULL CHECK (typeof(changeset) = 'blob' AND length(changeset) > 0),
    PRIMARY KEY (device_id, sequence)
) STRICT, WITHOUT ROWID;

CREATE TABLE sync_batch (
    device_id  BLOB PRIMARY KEY NOT NULL
               REFERENCES sync_devices(device_id) ON DELETE CASCADE,
    first_sequence INTEGER NOT NULL CHECK (first_sequence > 0),
    last_sequence  INTEGER NOT NULL CHECK (last_sequence >= first_sequence),
    package_id BLOB NOT NULL UNIQUE
               CHECK (typeof(package_id) = 'blob' AND length(package_id) = 32),
    bytes      BLOB NOT NULL CHECK (typeof(bytes) = 'blob' AND length(bytes) > 0)
) STRICT, WITHOUT ROWID;
