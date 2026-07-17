# Vrac workspace format v1

This document defines the canonical Vrac workspace format shipped with engine
version 0.1. Format version 1 is frozen. Future schema changes must use a new
`PRAGMA user_version` and an explicit migration; this document and the v1 test
fixture remain unchanged as historical references.

## File identity

A workspace is an ordinary SQLite database with these header values:

```sql
PRAGMA application_id = 0x56524143; -- `VRAC` in ASCII
PRAGMA user_version = 1;
```

The engine rejects non-zero application identifiers belonging to another
application and unsupported schema versions without modifying them. During the
pre-0.1 development period, Vrac created valid v1 databases without an
application identifier. Such a file is marked on first open only after its
complete application schema matches v1 exactly.

## Canonical schema

[`crates/vrac/schema.sql`](crates/vrac/schema.sql) is the executable source of
truth for new v1 workspaces:

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
```

`id` is the stable business identity. It is independent from SQLite `rowid` and
is encoded as 16 opaque bytes. `parent_id = NULL` represents a root node.
Sibling order is deterministic by `(position, id)`. `position` is private
storage state and is never part of the public node model.

The table and index above are canonical data structures. A v1 workspace has no
derived index or cache. Future derived structures must be rebuildable from the
canonical tables.

## Connection guarantees

Every engine connection enables and verifies:

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL; -- `memory` for an in-memory workspace
PRAGMA synchronous = FULL;
```

Every business mutation uses an explicit transaction. The active database must
remain on a local disk and must not live directly in a synchronized directory
or on a network filesystem.

## Migration policy

- Published schema versions and their fixtures are immutable.
- Every schema change increments `user_version`.
- A migration validates its source version and runs transactionally.
- A potentially destructive migration requires a recoverable copy and an
  integrity check before replacing the active workspace.
- Unknown versions and foreign application identifiers are never rewritten.
- Migration code is added only when a second schema version exists; v1 does not
  require a migration framework.

## Checkpoints and recovery

Future checkpoints will be complete, immutable SQLite databases that can be
opened directly after validation. They will be created with SQLite's backup
API, never by copying an active database and its WAL files. Replaying the full
history is not part of workspace restoration.
