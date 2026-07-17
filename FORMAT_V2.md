# Vrac workspace format v2

This document defines the canonical Vrac workspace format introduced for node
tags and inline references. Format version 2 is the only supported format.

## File identity

A workspace is an ordinary SQLite database with these header values:

```sql
PRAGMA application_id = 0x56524143; -- `VRAC` in ASCII
PRAGMA user_version = 2;
```

The engine rejects unknown versions and foreign non-zero application
identifiers without rewriting them. Valid unmarked v2 workspaces are marked
only after their complete schema has been validated.

## Canonical schema

[`crates/vrac/schema.sql`](crates/vrac/schema.sql) is the executable
source of truth for new v2 workspaces:

```sql
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
```

`parent_id = NULL` represents a root-level node. The product's single `root`
is a virtual view over those nodes and is not stored. Sibling order remains the
private `(position, id)` order.

## Tags

Tags are unordered node properties. The engine canonicalizes every input by
trimming its ends and applying Rust Unicode lowercase conversion. A canonical
tag is non-empty and contains neither whitespace nor `#`. Canonical tags are
stored without a visual prefix and are returned in bytewise lexical order.

The primary key prevents duplicates on one node. `node_tags_by_tag` supports a
future reverse lookup without adding data to nodes that have no tags. SQLite
cannot enforce the complete Unicode canonicalization rule; `vrac check`
reports values written outside the engine that violate it.

## Inline references

The source node keeps human-readable text such as `Point on [[Project X]]`.
Each `node_references` row associates the UTF-8 byte range of `Project X` with
the stable identifier of its target. `start_byte` is inclusive and `end_byte`
is exclusive; the surrounding `[[` and `]]` are outside the range.

Ranges for one source must be valid UTF-8 boundaries, non-empty,
non-overlapping, and surrounded by the bracket syntax. The engine validates
these rules transactionally and `vrac check` verifies them for externally
modified files. Multiple references to one target, self-references, and cycles
between references are valid because they do not affect tree reachability.

The source's stored label is a readable fallback. Normal reads resolve and
return the target's current plain text, so renaming a target requires no rewrite
of its sources. Resolution is not recursive. Deleting a source would cascade
its outgoing properties; deleting a referenced target is restricted. Node
deletion is not part of the current public API.

The committed [`v2.vrac`](crates/vrac/tests/fixtures/v2.vrac) sample is the
immutable format fixture.

A checkpoint is a complete standalone file in this same format. It contains no
dependency on the active workspace's WAL and can be opened directly as a normal
workspace.

## Integrity and derived data

Tables and indexes in the schema above are canonical. Schema changes require a
new format version; older formats are not supported during pre-production
development. Full-text search is not part of v2; any future search index must
be derived and rebuildable from canonical node text.
