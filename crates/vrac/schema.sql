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
