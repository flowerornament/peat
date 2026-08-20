# peat hooks — Claude Code & Codex integration

The hooks are not an accessory: **installing peat means installing the
hooks.** The binary alone is a CLI you must remember to run; the fabric —
brief on wake, capture at every boundary, invisible deposit nudges — is
entirely below. Hooks are copied config you own; see **Keeping hooks
current** for the update contract.

Verified against the Claude Code hooks docs (2026-08-16) and Codex ≥0.148.
The contract differs from early drafts of the spec in one important way:
**there are no `$CLAUDE_TRANSCRIPT_PATH` / `$CLAUDE_SESSION_ID` environment
variables.** Hook commands receive a JSON object on **stdin**; extract
fields with `jq`.

## Stdin contract (fields we use)

Every moment receives at least:

```json
{
  "session_id": "abc123",
  "hook_event_name": "SessionStart | UserPromptSubmit | PostToolUse | Stop | PreCompact | SessionEnd",
  "cwd": "/path/to/project",
  "transcript_path": "/Users/you/.claude/projects/<slug>/<session>.jsonl"
}
```

`Stop` additionally carries `last_assistant_message` (final text of the
turn — authoritative, because the transcript file may lag) and
`stop_hook_active`. `PostToolUse` carries `tool_name` and `tool_input`
(the commit-nudge matcher reads `.tool_input.command`). `SessionStart`
carries `source`: `startup | resume | clear | compact | fork`.

## Stdout treatment

`SessionStart` is one of the few hooks whose **plain-text stdout is added
to the session context** — exactly what `peat brief` wants. The judgment
nudges (`UserPromptSubmit`, `PostToolUse`) must instead print a JSON object
with an `additionalContext` key — invisible to the user, weighed by the
agent. `Stop`/`PreCompact`/`SessionEnd` stdout goes to the debug log only;
rely on exit code 0.

## Snippet — `.claude/settings.json` (project)

All six moments. Copy whole; per-project edits (shared-db `PEAT_DB`
anchors, `READY` gates) go on top — see **Worktree desks**.

