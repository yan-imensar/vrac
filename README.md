# Vrac

> Capture first. Structure later—or never.

Vrac is the local-first TUI outliner your terminal has been missing. It opens
on today, stays out of the way, and remains stupidly fast—even across millions
of notes.

Your thoughts do not wait for the right folder. Vrac does not ask for one.
Write everything in the Journal, let structure emerge through bullets, add
`[[references]]` and `#tags` when they are useful, and find the important stuff
later without spending your life maintaining a second brain.

Vrac is early software and is already used for daily note-taking. Its workspace
format starts cleanly at V1.

## Not another productivity system

- Open it and type. You are already in today's Journal.
- Structure naturally with real, inline-editable bullets.
- Keep tasks, decisions, meetings, and ideas where they actually happened.
- Connect concepts with stable `[[references]]` and contextual backlinks.
- Add indexed `#tags` without turning every sentence into metadata soup.
- Work locally or through SSH with no graphical dependency.
- Synchronize through an ordinary iCloud, Syncthing, Dropbox, or OneDrive
  folder while the active SQLite database stays on local storage.

No account. No server. No telemetry. No cloud database. No loading spinner
contemplating its own existence.

Just one fast terminal outliner, an ordinary SQLite database, and your thoughts.

## Install

On macOS or x86_64 Linux, install the latest release without Rust or `sudo`:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/yan-imensar/vrac/releases/latest/download/vrac-installer.sh | sh
```

The installer verifies the release SHA-256 checksum and writes `vrac` to
`~/.local/bin`. Set `VRAC_INSTALL_DIR` to choose another directory, or
`VRAC_VERSION=v0.1.0` to install a specific release. It never edits shell
configuration; if necessary, it prints the directory to add to `PATH`.

For a manual installation, download the archive for your platform from the
[latest GitHub release](https://github.com/yan-imensar/vrac/releases/latest):

| Platform | Archive |
| --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin.tar.gz` |
| macOS Intel | `x86_64-apple-darwin.tar.gz` |
| Linux x86_64 | `x86_64-unknown-linux-musl.tar.gz` |
| Windows x86_64 | `x86_64-pc-windows-msvc.zip` |

Each archive has a matching SHA-256 checksum. Extract it, place `vrac` (or
`vrac.exe`) somewhere on your `PATH`, and run:

```sh
vrac
```

Developers with Rust 1.95 or newer can install the current main branch
directly:

```sh
cargo install --locked --git https://github.com/yan-imensar/vrac vrac
```

From a source checkout:

```sh
cargo install --locked --path crates/vrac
vrac
```

The first launch opens a terminal-native folder browser. Choose or create a
folder for the workspace; the same flow works over SSH. Later launches reuse
that selection. If the selected workspace can no longer be opened, Vrac shows
the browser again and leaves its existing local and shared files untouched.
Use `vrac --workspace <provider-folder>` to open an explicit provider folder,
or `vrac workspace select` to reopen the browser.

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

## Local means local

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

Terminal presentation preferences live in
`$XDG_CONFIG_HOME/vrac/config.toml`, or `~/.config/vrac/config.toml` when
`XDG_CONFIG_HOME` is unset. Windows uses `%APPDATA%\vrac\config.toml`.
Vrac creates this file only after a setting changes. `:lines on` and
`:lines off` show or hide hierarchy guides immediately and remember the choice:

```toml
lines = true
```

Independent changes merge; real conflicts stop atomically instead of silently
using last-writer-wins.

## Command line

The product is terminal-first. It opens the remembered workspace by default,
can open an explicit provider folder, or can ask for another one:

```text
vrac
vrac --workspace <provider-folder>
vrac workspace select
```

Bounded commands that operate directly on one local SQLite file are available
under the explicit `db` namespace:

```text
vrac db init <file>
vrac db add <file> [--parent <id>] [--first|--last|--before <id>|--after <id>] <text>
vrac db node <file> <id>
vrac db children <file> [--parent <id>] [--limit <n>|--all]
vrac db set-text <file> <id> <text>
vrac db move <file> <id> [--parent <id>] [--first|--last|--before <id>|--after <id>]
vrac db check <file>
```

These are database maintenance and scripting commands, not the primary
workspace interface. Run `vrac db --help` for the same command summary.

## Repository layout

```text
crates/vrac-engine      model, business rules, SQLite, and sync protocol
crates/vrac-workspace   local DB lifecycle, provider layout, and sync rounds
crates/vrac-tui         library terminal frontend, transient state, and rendering
crates/vrac             public `vrac` binary and scriptable command routing
```

The engine is synchronous and independent from every interface. Workspace
orchestration is reusable by terminal and future frontends. The CLI and TUI
contain no SQL, provider-format knowledge, or duplicated business rules. The
`vrac` package provides the only product executable and supplies launch context
to the terminal frontend.

The current workspace format is documented in [FORMAT.md](FORMAT.md). See
[CONTRIBUTING.md](CONTRIBUTING.md) for the branch, pull-request, and release
workflow.

## Development

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

"Fast" is not decorative copy. The engine and TUI have reproducible
five-million-node release scenarios, intentionally separate from ordinary
correctness tests. The engine scenario creates its own performance database;
data generation is deliberately not part of the installed command surface:

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

Vrac is free software licensed under the
[GNU Affero General Public License v3.0](LICENSE). You may use, modify, share,
and sell it. If you distribute a modified version or make one available over
a network, its users must be offered the corresponding source code under the
same license.
