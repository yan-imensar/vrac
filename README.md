# Vrac

Vrac is a local-first outliner designed for fast capture and mental offloading.
The repository contains the v0.1 Rust engine, its CLI, and the first minimal
Tauri/Svelte product slice.

Contribution rules are documented in [`AGENTS.md`](AGENTS.md).
The current SQLite workspace format is documented in [`FORMAT.md`](FORMAT.md).

## Layout

```text
crates/vrac       engine library, model, and SQLite storage
crates/vrac-cli   CLI client, producing the `vrac` binary
src               minimal Svelte outliner
src-tauri         thin Tauri client of the engine
```

## Development

```sh
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm check
pnpm build
(cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings)
```

Build the optimized CLI with:

```sh
cargo build --release -p vrac-cli
```

## Performance scenario

The reproducible performance scenario defaults to five million nodes and must
run in release mode against a new file on a local, non-synchronized disk:

```sh
cargo run --release -p vrac-cli --example performance -- \
  /local/path/vrac-5m-wide.vrac --shape wide
```

Use a different new file with `--shape deep` or `--shape mixed` to exercise the
other tree shapes. `--nodes` can reduce the dataset for a smoke run. The
scenario refuses to overwrite an existing path and reports tab-separated
timings for generation, reopening, root pagination, synchronized mutations,
package preparation, and integrity checking. Interactive measurements use 100
samples and fail the scenario when their p95 exceeds the 2 ms reference-machine
engine budget. Platform-specific baselines will be measured on representative
devices before release.
The generated workspace includes a metadata-rich page with multiple tags and
references plus a deep path. Large performance scenarios are intentionally
separate from ordinary correctness tests. The scenario also creates and
validates a companion
`*.checkpoint.vrac` file without applying the interactive latency budget to
that background operation.

The same generated workspace drives a bounded-memory regression scenario. It
repeats the frontend's root page, metadata-rich page, indexed search, and deep
path reads without retaining their results:

```sh
cargo run --release -p vrac-cli --example memory -- \
  /local/path/vrac-5m-wide.vrac
```

On Unix systems, including macOS, iOS, Linux, and Android, the scenario reads
the process peak resident set from the operating system and enforces a 32 MiB
growth budget. This is an engine budget, not the total Tauri/WebView process
budget. Full integrity checks remain separate because they traverse the entire
workspace; they must not run on the UI thread. Actual packaged applications
will receive device-level memory baselines before mobile release.

## Checkpoints

`Engine::checkpoint(destination)` creates a complete SQLite snapshot through
SQLite's online backup API. It validates the schema and all integrity rules
before publishing the destination, never copies an open database or its WAL,
and never overwrites an existing file. The resulting file is an ordinary
workspace that can be opened directly.

Checkpoint creation is synchronous and proportional to the complete workspace
size. A client must schedule it outside its interactive execution path. The
engine deliberately provides no scheduler, retention policy, or destructive
restore operation yet.

## Personal synchronization

`Engine::open_synced` captures each product mutation in the same SQLite
transaction as the data. The engine groups pending changes into small,
immutable, checksummed `.vrac-sync` packages. Clients only publish and read the
opaque bytes using the platform's provider: iCloud documents on Apple systems,
OneDrive or a selected folder on Windows, a selected synchronized folder on
Linux, and the Storage Access Framework on Android.

Packages are idempotent and ordered per device. Independent changes merge;
true conflicts abort atomically and remain available instead of silently using
last-writer-wins. A package that causally depends on another device is reported
separately so the provider adapter can apply other packages and retry it.
Reopening an active synchronized workspace with `Engine::open` resumes its
existing capture identity; supplying a different identity is rejected. This
prevents an ordinary application restart from silently producing
unsynchronized edits. There is no server, account system, CRDT, or permanent
event history.

The current graphical client implements the first provider adapter for iCloud
Drive. The open SQLite database always remains in local application data.
Enabling iCloud publishes a validated bootstrap checkpoint and immutable
packages to the app's ubiquity container; joining streams that checkpoint to a
new local file and runs a complete integrity check before opening it. Sync runs
off the UI thread after local edits, when the window regains focus, or on
request. Apple builds require the `iCloud.com.yanimensar.vrac` container and
iCloud Documents capability to be enabled for the signed App ID; otherwise the
workspace panel reports iCloud as unavailable.

## Graphical client boundary

The engine is synchronous and can be moved to the graphical client's dedicated
worker thread, keeping SQLite work off the UI thread without an async runtime in
the library. Nodes and identifiers remain engine types; a thin Tauri adapter
only converts command inputs and outputs. `Cursor` implements text conversion
as an opaque token that can cross IPC unchanged and be parsed for the next page.
The token is temporary continuation state, not workspace data.

The interface implements the dark outline surface, lazy branch expansion,
text editing, focused zoom, bounded pagination, node search, deletion, and
indentation. A central, optional Vim controller drives normal and insert modes;
the bottom status control toggles it without changing engine behavior. `:` and
`/` expand the same bottom area for commands and indexed node search. Backlinks,
undo history, virtual lists, mobile controls, and maintenance commands remain
outside this slice.

Native `View` menu items control webview display zoom. `File > Workspaces…` and
`:workspace` open the same small chooser for local workspace creation,
selection, and iCloud setup. `Default` preserves the original
`workspace.vrac`; additional workspaces are separate local databases.

Tauri owns one synchronous engine on one dedicated standard thread. Thin
commands adapt children, creation, text updates, paths, moves, deletion, and
search; they contain no SQL or product rules. Switching workspaces opens the
new database on that thread, while checkpoint and provider work runs outside
the UI thread.

## CLI

```text
vrac init <file>
vrac add <file> [--parent <id>] [--first|--last|--before <id>|--after <id>] <text>
vrac node <file> <id>
vrac children <file> [--parent <id>] [--limit <n>|--all]
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
clients. `children` reports when its bounded result has more entries;
`children --all` traverses every page internally without exposing cursors.

## Errors and exit codes

The engine returns typed errors and never prints or terminates the process.
Public error categories cover SQLite failures, invalid or unsupported
workspaces, missing nodes or parents, invalid relative placement, cycles,
pagination limits, invalid tags or references, and performance-data generation
limits. Synchronization errors distinguish malformed, foreign, out-of-order,
causally blocked, and conflicting packages. Checkpoint errors distinguish an
existing destination, failed integrity validation, and filesystem failures.

The CLI writes node data and successful command results to standard output and
diagnostics to standard error. Its exit codes are:

| Code | Meaning |
|---:|---|
| `0` | Command succeeded; `check` found no issue. |
| `1` | Engine, storage, or workspace operation failed. |
| `2` | Command syntax, option, identifier, or other CLI input is invalid. |
| `3` | `check` completed and reported integrity issues. |

## Database format

A Vrac workspace is an ordinary SQLite file. The current pre-production format
uses `PRAGMA application_id = 0x56524143` (`VRAC` in ASCII) and
`PRAGMA user_version = 1`. The engine validates both the marker and the exact
schema before accepting an existing file. Valid unmarked current databases are
marked only after validation.

Nodes may carry multiple canonical tags outside their text and stable inline
references whose displayed target text follows target renames. Root-level
nodes remain children of a virtual, unstored product root. The public engine
also provides a root-to-node path read for focused navigation.

File-backed workspaces use foreign-key enforcement, WAL journaling, and
`synchronous = FULL`. The current canonical schema is
[`crates/vrac/schema.sql`](crates/vrac/schema.sql); its compatibility rules are
defined in [`FORMAT.md`](FORMAT.md).
