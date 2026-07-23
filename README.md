# Vrac

Vrac is a local-first outliner designed for fast capture and mental offloading.
The repository contains the v0.1 Rust engine, its CLI, and the first minimal
Tauri/Svelte product slice.

Contribution rules are documented in [`AGENTS.md`](AGENTS.md).
The current SQLite workspace format is documented in [`FORMAT.md`](FORMAT.md).

## Layout

```text
crates/vrac       engine library, model, and SQLite storage
crates/vrac-cli   public `vrac` entrypoint and scriptable commands
crates/vrac-tui   keyboard-first terminal client
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

## Terminal client

The terminal outliner uses the same local-first workspace model as the desktop
application and starts on today's Journal page. Install the public entrypoint
and launch it without arguments:

```sh
cargo install --path crates/vrac-cli
vrac
```

On first launch an in-terminal folder browser asks for a workspace folder, so
it works locally and through SSH without a native dialog. Quick locations point
to detected iCloud Drive, Syncthing, Dropbox, and OneDrive folders; any local
directory remains selectable, and a folder can be created in place. This is the
visible folder that contains only `workspace-id`, `checkpoint.vrac`, and
immutable files under `changes/`. The active SQLite database remains private in
the platform's local application data and is never opened from the synchronized
folder. The selected folder is remembered for later launches. `:workspace` or
`vrac workspace` opens the selector again. Running
`vrac tui /path/to/another/folder` creates or opens that workspace and makes it
the new default. The separately installable `vrac-tui` binary remains a
compatibility alias. If the configured folder is unavailable, Vrac stops
instead of silently opening or recreating a detached copy.

Synchronization runs on startup and after idle periods outside inline editing.
`:sync` requests an immediate round. Closing remains instant: unpublished work
is already durable in the local outbox and is sent on the next round. A folder
already containing a Vrac workspace installs a validated private local copy;
an empty folder creates a new workspace. The previous implicit local database,
when present, is attached to the first selected folder without deleting the
original file.

Use `j`/`k` or the arrow keys to move, `h`/`l` to reach a parent or first
child without leaving the current zoom, `Space` to fold a branch, `Enter` to
focus a node, and `H` to return to the parent zoom.
`/` opens bounded node search and `:` opens the command menu; `#` toggles tags;
`i`, `o`, and `c` edit or create nodes directly in the outline. The normal
footer stays quiet; `?` opens the complete keyboard help and `?` or `Esc`
closes it.
While editing, `Enter` persists the current text and immediately starts the
next sibling, while `Tab` and `Shift-Tab` indent and outdent without leaving
the inline editor. `Esc` returns to navigation with non-empty changes already
persisted, so there is no separate save action. Up and down follow wrapped
visual lines, Home and End stay on the current visual line, Alt-arrow moves by
word, Alt-Backspace removes a word, and Ctrl-Enter persists and zooms into the
edited bullet. Terminal paste is inserted once and line breaks are flattened
inside the bullet. Ctrl-C also persists active work before quitting. `u` and
`Ctrl-R` undo and redo.
`yy`, `dd`, and `p` use the system clipboard and the engine's portable subtree
format. `dd` copies successfully before deleting. Holding a movement key
repeats it, and reaching the end of a loaded sibling page fetches the next page
automatically. Stable references survive ordinary text edits; edited complete
`[[labels]]` are resolved again by the engine, and loaded references immediately
follow target renames. `b` lists contextual backlink paths and opens their
matching nodes. While editing any existing or new bullet,
`[[` opens stable-reference completion and `#` opens tag completion. Enter or
Tab accepts a completion; typed or pasted closing brackets return directly to
the outline editor. Tags selected on a draft are created atomically with its
text. Synchronized terminal updates prevent intermediate redraws from
flashing. Tree guides show only continuing ancestor branches; structural
prefixes, references, and tags have distinct visual treatments. Inline
completion stays beside the outline instead of replacing it.

