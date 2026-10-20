# peat hooks — Claude Code & Codex integration

The hooks are not an accessory: **installing peat means installing the hooks.** Without them peat records nothing. They do two things: capture every session at every boundary, and at session start hand a resuming agent this desk's last closing message.

Every hook is the same command:

```
peat hook
```

Claude Code and Codex hand each hook a JSON object on stdin that names its own moment (`hook_event_name`), so one verb serves all four. What used to be six bash snippets (jq extraction, once-per-session markers, `nohup` detachment, per-desk `PEAT_DB` anchors and `READY` gates) is now code in `src/hook.rs`, versioned with the binary. **Upgrading peat upgrades the hooks**; the command in your settings file never changes. (0.5.0 retired two events; `peat hook install` removes them.)

## Install: once per machine, then `peat init` per repo

`peat hook` does nothing where there is no ledger, so the hook belongs in the **user-level** configs, written once and correct in every repo; a repo opts in by having a ledger.

```console
$ peat hook install            # the user-level configs (below)
$ peat hook install --check    # per-file report; exit 1 on any drift
$ peat hook install --print    # what each file would receive, without writing
$ peat init                    # in a repo: create its ledger (idempotent)
```

The user-level configs:

| harness     | file                                                                                                  |
| ----------- | ----------------------------------------------------------------------------------------------------- |
| Claude Code | `~/.claude/settings.json`, `$CLAUDE_CONFIG_DIR/settings.json`, and `settings.json` in every live `~/.claude-*` dir (it also holds `projects/`, e.g. `~/.claude-work`; a backup copy does not) |
| Codex       | `$CODEX_HOME/config.toml`, else `~/.codex/config.toml` when `~/.codex` exists — `[[hooks.<Event>]]` blocks |

Install is a merge, and idempotent: other keys and other tools' hooks (`bd prime`, `hail`, formatters) are kept where they are. A hook that runs peat some other way — the copied snippets of releases before 0.4.0, matched only where peat is the program a command runs and is followed by a verb a peat hook ran (`hook`, `capture`, `brief`, `obs`, `push`, `distill`, `recall`), never as a substring or inside quoted text except the script of a shell's `-c` (`repeat …`, `say "peat finished"` and `hail fyi peat 'build ok'` are not peat; `sh -c 'peat capture …'` is; `/nix/…/bin/peat hook` is the current hook) — is replaced by `peat hook` in its own slot, so no other tool's group changes index (Codex keys hook trust to `path:event:group:hook`); peat is removed from the events it no longer uses (`UserPromptSubmit`, `PostToolUse`). Where that removal moves another tool's hook to a lower index (`[peat hook, hail deliver]` on a retired event), install names each moved hook so you can re-trust it in Codex's `/hooks`. A file of a shape peat does not write (a top-level array, `"hooks": "…"`, an event that is not an array, an inline `hooks = {…}` or a plain `[hooks.Stop]` table) is refused with the file and key named, never repaired, and a file peat cannot read (permissions, not UTF-8) is refused, never overwritten. A file is written only when its content changes, atomically (a temp file beside it, created with the file's mode — 0600 for a new one — then a rename; a symlinked config is written at its target, and a read-only target is refused). Because the write is a rename, a hardlinked config becomes a separate file: the other links keep the old content. A JSON rewrite normalizes key order (pretty JSON, keys sorted); Codex's `config.toml` is edited in place, so its comments, ordering and trust records (`[hooks.state]`) survive. `--check` reports drift per file — an event missing `peat hook`, or a stale peat entry left on a retired event — and exits 1 on any.

**Per-repo copies fire twice, harmlessly.** Releases up to 0.5.0 wrote the hooks into each repo (`.claude/settings.json`, `.claude/settings.local.json`, `.codex/hooks.json`; some repos carry `[[hooks.*]]` in `.codex/config.toml`). Beside the user-level hook these run a second time. The duplicate guard keeps the brief single — a `SessionStart` with the same session id and `source` that fires twice within 10 s prints once (the twins decide under an exclusive lock on a marker in `.peat/fired/`) — and a duplicate capture is idempotent. So install, run inside a repo, removes peat only from the copies that are not committed work (asked read-only of jj, or of git in a git-only repo; a file neither can vouch for counts as tracked), keeping every other tool's hooks and deleting a file that held only peat's. A tracked copy is reported with the command that removes it, `peat hook install --strip-project`. `--check` lists repo copies as notes, never as drift, so an intentional `--project` setup stays green. The home directory is never treated as a repo, even when `~/.git` exists: its `.claude/` and `.codex/` are the user-level configs.

