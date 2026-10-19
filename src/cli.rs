//! The command line: every verb and flag, nothing else. Bodies live in
//! `cmd/`, one module per verb; `main` dispatches.

use std::path::PathBuf;

use clap::Parser;

use crate::pipeline::DAY_MS;

/// Shared row filters for the verbs that walk indexed text or the ledger.
#[derive(clap::Args, Default)]
pub struct Filter {
    /// Only this session (prefix ok)
    #[arg(long)]
    pub session: Option<String>,
    /// Only this kind: obs | said | user | final | compact | tool | file |
    /// commit | meta | compacted (commit: recorded before 0.5.0 only)
    #[arg(long)]
    pub kind: Option<String>,
    /// Only events from the last N days
    #[arg(long)]
    pub since: Option<u64>,
}

impl Filter {
    pub fn cutoff_ms(&self, now: u64) -> Option<u64> {
        self.since.map(|d| now.saturating_sub(d * DAY_MS))
    }

    pub fn matches(&self, now: u64, session: &str, kind: &str, ts_ms: u64) -> bool {
        self.session
            .as_ref()
            .is_none_or(|p| session.starts_with(p.as_str()))
            && self.kind.as_ref().is_none_or(|k| kind == k)
            && self.cutoff_ms(now).is_none_or(|c| ts_ms >= c)
    }
}

