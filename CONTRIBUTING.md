# Contributing to Vrac

Vrac keeps its development workflow as small and explicit as the product.
Every change reaches `main` through one short-lived branch and one pull request.

## Make a change

1. Update `main` and create a `feature/`, `fix/`, `docs/`, or `chore/` branch.
2. Keep the branch focused on one coherent change.
3. Add or update tests when behavior changes.
4. Run the checks required by the scope. Rust changes require at least:

   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   ```

5. Open a pull request with an imperative, user-readable title. Explain why
   the change exists and list the verification performed.

## Label the pull request

Every pull request has exactly one release-note label:

| Label | Use it for |
| --- | --- |
| `breaking-change` | An incompatible public change |
| `enhancement` | A new or improved user-facing capability |
| `bug` | A user-facing correction |
| `documentation` | Documentation only |
| `dependencies` | Dependency updates only |
| `maintenance` | Internal engineering worth mentioning |
| `skip-changelog` | Internal work users do not need to see |

GitHub rejects an unlabeled or ambiguously labeled pull request. Keep exactly
one validated commit on the branch and make its subject match the pull request
title. Once the required checks pass and conversations are resolved,
rebase-merge it and delete the branch. The same title becomes the commit on
`main` and the entry used by the generated release notes.

## Publish a release

Vrac follows Semantic Versioning. Prepare a release in its own pull request:

1. set the workspace version in `Cargo.toml` and refresh `Cargo.lock`;
2. label the version-only pull request `skip-changelog`;
3. merge it after CI passes;
4. create and push an annotated `vX.Y.Z` tag on that merge.

The release workflow verifies that the tag matches the workspace version,
builds the supported archives, generates SHA-256 checksums, and creates the
GitHub release from merged pull requests. Published tags and release artifacts
are never replaced; a correction receives a new version.

The first public release needs a short human-written overview because the
repository predates this pull-request workflow. Later releases should need
little or no manual changelog editing.