`peat init` creates `.peat/db` at the repo root and prints where; in a jj workspace or git worktree it creates the anchor repo's ledger, which every desk of that repo then shares. It is the one way a ledger comes to exist (bar an explicit `PEAT_DB`): a read in a repo that never ran it says so rather than creating one.

**The per-repo form** is still there for a repo shared with people who lack the user-level hook:

```console
$ peat hook install --project            # .claude/settings.json at the desk root
$ peat hook install --project --local    # .claude/settings.local.json (git-ignored, per-user)
$ peat hook install --project --codex    # .codex/hooks.json
```

It also writes the peat skill to `.claude/skills/peat/SKILL.md` (`--codex`: `.agents/skills/peat/SKILL.md`) and creates the ledger. `--check` passes on a desk with no skill copy (it may be installed user-level) and fails only on a stale one; `peat skill` prints it.

Verified against the Claude Code hooks docs (2026-08-16) and Codex ≥0.148. There are no `$CLAUDE_TRANSCRIPT_PATH` / `$CLAUDE_SESSION_ID` environment variables; hooks receive **stdin JSON**.

## The skill: user-level, matching the installed peat

The hooks capture; the skill (`skills/peat/SKILL.md`) tells an agent when to read what was captured. It belongs where the hooks are, once per machine, and should be the copy that ships with the installed binary. Only `--project` writes it into a desk; a user-level install never does, so no copy lands in a desk's working change.

- **home-manager:** the package installs the skill at `$out/share/peat/skills/peat/SKILL.md`, and the module links that directory into each home-relative target, so an upgrade of peat upgrades the skill:

  ```nix
  programs.peat.enable = true;
  programs.peat.skill.enable = true;
  # default targets: [ ".agents/skills/peat" ".claude/skills/peat" ]
  programs.peat.skill.targets = [ ".agents/skills/peat" ".claude/skills/peat" ".claude-work/skills/peat" ];
  ```

  Codex reads `~/.agents/skills` (and `$CODEX_HOME/skills`); Claude Code reads `skills/` in its config dir, so each extra `CLAUDE_CONFIG_DIR` needs its own target. `skill.enable` is off by default, like the other modules on this pattern (anneal, ferry, hail): Home Manager refuses to replace a real file, so a machine that ran `peat skill --user` deletes those copies before switching.
- **without nix:** `peat skill --user` writes the skill to `skills/peat/SKILL.md` in every Claude Code config dir that exists (the same dirs install wires) and, when Codex is set up, to `~/.agents/skills/peat/SKILL.md` — atomically like the configs, mode 0644. Each harness ends with one copy, never two: one that already reads a current copy is left as it is; one whose skill path anywhere below its config dir (or `~` for `.agents`) is a symlink — home-manager's store links, or a `skills/` dir linked into a shared hub — is left alone entirely; an existing regular copy (in `~/.agents/skills` or `$CODEX_HOME/skills` for Codex) is rewritten in place; only a harness with no copy gets a new file. Run it again after an upgrade.

`peat hook install` and `peat hook install --check` report the skill per harness: `✓` for a current copy, a note for a copy that differs from this binary's (another peat build installed it — expected while running a development binary beside a home-manager one) and a note naming both ways to get one when a harness has none. These are notes, never drift: `--check` does not fail over the skill.

## What `peat hook` does at each moment

| stdin `hook_event_name` | does |
| ----------------------- | ---- |
| `SessionStart`          | writes `.peat/current-session`; prints (**stdout is injected as context**) this desk's previous closing message (another session, ended within three days) and one line on how to look things up. Lock wait capped at 15 s |
| `Stop`                  | detached `peat capture <transcript> --final-msg <last_assistant_message>` — never blocks |
| `PreCompact`            | detached salvage capture |
| `SessionEnd`            | detached salvage capture |

