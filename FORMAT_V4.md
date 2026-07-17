# Vrac workspace format v4

Format v4 adds indexed text search without changing canonical product or
synchronization data. It is the current pre-production format.

## File identity

A workspace is an ordinary SQLite database with:

```sql
PRAGMA application_id = 0x56524143; -- `VRAC`
PRAGMA user_version = 4;
```

[`crates/vrac/schema.sql`](crates/vrac/schema.sql) is the executable schema.
The engine validates the complete application schema and workspace identity
before accepting a file.

## Canonical data

The canonical tables and their v3 semantics are unchanged:

- `nodes(id, parent_id, position, text)`;
- `node_tags(node_id, tag)`;
- `node_references(source_id, start_byte, end_byte, target_id)`;
- `workspace`, `sync_devices`, `sync_outbox`, and `sync_batch`.

Only `nodes`, `node_tags`, and `node_references` are exchanged in SQLite
changesets. Search data is derived locally and is never included in sync
packages.

Deleting a node removes its complete subtree and its outgoing tags and
references. SQLite still restricts deletion of a referenced target. The engine
adds the product rule that references contained inside the same deleted
subtree are allowed, while a reference from outside the subtree rejects the
whole deletion transaction.

## Search index

`node_search` is an FTS5 external-content table over `nodes.text`. It uses the
Unicode tokenizer with diacritic removal and native two- and three-character
prefix indexes. One-character terms return no result, avoiding an unhelpful
workspace-wide prefix while a person is still typing. Three triggers mirror
node inserts,
text updates, and deletions into the index. Search joins FTS row identities
back to `nodes`, stops at a bounded result, and then resolves normal node
metadata through the same grouped reads as other APIs.

The FTS5 shadow tables are SQLite implementation data. They are not canonical,
not synchronized, and may be rebuilt from `nodes` without losing product data.

## Development format policy

New workspaces are created directly as v4. The pre-production v2 and v3
fixtures remain immutable and unsupported; no migration code is retained while
there is no production workspace to migrate.

All synchronization, package, checkpoint, WAL, foreign-key, and durability
rules defined by [`FORMAT_V3.md`](FORMAT_V3.md) remain unchanged.
