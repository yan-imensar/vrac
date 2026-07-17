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
vrac add <file> [--parent <id>] <text>
vrac node <file> <id>
vrac children <file> [--parent <id>] [--limit <n>]
vrac set-text <file> <id> <text>
vrac move <file> <id> [--parent <id>] [--position <n>]
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

Node output contains four tab-separated columns: identifier, parent (`-` for a
root node), position, and escaped text.