Any other event, including the retired `UserPromptSubmit` and `PostToolUse`, prints nothing and exits 0, so an entry an older install left behind is harmless until `peat hook install` removes it.

### What SessionStart injects

At most two things, and nothing else:

```text
peat: this desk's last session (5h ago) closed with:
  Landed the hook rewrite; `peat hook install --check` passes on all four events. …  ▸ peat a2a9fc17
peat: to see the sessions behind a commit, change, bead, file or topic before you change it, run `peat <commit|change|bead-id|path|words>`.
```

"This desk" is the workspace: under jj each writing agent has its own, so the closing message is the last one written from this workspace, never another agent's. The closing message is shown only when that session is not the one starting (a resume or compaction already holds its own context) and ended within three days (older work is better read on demand than pushed); otherwise only the lookup line appears. On a busy ledger the lock wait is capped at 15 s, after which only the lookup line is printed. No counts, no rulings, no observations: the dashboard (`peat`) is for a person at a terminal, and everything else is pulled when needed, e.g. `peat <commit>` before amending it.

Captures run detached: `peat hook` spawns `peat capture` in its own process group with stdio closed and returns in milliseconds, so a harness deadline or teardown cannot cancel the work. The closing message travels as an argv element, never re-quoted through a shell — a hostile final message (quotes, backticks, `$VAR`, backslashes) round-trips byte-exact. On Codex, when the Stop payload carries no `transcript_path`, the rollout is found under `~/.codex/sessions/` by session id.

Two rules the implementation keeps, and that every path exits 0 to honour: **peat failing may never break a session**, and **peat never blocks** — no `decision: block`, no exit 2. A blocking Stop hook renders as a `hook error`, force-continues the turn, and displaces the reply the user was reading; it is enforcement machinery, wrong for soliciting judgment.

## Switching it off

`peat hook` does nothing, silently, when:

- the desk has no ledger — no `.peat/db`, no `.peat/redirect`, and no worktree anchor that has one (see **Worktree desks**);
- a `.peat/off` file exists beside the desk's `.peat` or beside the shared ledger — the pause switch. `touch .peat/off` stops every seat of a shared ledger; `rm` resumes.

(Releases before 0.4.0 used the inverse, an opt-in `READY` marker. Delete any you still have; they are ignored.)

## Worktree desks

