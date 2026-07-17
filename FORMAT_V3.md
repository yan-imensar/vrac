# Vrac workspace format v3

Format v3 adds the minimum durable state required to exchange changes between
one person's devices. It keeps the v2 node, tag, and reference model unchanged.
Format v3 is the only format accepted by the current pre-production engine.

## File identity

A workspace is an ordinary SQLite database with:

```sql
PRAGMA application_id = 0x56524143; -- `VRAC`
PRAGMA user_version = 3;
```

[`crates/vrac/schema.sql`](crates/vrac/schema.sql) is the executable schema.
The engine validates the complete schema and the single workspace identity
before accepting a file.

## Product data

The canonical product tables remain:

- `nodes(id, parent_id, position, text)`;
- `node_tags(node_id, tag)`;
- `node_references(source_id, start_byte, end_byte, target_id)`.

Their v2 semantics and indexes are unchanged. `parent_id = NULL` represents a
root-level node. Tags are canonical unordered properties. Reference byte ranges
cover only the UTF-8 label between `[[` and `]]` and resolve their target text
when read.

## Synchronization state

The additional tables are deliberately small and direct:

```sql
CREATE TABLE workspace (
    singleton    INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    workspace_id BLOB NOT NULL UNIQUE
                 CHECK (typeof(workspace_id) = 'blob' AND length(workspace_id) = 16)
) STRICT;

CREATE TABLE sync_devices (
    device_id        BLOB PRIMARY KEY NOT NULL
                     CHECK (typeof(device_id) = 'blob' AND length(device_id) = 16),
    next_sequence    INTEGER CHECK (next_sequence IS NULL OR next_sequence > 0),
    applied_sequence INTEGER NOT NULL DEFAULT 0 CHECK (applied_sequence >= 0),
    applied_package  BLOB CHECK (applied_package IS NULL OR
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
```

`workspace_id` prevents packages from being applied to another workspace.
`sync_devices` stores one monotonically increasing sequence per source device
and the latest sequence represented locally. `next_sequence = NULL` means the
device is known only as a remote source.

Every product mutation captured in synchronized mode inserts one SQLite
changeset into `sync_outbox` in the same transaction. A mutation cannot commit
without its outbox row. A fresh workspace opened without a device identity is
unsynchronized. Once a local device is active, ordinary reopening resumes that
identity so capture cannot be disabled accidentally. Explicitly requesting a
different local identity is rejected.

All outbox rows waiting when synchronization runs are combined into one
immutable provider package. Changes committed after its preparation form the
next package. `sync_batch` retains exactly the prepared package until the
client confirms its durable publication. A retry therefore returns identical
bytes and the same filename. Confirmation atomically removes the covered
outbox rows and advances the source device's applied sequence.

These transport rows are bounded by normal synchronization and are not a
permanent event history.

## Package format

A `.vrac-sync` package uses this fixed binary layout:

| Bytes | Value |
| ---: | --- |
| 8 | ASCII `VRACSYNC` |
| 1 | package format version (`1`) |
| 16 | workspace ID |
| 16 | source device ID |
| 8 | first sequence, unsigned big-endian |
| 8 | last sequence, unsigned big-endian |
| 8 | payload length, unsigned big-endian |
| variable | SQLite changeset payload |
| 32 | BLAKE3 hash of every preceding byte |

The stable filename is
`<device>-<first:020>-<last:020>.vrac-sync`. Provider files are immutable.
Package bytes are opaque to clients; filesystem paths, Apple document
providers, OneDrive, Android Storage Access Framework, and similar adapters
only list, read, and publish them.

Packages are applied in sequence order within each device stream. Applying an
already represented package is a no-op. Independent row changes merge. A real
SQLite changeset conflict aborts the complete package. The engine also rejects
a merge that would create a tree cycle or invalid overlapping references, even
when both devices' local transactions were independently valid. No last-writer
wins rule silently discards content.

A package may causally depend on a change received earlier from another device.
If that dependency is absent, the engine leaves the package unapplied and
reports a missing dependency rather than a content conflict. A provider adapter
continues with other packages and retries deferred packages after making
progress. Three-device tests cover this out-of-order arrival.

There is no CRDT, server account, transport abstraction, collaboration model,
or unbounded operation log in v3.

## Checkpoints

A checkpoint contains the complete product state and applied sequence for each
known device. Before copying a synchronized workspace, the engine fixes the
current package boundary. In the checkpoint copy, pending outbox and prepared
batch rows are removed and their sequences are marked as represented by the
snapshot. Local sequence allocation is detached from every previous
installation. The active workspace retains its queue so existing devices can
still receive those packages.

A new device verifies and opens the checkpoint, supplies a newly generated
local device ID, then applies only packages beyond the checkpoint frontiers.

## Development version policy

The immutable v2 fixture and documentation remain in the repository. No v2 to
v3 migration is shipped because the product has no production workspace yet;
the current engine rejects older formats instead of carrying unused migration
code.

The committed [`v3.vrac`](crates/vrac/tests/fixtures/v3.vrac) sample is the
immutable v3 format fixture.