#[derive(Parser)]
#[command(
    name = "peat",
    version,
    about = "the record of what agents did, as a fold",
    long_about = "the record of what agents did, as a fold\n\n\
Hooks capture every agent session into one append-forever ledger; every\n\
readable surface is an incrementally maintained view over it. Bare `peat` orients;\n\
`peat <thing>` looks closer, inferring what you mean from its shape; and\n\
every line of every read ends in the exact command that goes one level\n\
deeper.",
    after_help = "READING BY SHAPE:\n  \
peat                       the dashboard: recent activity, the ladder, last session\n  \
peat 2026-w33              a window: w33, 2026-07, 2026-08-14, q3, 2026, a..b\n  \
peat 36f96b8d [seq]        a session by id prefix; one event with seq\n  \
peat fm18                  a subject's full evidence trail (exact name)\n  \
peat 1bdaa47b              a commit or jj change: the session that made it\n  \
peat src/hook.rs           a file: the sessions that edited it\n  \
peat peat-520              a bead id (or other token): sessions that mention it\n  \
peat fold hnsw fix         anything else: hybrid keyword+semantic search\n\n\
DEPOSITING:\n  \
peat obs staging \"deploys go through the blue env first\" --from 1042\n\n\
Subcommands are the explicit spellings of the same reads. Harness hooks run\n\
`peat hook`, which injects only a short agent brief at session start.",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    /// What to look at — see READING BY SHAPE below (empty: the brief)
    pub query: Vec<String>,
    /// Full structured output (the API form; never clipped, never styled)
    #[arg(long, global = true)]
    pub json: bool,
    /// How many summary lines the brief compresses the older past into
    /// (whole history always covered; default 8, or PEAT_BRIEF_BUDGET)
    #[arg(long)]
    pub budget: Option<usize>,
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Ingest a session transcript — Claude Code JSONL or Codex rollout,
    /// auto-detected. Idempotent: hooks re-run it at Stop, PreCompact, and
    /// SessionEnd; re-capture ingests only the delta
    Capture {
        transcript: PathBuf,
        /// Session id if the transcript doesn't carry one
        #[arg(long)]
        session: Option<String>,
        /// Authoritative closing message (the Stop hook passes
        /// `.last_assistant_message`); overrides transcript tail parsing
        #[arg(long)]
        final_msg: Option<String>,
    },
    /// Deposit one judged claim about a subject (the only writing verb
    /// an agent runs by hand)
    Obs {
        subject: String,
        /// The claim, as one short sentence
        text: Vec<String>,
        /// Seqs of captured events this observation rests on
        #[arg(long, value_delimiter = ',')]
        from: Vec<u32>,
        #[arg(long)]
        session: Option<String>,
        /// Backdate the observation to noon of this day (YYYY-MM-DD) —
        /// retroactive annotation; asof briefs for that day will carry it
        #[arg(long)]
        at: Option<String>,
    },
    /// The dashboard `peat` prints bare: active sessions, recent days, the
    /// temporal ladder, last session, touched files (for a person at a
    /// terminal; hooks inject a much shorter agent brief instead)
    Brief {
        task: Vec<String>,
        #[arg(long)]
        json: bool,
        /// How many summary lines to compress the older past into
        /// (whole history always covered; default 8, or PEAT_BRIEF_BUDGET)
        #[arg(long)]
        budget: Option<usize>,
    },
    /// Search memory (what `peat <words>` runs): hybrid keyword+semantic,
    /// each hit tagged [kind · age] and addressed
    Recall {
        query: Vec<String>,
        /// Max hits to print
        #[arg(long, default_value_t = 12)]
        limit: usize,
        #[command(flatten)]
        filter: Filter,
        /// Read one subject's full evidence trail instead of searching
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// The raw ledger floor, oldest first, auto-paged on a terminal
    Events {
        #[command(flatten)]
        filter: Filter,
        #[arg(long)]
        json: bool,
    },
    /// The claims register: every subject, newest-wins text, support
    Subjects {
        #[arg(long)]
        json: bool,
    },
    /// One session's overview (what `peat <prefix>` runs), or one event
    /// in full with its citers when seq is given
    Show {
        /// Session id (prefix ok)
        session: String,
        seq: Option<u32>,
    },
    /// One window of time (what `peat <window>` runs): digest, children
    /// one rung finer, and what was said inside it
    Zoom {
        /// w33, 2026-w33, 2026-07, 2026-08-14, q3, 2026, or a..b
        window: String,
        #[arg(long)]
        json: bool,
    },
    /// The sessions behind a commit or change id, a bead id (or any token
    /// the work mentions), or a file path, with the moment in each (what
    /// `peat <ref>` runs)
    Ref {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Fold the journal into the LSM now — the maintenance cost that
    /// captures otherwise defer. Safe to run any time; takes minutes on a
    /// large ledger, so run it when nothing is waiting on you
    Compact,
    /// Time travel: the brief as it would have read at the end of DATE.
    /// Replays the ledger prefix through the same deterministic pipeline.
    Asof {
        /// YYYY-MM-DD (cut at the end of that local calendar day)
        date: String,
        task: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print the peat skill (when to look things up, how far to trust a
    /// line). `peat hook install` lays it beside the hooks where the
    /// harness discovers it
    Skill,
    /// The one command every harness hook runs. Reads the hook's stdin
    /// JSON and dispatches on hook_event_name, never failing the session:
    /// SessionStart → the agent brief (+ session id); Stop / PreCompact /
    /// SessionEnd → detached capture
    Hook {
        /// Override the event (default: stdin's hook_event_name)
        #[arg(long)]
        event: Option<String>,
        #[command(subcommand)]
        cmd: Option<HookCmd>,
    },
}

#[derive(clap::Subcommand)]
pub enum HookCmd {
    /// Wire `peat hook` into .claude/settings.json (or .codex/hooks.json)
    /// at the desk root and lay the peat skill beside it — merges, keeps
    /// other hooks, replaces any copied peat bash snippets, idempotent
    Install {
        /// Target Codex (.codex/hooks.json) instead of Claude Code
        #[arg(long, conflicts_with = "local")]
        codex: bool,
        /// Target Claude Code's git-ignored .claude/settings.local.json
        #[arg(long)]
        local: bool,
        /// Report whether the hooks are installed; exit 1 if not
        #[arg(long)]
        check: bool,
        /// Print the snippet instead of writing it
        #[arg(long)]
        print: bool,
    },
}
