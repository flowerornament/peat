# peat hooks — Claude Code & Codex integration

The hooks are not an accessory: **installing peat means installing the hooks.** Without them peat records nothing. They do two things: capture every session at every boundary, and at session start hand a resuming agent this desk's last closing message.

Every hook is the same command:

```
peat hook
```

Claude Code and Codex hand each hook a JSON object on stdin that names its own moment (`hook_event_name`), so one verb serves all four. What used to be six bash snippets (jq extraction, once-per-session markers, `nohup` detachment, per-desk `PEAT_DB` anchors and `READY` gates) is now code in `src/hook.rs`, versioned with the binary. **Upgrading peat upgrades the hooks**; the command in your settings file never changes. (0.5.0 retired two events; `peat hook install` removes them.)

## Install

```console
$ peat hook install            # Claude Code → .claude/settings.json
$ peat hook install --local    # Claude Code → .claude/settings.local.json (git-ignored, per-user)
$ peat hook install --codex    # Codex       → .codex/hooks.json
$ peat hook install --check    # exit 0 if wired on all four events, else 1 and what is missing
$ peat hook install --print    # the snippet, for pasting by hand
```

Install also writes the peat skill to `.claude/skills/peat/SKILL.md` (`--codex`: `.agents/skills/peat/SKILL.md`); `peat skill` prints it. Install is a merge, and idempotent: other keys and other tools' hooks (`bd prime`, formatters) are kept; any hook that invokes peat some other way — the copied snippets of releases before 0.4.0 — is removed so each event ends up with exactly one peat hook, and peat is removed from the events it no longer uses (`UserPromptSubmit`, `PostToolUse`). It also creates the ledger if the desk has none, because `peat hook` is a no-op without one. The file is rewritten as pretty JSON with keys sorted.

Verified against the Claude Code hooks docs (2026-08-16) and Codex ≥0.148. There are no `$CLAUDE_TRANSCRIPT_PATH` / `$CLAUDE_SESSION_ID` environment variables; hooks receive **stdin JSON**.

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

Because the hook text is now the constant `peat hook`, this is **one review per desk**, not one per peat release as it was when the snippet changed; Codex keys trust to each hook definition, so after an install that adds or removes events (as 0.5.0 does), open `/hooks` once and trust anything it lists as pending. Codex also accepts the same hooks inline in `.codex/config.toml` (`[[hooks.SessionStart]]` with `hooks = [{ type = "command", command = "peat hook" }]`); a layer carrying both forms loads both and warns, so use one.

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
