# Gitoxide migration

The production Rust library and `git-meta` CLI use the `grix` branch of
Gitoxide. Repository operations use library APIs: metadata remote discovery and
configuration, fetch/push, shallow history, object hydration, release queries,
README publication, changed-path inspection, blame, and object maintenance.
There is no production `git2` dependency or generic Git subprocess helper.

Git remains an interoperability oracle in tests and in the benchmark's repository
setup and packed-size measurement. The independent Ruby example scripts still
invoke Git. Pagers, GitHub CLI integration, and configured external programs
(transport services, credential helpers, hooks, filters, and text converters)
remain external programs. Sender and recent-object hooks use `gix-hook`.

## Local builds

Both `gix` and `gix-testtools` name the public Gitoxide URL and `grix` branch.
The portable lock records the published revision `ff0bc29`, which contains the
required backend APIs. Ordinary Cargo builds and CI need no local overrides.
For local Gitoxide development, use a per-invocation patch pointing at the
existing `grix` worktree:

```toml
# Save outside either checkout, e.g. /tmp/git-meta-grix-local.toml.
[patch."https://github.com/GitoxideLabs/gitoxide"]
gix = { path = "/path/to/gitoxide.grix/gix" }
gix-testtools = { path = "/path/to/gitoxide.grix/tests/tools" }
```

```sh
cargo test --workspace --all-targets --config /tmp/git-meta-grix-local.toml
cargo clippy --workspace --all-targets --fix --allow-dirty \
  --config /tmp/git-meta-grix-local.toml -- -D warnings
```

Cargo rewrites lock entries when a path patch is active. Preserve the portable
lock before local builds and restore that saved copy afterward; do not commit
the path-resolved lock or a machine-specific Cargo configuration.

## Behavior and limits

Native CLI tests run with an empty `PATH` and cover two-client metadata exchange,
historical hydration, sender-hook rejection, multiple push destinations, README
publication, inspection, and maintenance. Gitoxide tests compare protocol v1/v2
filtered fetch and explicit object hydration, partial-clone push, shallow merge
histories, and failure cases with Git. A server without filter support sends the
full selection, as with Git.

Synchronization hydrates the common ancestor needed for three-way merges as well
as the fetched tip. Standalone materialization remains an offline operation and
requires its input trees and blobs to be present.

Automatic maintenance compacts all local objects without expiring them, keeps
promisor objects separate, preserves object ages, and respects kept packs and
collector locks. It currently rewrites unkept packs instead of choosing geometric
subsets and reuses existing deltas without searching for new ones. Retaining
unreachable objects protects hydrated values whose containing history is absent;
it can use more space than the former immediate pruning.

Metadata remotes support the `blob:none` filter used by setup. Other filter
expressions fail explicitly. The large persistent scale fixture is not used by
migration tests. The existing CI workflow builds the pinned public backend.

Blame intentionally differs from Git in two cases: it clears stale unblamable
markers after successful attribution to a later merge parent, and applies empty
ignore-file resets in configuration order.
