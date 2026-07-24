# Vrac

Vrac is a local-first terminal outliner built for fast capture and mental
offloading. Write first, structure naturally, and retrieve notes later through
their context, tags, references, and backlinks.

The project is approaching its first public release and is already used for
daily note-taking. The documented workspace format starts cleanly at V1.

## Why Vrac

- Capture directly into today's Journal without choosing a destination first.
- Edit a real outline inline with keyboard-first navigation.
- Keep tasks, decisions, meetings, and ideas in their original context.
- Connect concepts with stable `[[references]]` and retrieve their backlinks.
- Classify nodes with indexed `#tags` without mixing metadata into their text.
- Work locally or through SSH with no graphical dependency.
- Synchronize through an ordinary iCloud, Syncthing, Dropbox, or OneDrive
  folder while the active SQLite database stays on local storage.

Vrac has no account, server, telemetry, or cloud database. SQLite is the single
source of truth on each device.

## Install

Vrac currently requires Rust 1.95 or newer. From a source checkout:

```sh
cargo install --path crates/vrac
vrac
```

The first launch opens a terminal-native folder browser. Choose or create a
folder for the workspace; the same flow works over SSH. Later launches reuse
that selection. If the selected workspace can no longer be opened, Vrac shows
the browser again and leaves its existing local and shared files untouched.

To build without installing:

```sh
cargo run --release -p vrac
```

## Terminal workflow

Vrac opens today's Journal and persists non-empty edits automatically. There is
no save command.

### Navigation

| Key | Action |
| --- | --- |
| `j` / `k`, arrows | Move between bullets |
| `h` / `l` | Parent or first child inside the current zoom |
| `H` | Return to the parent zoom |
| `Enter` | Focus the selected bullet |
| `Space` | Expand or collapse |
| `gg` / `G` | First or last visible bullet |
| `/` | Search nodes |
| `:` | Open commands |
| `b` | Open contextual backlinks |
| `#` | Toggle tags in normal mode |
| `?` | Open keyboard help |

### Editing

| Key | Action |
| --- | --- |
| `i` / `a` | Edit at the start or end |
| `o` / `O` | Create a sibling after or before |
| `c` | Create a child |
| `Enter` | Persist and continue with a sibling |
| `Tab` / `Shift-Tab` | Indent or outdent |
| `Esc` | Persist non-empty work and return to normal mode |
| `[[` | Complete a stable reference inline |
| `#` | Complete a tag inline |
| `Ctrl-Enter` | Persist and focus the edited bullet |

Up and down move through wrapped visual lines before crossing into adjacent
bullets. Home and End stay on the current visual line. Alt-arrow moves by word,
and Alt-Backspace removes a word.

### Structure and history

| Key | Action |
| --- | --- |
| `yy` | Copy a subtree as portable indented text |
| `dd` | Copy, then delete a subtree |
| `p` | Paste an indented outline |
| `u` / `Ctrl-R` | Undo or redo |

Clipboard text remains useful outside Vrac. Paste accepts ordinary lines or
indented `- ` bullets and creates the complete hierarchy atomically.

## Workspaces and synchronization

The selected folder contains only portable synchronization material:

```text
workspace-id
checkpoint.vrac
changes/*.vrac-sync
```

The open SQLite database and its WAL remain in the platform's local application
data. Vrac never opens a live database directly from a synchronized or network
folder.

Synchronization runs at startup and after idle periods outside inline editing.
Use `:sync` for an immediate round and `:workspace` to choose another folder.
Independent changes merge; real conflicts stop atomically instead of silently
using last-writer-wins.

## Command line

The same `vrac` binary keeps bounded, scriptable engine commands:

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

Launching `vrac` without arguments opens the TUI. `vrac tui [workspace-folder]`
opens a specific workspace, and `vrac workspace` opens the folder selector.

## Repository layout

```text
crates/vrac-engine   model, business rules, SQLite, sync, and checkpoints
crates/vrac          public `vrac` binary and scriptable commands
crates/vrac-tui      terminal interaction and rendering
```

The engine is synchronous and independent from every interface. The CLI and
TUI contain no SQL or duplicated business rules.

The current workspace format is documented in [FORMAT.md](FORMAT.md), and
contribution rules are in [AGENTS.md](AGENTS.md).

## Development

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The engine and TUI have reproducible five-million-node release scenarios. They
are intentionally separate from ordinary correctness tests:

```sh
cargo run --release -p vrac --example performance -- \
  /local/path/vrac-5m-wide.vrac --shape wide

cargo run --release -p vrac-tui --example tui_performance -- \
  /local/path/to/workspace-or-checkpoint
```

The engine budget is 2 ms at p95 for interactive operations. The terminal
scenario requires first-view model creation below 1.5 seconds, interactive work
including frame serialization below 16.667 ms at p95, and total peak RSS below
64 MiB on the reference machine.

## License

Vrac is available under the [MIT License](LICENSE).
