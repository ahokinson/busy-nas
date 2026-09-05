# busy-nas

Single-writer handoff for unfinished work.

## Why

Moving a dirty worktree between machines is easy; knowing which copy is current is not. `busy-nas` makes the NAS the canonical WIP tree and treats the local checkout as a temporary place to work.

This is not a Git workflow. Git remains useful for history, review, and release, but it is not asked to coordinate a half-finished worktree. `busy-nas` copies `.git` when it exists, but never creates commits, changes branches, or contacts a remote.

- The NAS holds the only canonical WIP tree.
- A lease gives one machine exclusive ownership of a project.
- A local checkout is disposable after `put` or `discard`.
- Handoffs are explicit. There is no automatic conflict resolution or stale-lease takeover.

## Configuration

The configuration file is `~/.config/busy-nas/config.toml` on Linux and the platform configuration directory on macOS. Use `--config` or `BUSY_NAS_CONFIG` to override it.

```toml
# Local, temporary worktree root.
workspace_root = "/home/you/Developer"
snapshot_retention = 20

[nas]
host = "nas.home"
user = "developer" # optional
# Canonical source root: <root>/<project>.
root = "/srv/developer"
```

`workspace_root` must be an existing local directory, not a symlink or an NFS, SMB/CIFS, or SSHFS mount. Local lease tokens are kept in the platform state directory (`~/.local/state/busy-nas/` on Linux).

The NAS needs SSH, rsync, and write access to `nas.root`; it is never mounted locally.

## Commands

| Command                     | Meaning                                                                                         |
| --------------------------- | ----------------------------------------------------------------------------------------------- |
| `get <project>`             | Acquire the lease and copy NAS source to the local workspace.                                   |
| `put <project>`             | Snapshot NAS source, sync local changes back, verify, then remove the local checkout and lease. |
| `discard <project>`         | Remove the local checkout and release the lease without changing NAS source.                    |
| `status`                    | Show projects, NAS leases, and local lease records.                                             |
| `reclaim <project> --force` | Replace a lease only when its owner machine is gone or unavailable.                             |

`put` propagates source deletions. Before changing the canonical tree, it creates a timestamped snapshot under `<nas.root>/.busy-nas/snapshots/<project>/` and retains the configured number of snapshots. Leases live under `<nas.root>/.busy-nas/leases/<project>`.

## What moves

Everything needed to resume work moves: `.git`, lockfiles, `vendor/`, `dist/`, untracked files, and uncommitted changes.

The local build and environment artifacts below do not move:

- `node_modules/`
- `target/`
- `.direnv/`
- `.devenv/`
- `result`
- `result-*`

Excluded paths already on the NAS are preserved by `put`.
