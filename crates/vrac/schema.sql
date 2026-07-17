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
