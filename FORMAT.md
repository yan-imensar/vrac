# Vrac workspace format

This document defines the only supported pre-production workspace format.
There are no historical production formats and no migration code.

## File identity

A workspace is an ordinary SQLite database with:

```sql
PRAGMA application_id = 0x56524143; -- `VRAC`
PRAGMA user_version = 1;
```

[`crates/vrac/schema.sql`](crates/vrac/schema.sql) is the executable schema.
The engine validates the complete application schema and the single workspace
identity before accepting a file. A valid unmarked database is marked only
after that validation succeeds.

## Product data

The canonical product tables are:

- `nodes(id, parent_id, position, text)`;
- `node_tags(node_id, tag)`;
- `node_references(source_id, start_byte, end_byte, target_id)`.

Node identifiers are opaque 16-byte values. `parent_id = NULL` represents a
root-level node; the product root is virtual and is not stored. Siblings are
ordered deterministically by `(position, id)`, while numeric positions remain
private storage details. Normal node reads also return whether a node currently
has children. This value is derived through the parent index and is not stored
or synchronized.

Tags are unordered properties stored without a visual `#`. The engine trims
them, converts them to Unicode lowercase, rejects empty values, whitespace and
`#`, removes duplicates, and returns them in lexical order.

Reference ranges cover only the UTF-8 label between `[[` and `]]`. They are
half-open, non-empty, on character boundaries, and non-overlapping within one
source. Multiple references, self-references, and reference cycles are valid.
Reads resolve the target's current plain text without recursively resolving
references in that text. Deleting a source cascades its properties; deleting a
target referenced from outside the deleted subtree is rejected atomically.

## Synchronization state

The canonical synchronization tables are:

- `workspace(singleton, workspace_id)`;
- `sync_devices(device_id, next_sequence, applied_sequence, applied_package)`;
- `sync_outbox(device_id, sequence, changeset)`;
- `sync_batch(device_id, first_sequence, last_sequence, package_id, bytes)`.

`workspace_id` prevents packages from being applied to another workspace.
Every synchronized product mutation writes its SQLite changeset to
`sync_outbox` in the same transaction. One prepared immutable package per
device is retained in `sync_batch` until the client confirms publication.
Confirmation removes the covered outbox rows and advances the applied
sequence atomically. These rows are a bounded delivery queue, not permanent
history.

Only `nodes`, `node_tags`, and `node_references` are exchanged in changesets.
Independent changes merge. A row conflict, tree cycle, invalid reference, or
other integrity failure aborts the complete incoming package. A missing causal
dependency is reported separately so the caller can apply other packages and
retry.

## Package encoding

A `.vrac-sync` package uses this fixed binary layout:

| Bytes | Value |
| ---: | --- |
| 8 | ASCII `VRACSYNC` |
| 1 | package encoding version (`1`) |
| 16 | workspace ID |
| 16 | source device ID |
| 8 | first sequence, unsigned big-endian |
| 8 | last sequence, unsigned big-endian |
| 8 | payload length, unsigned big-endian |
| variable | SQLite changeset payload |
| 32 | BLAKE3 hash of every preceding byte |

The stable filename is
`<device>-<first:020>-<last:020>.vrac-sync`. Packages are immutable and opaque
to provider clients.

## Derived search data

`node_search` is an FTS5 external-content table over `nodes.text`. It uses the
Unicode tokenizer with diacritic removal and native two- and three-character
prefix indexes. One-character terms return no result. SQLite triggers mirror
node inserts, text updates, and deletions into the index.

The FTS table and its shadow tables are local derived data. They are excluded
from synchronization and may be rebuilt from canonical node text.

## Durability and checkpoints

File-backed workspaces enable foreign keys, WAL journaling, and
`synchronous = FULL`. Every business mutation is an explicit transaction.

A checkpoint is a complete standalone workspace produced through SQLite's
online backup API. It contains canonical state and synchronization frontiers,
but no pending outbox or prepared batch from the source installation. The
engine validates schema and integrity before publishing it and never copies an
open database or its WAL directly.

## Integrity

`Engine::check` verifies SQLite integrity, foreign keys, tree reachability,
canonical tags, reference ranges, synchronization frontiers, queued
changesets, and prepared packages. Normal reads are bounded and never traverse
the complete tree.

## Pre-production evolution

Until a workspace format ships to users, a schema change replaces this format,
resets development data and updates this document, the executable schema, and
the single current fixture together. Historical development fixtures and
migrations are not retained.

Once a format is released, it becomes immutable. Every later schema change
will increment `user_version`, preserve a fixture for the released version,
and provide an explicit tested migration.