The same `vrac` binary keeps its non-interactive engine commands (`init`, `add`,
`node`, `children`, `set-text`, `move`, and `check`). They remain suitable for
scripts and future editor integrations; launching the TUI does not change their
arguments, output, or exit-code contract.

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
references, contextual backlink pages, their relevant tag counts, plus a deep
path. Large performance scenarios are intentionally separate from
ordinary correctness tests. The scenario also creates and validates a companion
`*.checkpoint.vrac` file without applying the interactive latency budget to
that background operation.

The same generated workspace drives a bounded-memory regression scenario. It
repeats the frontend's root page, metadata-rich page, indexed node and tag
completion, Journal-day read, and deep path reads without retaining results:

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

## Packaged product baseline

The complete release app has its own reference-Mac gate in addition to the
engine scenarios. The current macOS bundle is 7,124 KiB. The quick-capture
runtime pass reached the first usable view in 0.68 seconds and used about
81 MiB of physical footprint across Vrac and its WebKit helpers. Lazily creating
and then hiding the capture WebView leaves about 101 MiB at idle. Ordinary
startup does not pay for capture. Combined idle CPU was 0.3%. The latest complete
main-outline stress pass containing 100 creates, 100 persisted edits, and 20
complete 100-row reloads used about 199 MiB and remained below the next 60 Hz
paint at p95 for every measured interaction.

The local gates are 8 MiB for the bundle, 1.5 seconds for startup, 96 MiB for
the initial view, 224 MiB after the stress pass, and 1% idle CPU. These are
regression limits for the reference Mac, not estimates for iOS, Android,
Windows, or Linux. Each packaged target requires its own baseline before
release; shared-memory-aware physical footprint is used on macOS because
summed process RSS overstates WebKit usage.

## Checkpoints

`Engine::checkpoint(destination)` creates a complete SQLite snapshot through
SQLite's online backup API. It validates the schema and all integrity rules
before publishing the destination, including consistency between canonical
node text and its derived full-text index. It never copies an open database or
its WAL and never overwrites an existing file. The resulting file is an
ordinary workspace that can be opened directly.

`Engine::restore_checkpoint(checkpoint, recovery)` validates that the
checkpoint belongs to the current workspace, creates `recovery` from the
current state, then restores canonical content in one transaction. On a
synchronized engine the restoration is captured like any other mutation, so it
can be published safely instead of being undone by newer remote packages.

Checkpoint creation is synchronous and proportional to the complete workspace
size. A client must schedule it outside its interactive execution path. The
engine deliberately provides no scheduler or retention policy.

## Personal synchronization

`Engine::open_synced` captures each product mutation in the same SQLite
transaction as the data. The engine groups pending changes into small,
immutable, checksummed `.vrac-sync` packages. Clients only publish and read the
opaque bytes through a provider folder or the equivalent platform storage API.
The engine itself contains no provider-specific code.

Packages are idempotent and ordered per device. Independent changes merge;
true conflicts abort atomically and remain available instead of silently using
last-writer-wins. A package that causally depends on another device is reported
separately so the provider adapter can apply other packages and retry it.
Reopening an active synchronized workspace with `Engine::open` resumes its
existing capture identity; supplying a different identity is rejected. This
prevents an ordinary application restart from silently producing
unsynchronized edits. There is no server, account system, CRDT, or permanent
event history.

The current desktop client presents one concept: a user-selected workspace
folder. Selecting an empty folder creates a workspace; selecting an existing
Vrac folder opens it locally. Separate folders naturally represent separate
workspaces such as Personal and Work. A folder may live inside iCloud Drive,
OneDrive, Dropbox, Syncthing, or another provider.

On first launch the folder chooser remains open until a usable workspace is
selected. There is no unnamed or implicit workspace in the user interface.