<!-- hooks snippet v3 · 2026-08-20 · claude -->

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "in=$(cat); cmd=$(printf '%s' \"$in\" | jq -r '.tool_input.command // empty'); case \"$cmd\" in *'git commit'*|*'jj describe'*|*'just land'*) printf '{\"additionalContext\":\"a commit landed — deposit peat obs <subject> \\\\\"<one-line claim>\\\\\" for anything durable learned this change\"}' ;; esac; exit 0"
          }
        ]
      }
    ],
    "PreCompact": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "in=$(cat); tp=$(printf '%s' \"$in\" | jq -r '.transcript_path // empty'); { [ -n \"$tp\" ] && peat capture \"$tp\"; } 2>/dev/null || true"
          }
        ]
      }
    ],
    "SessionEnd": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "in=$(cat); tp=$(printf '%s' \"$in\" | jq -r '.transcript_path // empty'); { [ -n \"$tp\" ] && peat capture \"$tp\"; } 2>/dev/null; exit 0"
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "in=$(cat); mkdir -p .peat; printf '%s' \"$in\" | jq -r '.session_id' > .peat/current-session 2>/dev/null; peat brief 2>/dev/null || true; src=$(printf '%s' \"$in\" | jq -r '.source // empty'); if [ \"$src\" = \"compact\" ]; then echo \"\"; echo \"peat: context was just compacted — if durable knowledge from before the compaction is not yet deposited, do it now from the summary: peat obs <subject> \\\"<one-line claim>\\\"\"; fi; exit 0"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "in=$(cat); tp=$(printf '%s' \"$in\" | jq -r '.transcript_path // empty'); fm=$(printf '%s' \"$in\" | jq -r '.last_assistant_message // empty'); { [ -n \"$tp\" ] && peat capture \"$tp\" --final-msg \"$fm\"; } 2>/dev/null || true"
          }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "in=$(cat); sid=$(printf '%s' \"$in\" | jq -r '.session_id // empty'); mk=.peat/nudged-\"$sid\"; if [ -n \"$sid\" ] && [ ! -f \"$mk\" ]; then mkdir -p .peat; touch \"$mk\"; printf %s \"{\\\"additionalContext\\\": \\\"peat is recording this session. At natural completion points \\\\u2014 a commit, a finished task \\\\u2014 deposit durable knowledge: peat obs <subject> \\\\\\\"<one-line claim>\\\\\\\" [--from seq,seq]. Read a belief trail with: peat <subject>.\\\"}\"; fi; exit 0"
          }
        ]
      }
    ]
  }
}
```

## Snippet — `.codex/hooks.json` (project)

Identical contract (Codex ≥0.148 hooks engine is Claude-compatible:
`postToolUse` supported, shell tool serializes canonical
`tool_name: "Bash"`). One difference: Stop falls back to finding the
rollout under `~/.codex/sessions/` when stdin carries no transcript path.

<!-- hooks snippet v3 · 2026-08-20 · codex -->

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "in=$(cat); cmd=$(printf '%s' \"$in\" | jq -r '.tool_input.command // empty'); case \"$cmd\" in *'git commit'*|*'jj describe'*|*'just land'*) printf '{\"additionalContext\":\"a commit landed — deposit peat obs <subject> \\\\\"<one-line claim>\\\\\" for anything durable learned this change\"}' ;; esac; exit 0"
          }
        ]
      }
    ],
    "PreCompact": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "in=$(cat); tp=$(printf '%s' \"$in\" | jq -r '.transcript_path // empty'); { [ -n \"$tp\" ] && peat capture \"$tp\"; } 2>/dev/null; exit 0"
          }
        ]
      }
    ],
    "SessionEnd": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "in=$(cat); tp=$(printf '%s' \"$in\" | jq -r '.transcript_path // empty'); { [ -n \"$tp\" ] && peat capture \"$tp\"; } 2>/dev/null; exit 0"
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "in=$(cat); mkdir -p .peat; printf '%s' \"$in\" | jq -r '.session_id' > .peat/current-session 2>/dev/null; peat brief 2>/dev/null || true; src=$(printf '%s' \"$in\" | jq -r '.source // empty'); if [ \"$src\" = \"compact\" ]; then echo \"\"; echo \"peat: context was just compacted — if durable knowledge from before the compaction is not yet deposited, do it now from the summary: peat obs <subject> \\\"<one-line claim>\\\"\"; fi; exit 0"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "in=$(cat); sid=$(printf '%s' \"$in\" | jq -r '.session_id // empty'); tp=$(printf '%s' \"$in\" | jq -r '.transcript_path // empty'); fm=$(printf '%s' \"$in\" | jq -r '.last_assistant_message // empty'); [ -z \"$tp\" ] && [ -n \"$sid\" ] && tp=$(find \"$HOME/.codex/sessions\" -name \"rollout-*${sid}*.jsonl\" 2>/dev/null | head -1); { [ -n \"$tp\" ] && peat capture \"$tp\" --final-msg \"$fm\"; } 2>/dev/null || true"
          }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "in=$(cat); sid=$(printf '%s' \"$in\" | jq -r '.session_id // empty'); mk=.peat/nudged-\"$sid\"; if [ -n \"$sid\" ] && [ ! -f \"$mk\" ]; then mkdir -p .peat; touch \"$mk\"; printf %s \"{\\\"additionalContext\\\": \\\"peat is recording this session. At natural completion points \\\\u2014 a commit, a finished task \\\\u2014 deposit durable knowledge: peat obs <subject> \\\\\\\"<one-line claim>\\\\\\\" [--from seq,seq]. Read a belief trail with: peat <subject>.\\\"}\"; fi; exit 0"
          }
        ]
      }
    ]
  }
}
```

## Compaction

`PreCompact` fires before compaction replaces the context window: it
cannot consult the agent (no turn exists), so it runs a mechanical salvage
`peat capture` — idempotent upserts make the later Stop capture re-cover
the same events for free. The compactor's own summary is captured as a
`CompactSummary` event whenever the transcript is ingested, and the next
`SessionStart` (`source: compact`) nudges the agent to deposit durable
knowledge from that summary while it still recognizes it.

Notes:

- **peat failing may never break a session** — every command ends
  `|| true` or an explicit `exit 0`, and capture itself is never-fatal.
- The Stop hook reads stdin **once** and passes `last_assistant_message`
  as `--final-msg`, authoritative over transcript tail parsing. The
  empty-path guard skips capture rather than erroring.
- SessionStart writes `.peat/current-session` so `peat obs` (run by the
  agent mid-session, which has no session id in its environment) resolves
  the session without `--session`. `.peat/` self-ignores.