Several desks, one memory: write a `.peat/redirect` in each desk (one line, the anchor's `.peat` relative to the desk root — e.g. `../murail/.peat`). Bare `peat` and `peat hook` both resolve through it; desk-local files (`current-session`) stay beside the redirect.

A git worktree (`.git` is a file) or a secondary jj workspace (`.jj/repo` is a file) needs no redirect at all: with no ledger of its own it resolves to the main checkout's `.peat/db` automatically, when that exists. So a desk Claude Code creates under `.claude/worktrees/` remembers into the shared ledger with no setup and no hardcoded path in the hook. `PEAT_DB=/path/to/.peat/db` in the environment remains the explicit override.

## Codex: hooks must be trusted before they run

Codex gates every non-managed command hook behind an explicit review. Installing `.codex/hooks.json` installs nothing that runs: Codex records trust against the **hash of the exact hook definition**, and an untrusted hook is skipped **silently** — no error, no output, no entry in the ledger.

Two independent gates, both required:

1. **Project trust.** Project-local hooks load only when the project's `.codex/` layer is trusted; in an untrusted project Codex ignores project config, hooks, and rules entirely.
2. **Hook trust.** Run `/hooks` in the Codex CLI to inspect sources, review new or changed hooks, and trust them. When review is pending at startup, Codex prints a warning pointing at `/hooks` — easy to miss in a scrollback, and the only symptom you get.

Because the hook text is now the constant `peat hook` and lives in the user config, this is **one review per machine**, not one per peat release or per desk; Codex keys trust to each hook definition, so after an install that adds or removes events (as 0.5.0 does), open `/hooks` once and trust anything it lists as pending. The user-level install writes the inline form into `~/.codex/config.toml` (`[[hooks.SessionStart]]` with `hooks = [{ type = "command", command = "peat hook" }]`); `--project --codex` writes `.codex/hooks.json`. A layer carrying both forms loads both and warns, so use one.

For non-interactive automation that vets hook sources by other means, `codex exec --dangerously-bypass-hook-trust` runs enabled hooks without persisted trust for that invocation.

**Verify by firing, not by reading.** After installing (and, on Codex, trusting), start a fresh session in the project and check that the mechanical half happened:

```console
$ cat .peat/current-session      # written by SessionStart; should hold the new session id
$ peat <that-id>                 # the session should exist in the ledger after the first Stop
```

If `current-session` still holds an older id, the hook did not run — on Codex the usual cause is pending review in `/hooks`.

You can also fire it by hand with a synthetic payload:

```console
$ echo '{"hook_event_name":"Whatever"}' | peat hook   # prints nothing, exit 0
```

(A synthetic `SessionStart` overwrites `.peat/current-session`; do that only in a desk with no live session.)

## Deadlines: why a hook gets cancelled

A harness cancels hooks that are still running when it needs to move on — most visibly at teardown:

```
SessionEnd hook [...] failed: Hook cancelled
```

That message means the hook was aborted, not that it failed. `peat hook` returns in milliseconds at every capture moment because the capture is detached, and SessionStart bounds its lock wait at 15 s (peat's default of 120 s is right for a human at a terminal, far too patient for a hook); past that the injection is skipped silently, which is the intended behaviour on a busy shared ledger.

Why a capture can be slow at all: writing to the ledger periodically has to fold the journal into the LSM, and that fold rewrites index structures sized by the whole corpus rather than by the events just captured. peat amortizes it — a capture folds only when it did bulk work or when the journal has grown enough that the next open would replay it anyway — so the cost lands rarely and, because every capture is detached, where nobody is watching. Measured on a 147 MB shared ledger: a **cold** capture of a 12.8 MB / 5,031-line transcript takes ~6 s; a warm re-capture of the same file ~0.3 s. The gap is the per-transcript cursor, so the expensive case is a transcript peat has never seen — exactly what a `--resume` of a long-lived session hands to `SessionEnd`.

**A cancelled hook costs at most a delay.** The transcript stays on disk and capture is idempotent, so anything missed is recovered by capturing that file again:

```console
$ peat capture ~/.claude/projects/<slug>/<session-id>.jsonl
```

## Compaction

`PreCompact` fires before compaction replaces the context window: it cannot consult the agent (no turn exists), so it runs a mechanical salvage capture — idempotent upserts make the later Stop capture re-cover the same events for free. Claude Code's own summary is captured as a `CompactSummary` event whenever the transcript is ingested, and is searchable like any other text. Codex encrypts its compaction summary, so a Codex compaction leaves only the marker.

## The moment-coverage matrix

Every moment a session can produce or lose knowledge, and the hook that covers it:

| moment                            | hook                               | does |
| --------------------------------- | ---------------------------------- | ---- |
| session begins                    | `SessionStart`                     | session id written; last closing message (on resume) and the lookup line injected |
| every stop                        | `Stop`                             | capture (`--final-msg` authoritative) — never blocks |
| context about to compact          | `PreCompact`                       | salvage capture |
| session resumes after compact     | `SessionStart` (`source: compact`) | as at session begin |
| `/clear` or other non-Stop ending | `SessionEnd`                       | salvage capture |

A prompt arriving is not a moment peat acts on: nothing is pushed per prompt.

Codex parity was verified end to end at 0.149: matchers and the stdin contract are identical (`session_id`, `transcript_path`, `cwd` all arrive as documented), and `SessionStart` stdout *is* injected into the Codex model's context. Subagent sessions do not fire the peat hooks — only top-level sessions deposit.

## Observations

peat never asks an agent to write one. `peat obs <subject> "<claim>" --from <seq>` remains for a hard-won claim that will outlive the work and that no commit, bead or instruction file would carry; the skill says when. Observations are shown only when asked (`peat subjects`, `peat <subject>`).