The selected folder visibly contains `workspace-id`, `checkpoint.vrac`, and a
`changes` directory of immutable packages. The open SQLite database and its WAL
remain in local application data as a disposable working copy. The application
shows its local size and can remove it without deleting the workspace folder.
It refuses removal while unpublished changes would be lost. A missing folder is
never replaced by silently opening that local copy: Vrac asks the user to locate
the workspace or remove the local copy. A new device validates the checkpoint
and installs its own working copy automatically.

A checkpoint is created with the workspace and refreshed immediately before a
local copy is removed while the folder is available. There is no arbitrary
timer yet; immutable changes remain sufficient to reconstruct the workspace.
If checkpoint publication is interrupted between its two atomic renames, the
next refresh restores the previous known-good checkpoint before trying again.
Package pruning and a periodic compaction policy are deferred until real usage
demonstrates a threshold, avoiding a distributed cleanup protocol in the MVP.
Sync runs off the UI thread after local edits, every two seconds while the
window is visible, and when it regains focus. Any imported package refreshes
the visible outline without interrupting an active edit.

The desktop client also keeps up to seven independent local recovery backups
per workspace in application data. It creates at most one per day, only after
the workspace changed, using a separate SQLite connection so normal editing
does not wait for the complete copy and integrity validation. Backups are
listed in the workspace panel. Restoring one first synchronizes, preserves the
replaced state as another recovery backup, restores atomically, and publishes
the restoration. Removing a local copy also removes its local recovery
backups; it never removes the user-selected workspace folder.

### Local two-device test

Debug builds accept `VRAC_DEV_DATA_DIR` to isolate local application state. Two
processes can therefore represent two devices without ever sharing an open
SQLite database:

```sh
VRAC_DEV_DATA_DIR=/tmp/vrac-device-a src-tauri/target/debug/vrac-app
VRAC_DEV_DATA_DIR=/tmp/vrac-device-b src-tauri/target/debug/vrac-app
```

Each directory receives its own device identity and local database. The debug
window title includes the directory name. Select an empty workspace folder in
the first window, then select the same folder in the second. Creation and local
installation are inferred from the folder contents; there is no separate
enable or join action. This override is absent from release builds.

## Graphical client boundary

The engine is synchronous and can be moved to the graphical client's dedicated
worker thread, keeping SQLite work off the UI thread without an async runtime in
the library. Nodes and identifiers remain engine types; a thin Tauri adapter
only converts command inputs and outputs. `Cursor` implements text conversion
as an opaque token that can cross IPC unchanged and be parsed for the next page.
The token is temporary continuation state, not workspace data.

The interface implements the outline surface, lazy branch expansion,
text editing, focused zoom, bounded pagination, node search, deletion, and
indentation. Long text wraps in both display and editing while the bullet and
disclosure remain aligned with the first line. Display and editing share the
same text box, so activating the caret does not move the text. Every workspace
opens on today's protected Journal node. Journal days are visible nodes tagged
`journal`, so they can be referenced normally;
typing a missing ISO date such as `[[2026-07-22]]` creates that day below
Journal. Root navigation is hidden by default and can be enabled for the
current session with `:root on`, then hidden again with `:root off`.
Hierarchy guides are visible by default and can be hidden for the current
session with `:lines off`, then restored with `:lines on`. The dark theme is
the default; `:light` and `:dark` switch the complete interface palette for the
current session. Typing
`[[` searches other reference targets and offers a new root node when no
matching target exists. Typing `#` lists canonical tags and can apply a
new one without storing the marker in node text. A central, optional Vim
controller drives normal, insert, and node-wise visual modes; the bottom status
control toggles it without changing engine behavior. `yy`, visual `y`, and
`:copy` write complete selected subtrees to the system clipboard as portable
indented text bullets. `p` and `:paste` read that same clipboard and recreate
its hierarchy atomically. Complete `[[labels]]` in typed or pasted text reuse
an exact root concept or create it in the same mutation; an empty root is
removed again when its final reference disappears. `dd` copies successfully before deleting.
The copied text remains directly usable in any other application. `:` and `/`
expand the same bottom area for commands and indexed node search. Search puts
referenced root concepts first, followed by tagged notes, ordinary text, and
finally notes whose match is part of an outgoing `[[reference]]`. Session undo
and redo are available through
`:undo` / `:redo`, Vim `u` / `Ctrl-R`, and the platform `Undo` / `Redo`
shortcuts. When the current bullet has incoming references, their original
Journal day and ancestor bullets appear below the outline. A reference on an
ancestor provides context to tagged descendants, so `#task` or `#decision` can
be filtered without copying the reference into every bullet. The results keep
the ordinary bullet presentation and open the original note when selected.
Results from the same Journal day share one date heading, and common ancestor
paths are merged into one editable outline instead of being repeated for every
match. Tags present in those contextual scopes appear immediately as clickable
badges with their occurrence counts; unrelated workspace tags are omitted.
Virtual lists, mobile controls, and maintenance commands remain outside this
slice.