- Hook-invoked briefs carry no task words (the session hasn't started);
  `peat brief <words>` remains available for manual use.

## Keeping hooks current

Hooks are installed by copying, so upgrading the peat binary never updates
them. The contract:

- The snippets in this file are canonical and carry a version stamp
  (`hooks snippet vN · date`). Releases that change them say **Hooks:** in
  the CHANGELOG, with what changed and what to re-copy.
- After `peat` upgrades, if the CHANGELOG mentions **Hooks:** since your
  last sync, re-copy the affected blocks. There is no automation,
  deliberately: hook config often carries per-project edits (`PEAT_DB`
  anchors, `READY` gates), and a blind overwrite would eat them.
- **Fire, don't read, after any hook edit**: pipe a synthetic stdin JSON
  through the command and check both branches — e.g.
  `printf '{"tool_name":"Bash","tool_input":{"command":"git commit -m x"}}' | sh -c "<command>"`
  must print an `additionalContext` JSON object, and a non-commit input
  must print nothing and exit 0. A hook path that quietly stops resolving
  (a moved binary, a stale absolute path) is invisible at runtime by
  design — firing the hook is the only check that sees it.

## The moment-coverage matrix

Every moment a session can produce or lose knowledge, and the hook that
covers it:

| moment | hook | mechanical | judged |
|---|---|---|---|
| session begins | `SessionStart` | brief injected as context; session id written | — |
| first user prompt | `UserPromptSubmit` | — | invisible `additionalContext` nudge: deposit at natural completion points (once per session) |
| a commit lands | `PostToolUse` (Bash: `git commit`/`jj describe`/`just land`) | — | nudge: deposit an obs |
| every stop | `Stop` | capture (`--final-msg` authoritative) | — (never blocks) |
| context about to compact | `PreCompact` | salvage capture | — |
| session resumes after compact | `SessionStart` (`source: compact`) | brief | nudge: deposit from the compact summary |
| `/clear` or other non-Stop ending | `SessionEnd` | salvage capture | — (the session is gone) |

Two behaviors worth knowing:

- **peat never blocks.** A blocking Stop hook — exit 2 or
  `decision:block` — renders as a `hook error`, force-continues the turn,
  and displaces the reply the user was reading: it is enforcement
  machinery, wrong for soliciting judgment. Every peat prompt travels as
  `additionalContext` (invisible to the user, weighed by the agent in
  flow): the commit nudge, the post-compact nudge, and the
  once-per-session deposit reminder at the first user prompt. If the user
  can notice a hook, it is the wrong channel.
- **Codex parity, verified at 0.148.** `HookEventName` includes
  `postToolUse` and the shell tool serializes canonical
  `tool_name: "Bash"`, so matchers and stdin contracts are identical; the
  snippets above differ only in the Stop rollout-path fallback. One gap
  remains: Codex's `SessionStart` `source` has no `compact` value, so the
  post-compact nudge is dormant there; `PreCompact` salvage still covers
  the mechanical half.

## Worktree desks

Write a `.peat/redirect` in each desk (one line, the anchor's `.peat`
relative to the desk root — e.g. `../murail/.peat`) so bare `peat` typed by
a human resolves to the shared ledger. Hooks keep using explicit `PEAT_DB`:
re-add `PEAT_DB=<anchor>/db` before each `peat` call, gate each command
with `[ -f <anchor>/READY ] || exit 0;` (the kill switch), and make marker
paths anchor-absolute. The `.peat/` dir self-ignores (peat writes
`.peat/.gitignore` with `*`), so jj never snapshots the redirect or the
markers into a desk commit.

## Obs guidance (append to the project's CLAUDE.md/AGENTS.md)

```markdown
### peat observations

At commit points and task completions, deposit one-line observations:

    peat obs <subject> "<claim>" [--from seq,seq]

An observation is read months later, by an agent on another desk, with zero
shared context. The test: would that reader know what to do differently?

- **State a timeless rule, not a story or a status.** The incident is
  already in the ledger (cite it: --from); deployment state belongs in
  beads and commits, where it is expected to rot.
- **No deixis**: never "tonight / just now / this session / the reviewer" —
  the timestamp is recorded; prose references to *now* rot immediately.
- **Findable names**: commands, paths, repo vocabulary — never episode
  names ("the v3 rebuild", "the fix").
- **One claim per obs**; repetition on a subject is how support accrues.
- Check `peat subjects` before naming a new subject.

Bad (real, deposited by peat's own author):
    "struck twice same evening: the v3 murail rebuild also ran a pre-Said
     binary; idempotent re-capture healed it silently — build -p peat
     before any fleet-facing run"
Good (real, deposited by a herald agent):
    "A precedent set in a zero-row domain can be actively wrong in a
     populated one: deterministic id derivation was correct for seat and
     sandbox at 0 existing rows and would have orphaned memory's 2297 —
     check every carried-forward pattern against the population it is
     about to meet."
```
