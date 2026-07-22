# Vrac contribution rules

These rules apply to the entire repository. When `ARCHITECTURE.md` is present
in the local workspace, read it before making changes. It is a private working
document and is intentionally not versioned. This file is the versioned minimum
set of rules for all interventions.

## Priority order

When tradeoffs are necessary, preserve this order:

1. do not lose or corrupt data;
2. keep the code understandable and easy to change;
3. respond immediately;
4. remain efficient with several million nodes;
5. preserve the invariants required for future synchronization;
6. keep data readable and recoverable over the long term.

A speed optimization must never silently weaken durability or integrity.

## Architecture boundaries

- `vrac` is the engine library. It owns the model, business rules, SQLite
  schema, queries, and transactions.
- `vrac-cli` is a thin client of the public `vrac` API. It parses arguments,
  calls the engine, presents results, and selects exit codes.
- The CLI, Tauri, and future interfaces contain no SQL or business rules.
- The engine depends on no user interface, graphical framework, or network
  service.
- The engine remains synchronous until measurements and a concrete need
  justify a change.
- SQLite is the only source of truth for current state.

Do not bypass the engine's public API to save time in a client. If the API
cannot express a legitimate action, evolve the API and its tests.

## Simplicity

Apply KISS and YAGNI strictly:

- represent the problem directly with concrete types and few layers;
- prefer a clear function over an internal framework;
- do not create a trait when there is only one real implementation;
- do not add a generic repository, dependency injection, ORM, event bus, or
  asynchronous runtime without a demonstrated present need;
- do not add modules, directories, or configuration for hypothetical future
  use;
- do not duplicate business types between the engine and its clients;
- add a dependency only when it removes more code, risk, or complexity than it
  introduces.

A new abstraction must remove complexity that has already been observed. When
in doubt, start with the clearest direct implementation.

## Data and SQLite

- Every business mutation is atomic and runs in an explicit transaction.
- Enable and preserve the SQLite guarantees defined by the architecture,
  including foreign keys and the selected synchronization level.
- Never use `rowid` as a node's business identity.
- Never open the active database from a network share or a directory managed
  by a file synchronization service.
- Never copy an open database and its WAL directly to create a backup or
  checkpoint.
- Every schema change increments `PRAGMA user_version` and uses an explicit,
  tested migration.
- Never modify a published migration or a database sample representing an old
  version.
- Never perform a potentially destructive transformation without a recoverable
  copy and integrity validation.
- Search indexes and all other derived data must be rebuildable from canonical
  tables.

Storage failures must surface as useful errors. The library never prints and
never terminates the process.

## Model invariants

- An identity is opaque, stable, and independent from the local database.
- Siblings are always returned in deterministic order.
- Clients express sibling order through `Placement`; numeric storage positions
  are not part of the public API.
- Collection reads use cursor pagination, never `OFFSET` on paths intended for
  large collections.
- A normal operation neither loads nor traverses the entire tree.
- Moving a subtree does not rewrite its descendants.
- Every creation or move validates that its parent exists.
- Every move prevents cycles in the same transaction.

Any invariant change must be explicit in the local architecture document and
covered by tests before it is considered complete.

## Performance

Several million nodes are a real constraint, but not a reason for speculative
optimization.

Interactive engine operations have a budget of 2 ms at p95 on the reference
machine. This budget covers loading a page of 100 nodes, including a page where
every node has tags and references, reading one node, creating or editing
content, replacing tags, moving a node, and reading a representative deep path.
Full integrity checks, exports, bulk generation, and complete-tree traversal are
background operations and are not subject to this interactive budget.

The packaged macOS reference product has separate release gates: the app
bundle stays below 8 MiB, a fresh-state launch reaches its first usable view in
under 1.5 seconds, the complete app stays below 96 MiB of physical footprint on
the initial view and 224 MiB after the 100-create/100-edit/20-reload stress
scenario, and steady idle CPU remains below 1%. Interactive create, persisted
edit, and full 100-row reload samples must complete before the next 60 Hz paint
at p95. Measure physical footprint across the app and its dedicated WebKit
helpers; summed RSS double-counts shared memory.

This is a regression gate for the reference machine, not an assumption that
all storage hardware behaves identically. Before release, measure representative
Apple, Android, Windows, and Linux devices and record their own baselines
without weakening the reference-machine gate.

- Keep frequent accesses indexed by parent, position, and identifier.
- Avoid complete loads, updates proportional to subtree size, and needlessly
  long transactions.
- Measure sensitive changes in `--release` mode, on a file, with a
  representative tree shape.
- After every engine or schema change that can affect interactive work, run the
  five-million-node release scenario and verify every reported interactive p95.
- After frontend or Tauri changes that can affect rendering, startup, IPC, or
  retained state, rerun the packaged product scenario on the reference Mac.
- The performance dataset must exercise plain nodes, metadata-rich pages,
  multiple tags, resolved references, target renames, content and tag edits,
  sibling moves, and paths at several depths.
- Compare before and after with a reproducible scenario.
- Keep an optimization only when its measured gain justifies its readability
  and maintenance cost.
- Do not claim a performance property without a benchmark or query-plan
  analysis that demonstrates it.

Desktop WebKit measurements are not mobile claims. Record separate packaged
baselines on representative iOS, Android, Windows, and Linux devices before
release.

A test with little data validates correctness, not scalability.

## Tests and verification

- Test the engine primarily through its public API and with a real SQLite
  database.
- Add a regression test for every bug fix.
- Use file-backed tests for behavior involving closing, reopening, migrations,
  or observable durability.
- Cover transactional failure paths: a failed mutation leaves no partial state.
- At minimum, cover deterministic ordering, pagination without duplicates or
  omissions, missing parents, cycles, and reopening persisted data.
- Keep tests deterministic and independent of execution order.

Before completing a change, run checks proportional to its scope. For Rust,
run at least formatting, workspace tests, and compiler diagnostics. Add Clippy
and relevant benchmarks when warranted. Clearly report any check that could not
be run.

Do not perform visual or manual UI inspection unless the user explicitly asks
for it. Run the relevant automated checks and leave appearance validation to
the user by default.

## Change discipline

- Make the smallest complete change that satisfies the present need.
- Preserve existing work and do not reformat or reorganize unrelated code.
- Do not change behavior and architecture simultaneously without an explicit
  reason.
- Record durable decisions in the local `ARCHITECTURE.md`, not only in code or
  a commit message.
- Keep API and schema documentation current when they change.
- Reject invalid states at the nearest engine boundary and return a precise
  error.
- Do not add future functionality merely to make an abstraction feel complete.
- Keep code, comments, documentation, error messages, and CLI output in
  English.
- After a change passes its required validation, commit it before starting
  unrelated work. Leave validated work uncommitted only when the user
  explicitly requests it.

A change is complete when the requested behavior exists, invariants are
preserved, relevant tests pass, and affected documentation is consistent.
