# Vrac

Vrac is a local-first outliner designed for fast capture and mental offloading.
The repository currently contains the independent Rust engine and its CLI. The
Tauri/Svelte interface remains outside the critical path until the engine is
stable.

Contribution rules are documented in [`AGENTS.md`](AGENTS.md).

## Layout

```text
crates/vrac       engine library, model, and SQLite storage
crates/vrac-cli   CLI client, producing the `vrac` binary
src-tauri         future desktop client, not connected to the engine yet
```

## Development

```sh
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Build the optimized CLI with:

```sh
cargo build --release -p vrac-cli
```

## CLI

```text
vrac init <file>
vrac add <file> [--parent <id>] [--first|--last|--before <id>|--after <id>] <text>
vrac node <file> <id>
vrac children <file> [--parent <id>] [--limit <n>]
vrac set-text <file> <id> <text>
vrac move <file> <id> [--parent <id>] [--first|--last|--before <id>|--after <id>]
vrac check <file>
vrac generate <file> --nodes <n> [--shape wide|deep|mixed]
```

Example:

```sh
cargo run -p vrac-cli -- init notes.vrac
root_id=$(cargo run -q -p vrac-cli -- add notes.vrac "First idea")
cargo run -q -p vrac-cli -- add notes.vrac --parent "$root_id" "Explore further"
cargo run -q -p vrac-cli -- children notes.vrac --parent "$root_id"
cargo run -q -p vrac-cli -- check notes.vrac
```

Node output contains three tab-separated columns: identifier, parent (`-` for a
root node), and escaped text. Numeric storage positions are not exposed to
clients.

## Database format

A Vrac workspace is an ordinary SQLite file. Format version 1 uses
`PRAGMA application_id = 0x56524143` (`VRAC` in ASCII) and
`PRAGMA user_version = 1`. The engine validates both the marker and the exact
schema before accepting an existing file. Valid unmarked version 1 databases
created before the marker was introduced are marked on their first successful
open.

File-backed workspaces use foreign-key enforcement, WAL journaling, and
`synchronous = FULL`. The canonical schema is
[`crates/vrac/schema.sql`](crates/vrac/schema.sql).