`CmdOrCtrl+Shift+Space`, the tray icon, and `File > Quick Capture` open the
same small Journal capture window without requiring the main outline. Its
`[[` reference and `#` tag completion use the same bounded searches as the
outline, and the resulting text, stable references, and tags commit in one
engine transaction. Closing capture hides it, and later invocations reuse that
same warm WebView. The unsaved draft remains scoped to its workspace. Vrac does
not integrate with a platform-specific window manager.

The bottom Vim status control is the single notification surface. It briefly
expands for manual synchronization results, recovery operations, and useful
errors,
then retracts to the current mode. `:sync` triggers an immediate
synchronization; successful no-op background rounds remain silent.

Navigation loads path, children, backlinks, and tag facets concurrently, then
publishes one complete visible snapshot. Late responses from an older zoom,
filter, or expanded backlink are ignored, and failed searches or expansions
reach the same visible error surface instead of looking like empty results.

Zoom shows the current node as a restrained heading above its ordinary
children. Protected Journal headings never become editors. A compact,
horizontally scrollable breadcrumb keeps ancestors available for navigation,
stopping at the parent so it never repeats the current heading.

Native `View` menu items control webview display zoom. `File > Open
Workspace…` and `:workspace` open the same small folder chooser. Selecting an
empty folder creates a workspace and selecting an existing Vrac folder opens
it; recent folders are listed as shortcuts. The chooser is mandatory while no
usable workspace is selected. The panel reports local-copy size and exposes an
explicit removal action that never deletes the selected folder.

Release assets run under a restrictive Content Security Policy: scripts remain
limited to packaged assets, IPC is the only connection target, and object,
frame, and base-URL injection are disabled. Development keeps its separate
Vite policy so hot reload does not weaken the packaged application.

Tauri owns one synchronous engine on one dedicated standard thread. Thin
commands adapt children, creation, text updates, paths, moves, deletion, node
search, and tag completion; they contain no SQL or product rules. Switching
workspaces opens the new database on that thread, while checkpoint and provider
work runs outside the UI thread.

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
limits. Performance-data generation is rejected after synchronization has been
activated so generated mutations cannot escape the synchronization outbox.
Synchronization errors distinguish malformed, foreign, out-of-order,
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
`PRAGMA user_version = 3`. Version 2 receives the additive root-label index
migration. The engine validates both the marker and the exact
schema before accepting an existing file. Valid unmarked current databases are
marked only after validation.

Nodes may carry multiple canonical tags outside their text and stable inline
references whose displayed target text follows target renames. Root-level
nodes remain children of a virtual, unstored product root. The public engine
also provides a root-to-node path read for focused navigation. One internal
system key protects the visible Journal container and its ISO calendar-day
nodes; days remain ordinary reference targets and carry the `journal` tag.

File-backed workspaces use foreign-key enforcement, WAL journaling, and
`synchronous = FULL`. The current canonical schema is
[`crates/vrac/schema.sql`](crates/vrac/schema.sql); its compatibility rules are
defined in [`FORMAT.md`](FORMAT.md).
