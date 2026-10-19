# peat

Agent memory as a fold. This is peat's home repo — extracted from the bogkit hackathon fork (PR flowercomputers/bogkit#18, which stays frozen as the submission record). fold/ese/anny arrive as git dependencies pinned by rev in Cargo.toml; bump the pin deliberately, and send fold improvements upstream as fresh PRs against the fork, never into this repo.

`README.md` is the product document; `.design/` holds the founding design corpus (dated `YYYY-MM-DD-kebab-title.md`; amendments append, never edit).

## Invariants (violations are bugs)

- **stdout is an API.** `peat hook` SessionStart stdout is injected into agent context; `--json` is machine-read. Decoration lives on stderr behind `is_terminal`; piped output is byte-identical to unstyled.
- **No wall-clock reads in any fold path** — time enters only at the capture/render boundary.
- **Additive schema evolution only**; every envelope carries its version.
- **Briefs clip; trails don't** — every clipped line ends in the command that reads the whole thing.
- Oracle tests stay red-capable: `cargo test -- --ignored` must FAIL.

## Build & Quality

```bash
just check     # fmt + clippy -D warnings + test — the land gate
cargo run -- brief
```

This repo captures its own development through the user-level hook `peat hook` (wired once in `~/.claude/settings.json`, `~/.claude-work/settings.json` and `~/.codex/config.toml`, never per repo), which briefs on start and captures at stop, compaction and session end. The ledger lives in the colocated checkout (`~/code/peat/.peat`); workspaces reach it through `.peat/redirect`.

## Version Control

This project uses jj, colocated with git. All agents share one commit graph,
and `main` is the only published bookmark. Each writing agent works in its
own jj workspace; jj runs with stock configuration and records edits
continuously, so there is nothing to stage.

```text
~/code/peat      colocated checkout: .git + .jj, the bd home, the shared
                 .peat ledger, releases (`just release`). No agent edits code here.
~/code/peat-1a   jj workspace, coordinator
~/code/peat-1b   jj workspace, implementer
```

- Work: `jj new main`, edit, `jj describe -m "area: subject (bead-id)"`.
  Pick up others' landings with `jj rebase -d main`.
- Publish: `just land` runs `just check` on exactly the described change,
  then moves `main` to it and pushes. It refuses undescribed work, a stale
  parent, and any edit made while the check ran. The implementer lands after
  the coordinator's GO. A raw `jj git push` skips the gate; never run one.
- Every description gains `Jj-Workspace:`, `Agent:` and `Session:` trailers
  from `scripts/jj-identity`, which a SessionStart hook runs. Do not type or
  strip them.
- Git runs only in the colocated checkout, and only to read (releases are the
  exception: `just release` tags and pushes from there). A jj workspace has no
  `.git`, and `~/.git` exists, so git in a workspace silently answers about
  the home directory. Never run a mutating git command.
- Review a frozen commit by commit id from your own desk, without moving
  `@`: `jj diff -r <commit-id>` and `jj file show -r <commit-id> <path>`.
  To run it, check it out only in a desk that holds no work of its own
  (`jj new <commit-id>` there, `jj abandon @` and `jj new main` after);
  moving another agent's `@` loses their place and costs a rebuild.
- Staleness: when another agent rewrites a commit under your `@`, every jj
  command refuses until `jj workspace update-stale`. Never `jj edit` a change
  that is an ancestor of another workspace's `@`.
- Recovery starts at `jj op log`, `jj undo` and `jj op restore`, before any
  destructive file operation. The `jj-ops` skill holds the jj model.

Completion: `jj describe`, then `just land`; `bd dolt push` if issues
changed. Work is not complete until `just land` succeeds.

## Work Tracking

`bd` holds all task state; `bd prime` is its full reference. Its one database
lives in `~/code/peat/.beads`, which every workspace reaches through
`.beads/redirect`. Claim before building, close on completion, cite the bead
id in the commit subject. Publish bd state with `bd dolt push`
(`BEADS_FSCK_TIMEOUT=3m` until the next nix rebuild), never in a commit. Use
`bd remember` for durable project knowledge, not memory files.
