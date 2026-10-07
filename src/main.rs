//! peat — agent memory as a fold.
//!
//! Agents deposit events (mechanical session exhaust + small observations);
//! every readable surface is a materialized fold view. Hooks run `capture`
//! at every session boundary (Stop, PreCompact, SessionEnd) and inject
//! `brief` stdout at SessionStart; the agent-facing surface is bare
//! `peat` (orient), `peat <thing>` (look closer, dispatched by shape),
//! and `peat obs` (the one judgment step).
//!
//! Wall-clock time is read only at the capture/render boundary (obs
//! timestamps, brief age labels) — never inside any fold path, so the
//! ledger stays deterministically replayable.
//!
//! Crate layout: `event` (the ledger schema — the API to our past),
//! `pipeline` (the fold graph), `transcript` (Claude Code JSONL -> events,
//! plus the civil-date math), `db` (path/session policy and the
//! open-with-retry), `brief` (assembly and rendering), `ui` (terminal
//! presentation). This file is CLI dispatch.

pub mod brief;
pub mod db;
pub mod distill;
pub mod event;
mod hook;
pub mod ladder;
pub mod pipeline;
pub mod transcript;
pub mod ui;

use std::path::PathBuf;

use clap::Parser;

use event::{Basis, DISTILL_SEQ_BASE, Envelope, Event, EventId, Lane, OBS_SEQ_BASE};
use pipeline::{DAY_MS, DayStats, DistRow, DistStats, ObsRow, SessStats, SubjStats, dist_key};
use ui::{age_label, clip, short_sess};

/// The day table pulled into memory for read-time joins (staleness of
/// anchored claims); bounded by the number of days in the history.
type DayCommits = std::collections::BTreeMap<u64, DayStats>;

/// Shared row filters for the verbs that walk indexed text or the ledger.
#[derive(clap::Args, Default)]
struct Filter {
    /// Only this session (prefix ok)
    #[arg(long)]
    session: Option<String>,
    /// Only this kind: obs | said | user | final | compact | tool | file |
    /// commit | meta | compacted
    #[arg(long)]
    kind: Option<String>,
    /// Only events from the last N days
    #[arg(long)]
    since: Option<u64>,
}

impl Filter {
    fn cutoff_ms(&self, now: u64) -> Option<u64> {
        self.since.map(|d| now.saturating_sub(d * DAY_MS))
    }

    fn matches(&self, now: u64, session: &str, kind: &str, ts_ms: u64) -> bool {
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
    about = "agent memory as a fold",
    long_about = "agent memory as a fold\n\n\
Sessions deposit events into one append-forever ledger; every readable\n\
surface is an incrementally maintained view over it. Bare `peat` orients;\n\
`peat <thing>` looks closer, inferring what you mean from its shape; and\n\
every line of every read ends in the exact command that goes one level\n\
deeper.",
    after_help = "READING BY SHAPE:\n  \
peat                       the brief (what hooks inject at session start)\n  \
peat 2026-w33              a window: w33, 2026-07, 2026-08-14, q3, 2026, a..b\n  \
peat 36f96b8d [seq]        a session by id prefix; one event with seq\n  \
peat fm18                  a subject's full evidence trail (exact name)\n  \
peat fold hnsw fix         anything else: hybrid keyword+semantic search\n\n\
DEPOSITING:\n  \
peat obs staging \"deploys go through the blue env first\" --from 1042\n\n\
Subcommands are the explicit spellings of the same reads; hooks use them.",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// What to look at — see READING BY SHAPE below (empty: the brief)
    query: Vec<String>,
    /// Full structured output (the API form; never clipped, never styled)
    #[arg(long, global = true)]
    json: bool,
    /// How many summary lines the brief compresses the older past into
    /// (whole history always covered; default 8, or PEAT_BRIEF_BUDGET)
    #[arg(long)]
    budget: Option<usize>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(clap::Subcommand)]
enum Cmd {
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
    /// Write memory from what the harness already wrote: for each closed
    /// day, one model call turns the person's words, compaction summaries,
    /// closing messages and commits into a paragraph plus any standing
    /// rulings (each citing the person's message); weeks, months and years
    /// merge the days. Kept in `.peat/memory`, never in the ledger. Hooks
    /// run it at session start (`touch .peat/distill-off` pauses that)
    Distill {
        /// Only days in the last N days (default: all history)
        #[arg(long)]
        since: Option<u64>,
        /// Model calls this run may spend; the rest waits for the next
        #[arg(long, default_value_t = 24)]
        limit: usize,
        /// Show what is stale; call no model
        #[arg(long)]
        dry_run: bool,
        /// The form hooks run: silent, and skipped where distilling is paused
        #[arg(long, hide = true)]
        sweep: bool,
        #[arg(long)]
        json: bool,
    },
    /// The user's standing instructions, newest first, each with the
    /// message it restates (`--all` includes withdrawn and superseded)
    Rulings {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
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
    /// The orientation `peat` prints bare: digest, temporal ladder, last
    /// session, beliefs (SessionStart hooks inject its stdout as context)
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
    /// The one command every harness hook runs. Reads the hook's stdin
    /// JSON, dispatches on hook_event_name, never fails the session:
    /// SessionStart → brief (+ session id); UserPromptSubmit → one deposit
    /// nudge per session; PostToolUse → nudge after a commit; Stop /
    /// PreCompact / SessionEnd → detached capture
    /// Print the peat skill (the judgment half: when to read, how to
    /// deposit, how far to trust a line). `peat hook install` lays it
    /// beside the hooks where the harness discovers it
    Skill,
    Hook {
        /// Override the event (default: stdin's hook_event_name)
        #[arg(long)]
        event: Option<String>,
        #[command(subcommand)]
        cmd: Option<HookCmd>,
    },
}

#[derive(clap::Subcommand)]
enum HookCmd {
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

/// `peat hook install`: the desk root is the directory holding `.peat`.
fn hook_install(sub: HookCmd) {
    let HookCmd::Install {
        codex,
        local,
        check,
        print,
    } = sub;
    let fabric = if codex {
        hook::Fabric::Codex
    } else if local {
        hook::Fabric::ClaudeLocal
    } else {
        hook::Fabric::Claude
    };
    if print {
        println!(
            "{}",
            serde_json::to_string_pretty(&hook::snippet()).unwrap()
        );
        return;
    }
    let root = db::peat_dir()
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    if !check {
        // the hooks are a no-op until a ledger exists here: create it
        migrate_views(&db::db_path());
        drop(db::open(db::db_path(), || peat_pipeline!()));
    }
    let skill = hook::install_skill(&root, fabric, check);
    match hook::install(&root, fabric, check).and_then(|h| skill.map(|s| (h, s))) {
        Ok((line, skill_line)) => {
            println!("{line}");
            println!("{skill_line}");
            if !check {
                println!("  {} hook on: {}", fabric.name(), hook::EVENTS.join(", "));
                if codex {
                    println!(
                        "  Codex runs no hook until it is trusted: open /hooks in the Codex CLI and trust it"
                    );
                }
            }
        }
        Err(e) => {
            ui::error(&e);
            std::process::exit(1);
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// One snapshot -> assembled brief. A macro because the reader tuple's
/// type contains closures and cannot be named. `$dist` is the memory
/// sidecar's rows, read beforehand ([`memory_rows`]).
macro_rules! make_brief {
    ($st:expr, $query:expr, $now:expr, $budget:expr, $dist:expr) => {
        $st.rtx(
            |(days, files, (kw, vec, texts), (subjects, evidence), sessions, _ledger)| {
                $crate::brief::assemble(
                    $query,
                    $now,
                    $budget,
                    &days,
                    &files,
                    |q, n| kw.search(q, n),
                    |v| vec.search(v),
                    |id| texts.get(id),
                    &subjects,
                    &evidence,
                    &sessions,
                    $dist,
                    $crate::db::peat_dir()
                        .parent()
                        .unwrap_or(std::path::Path::new("/")),
                )
            },
        )
    };
}

/// Everything in the memory sidecar's trail. Read before the ledger is
/// opened, and with a short lock wait: a reader that finds the sidecar
/// busy gets no digests rather than stalling (or, under a hook, losing the
/// whole brief while it holds the ledger). The sidecar is never created
/// by a read.
fn memory_trail() -> Vec<DistRow> {
    let path = db::memory_path();
    if !path.is_dir() {
        return Vec::new();
    }
    let Some(st) = db::try_open(path, || memory_pipeline!(), 3) else {
        return Vec::new();
    };
    st.rtx(|(distilled, trail)| {
        let keys: Vec<(String, DistStats)> = distilled.iter().collect();
        keys.into_iter()
            .flat_map(|(k, _)| trail.get(&k) as Vec<DistRow>)
            .collect()
    })
}

/// The newest row per key — as of `cutoff` when given (for `asof`).
fn memory_rows(trail: &[DistRow], cutoff: Option<u64>) -> Vec<DistRow> {
    let mut heads: std::collections::BTreeMap<String, &DistRow> = Default::default();
    for r in trail.iter().filter(|r| cutoff.is_none_or(|c| r.ts_ms <= c)) {
        let k = dist_key(r.lane, &r.key);
        if heads
            .get(&k)
            .is_none_or(|h| (r.ts_ms, r.seq) >= (h.ts_ms, h.seq))
        {
            heads.insert(k, r);
        }
    }
    heads.into_values().cloned().collect()
}

/// `peat distill`: gather the closed days' leaves, release the ledger,
/// let the model work, and open the memory sidecar only to deposit — no
/// store is held across a model call.
fn run_distill(since: Option<u64>, limit: usize, dry_run: bool, sweep: bool, json: bool) {
    let dbp = db::db_path();
    if sweep && (distill::paused(&dbp) || !dbp.is_dir()) {
        return;
    }
    let Some(lock) = distill::RunLock::take(&dbp) else {
        if !sweep {
            ui::note("another distill run holds this ledger; try again when it finishes");
        }
        return;
    };
    migrate_views(&dbp);
    let now = now_ms();
    let offset = transcript::local_offset_ms();
    let memp = db::memory_path();
    // a sweep starts where the last complete run stopped (less a margin
    // for days that were still open then); a manual run reads --since
    let mark = if sweep { distill::load_mark(&memp) } else { 0 };
    let from = since
        .map(|d| now.saturating_sub(d * DAY_MS))
        .unwrap_or(0)
        .max(mark.saturating_sub(2 * DAY_MS));
    let Some((days, said, newest)) = read_leaves(&dbp, from, mark, offset) else {
        return; // nothing new since the last sweep
    };
    let cmd = distill::command(&dbp);
    let opts = distill::Opts {
        today: distill::local_day(now, offset),
        limit,
        by: distill::by_label(&cmd),
        dry_run,
    };
    let snap = distill::Snapshot {
        days,
        said,
        dist: memory_rows(&memory_trail(), None)
            .into_iter()
            .map(|r| (dist_key(r.lane, &r.key), r))
            .collect(),
    };
    let phase = (!sweep && !dry_run).then(|| ui::Phase::new("distilling"));
    let model = |system: &str, user: &str| distill::call_model(&cmd, system, user);
    let mut deposit = |deps: &[distill::Deposit]| {
        lock.beat();
        let mut st = db::open(memp.clone(), || memory_pipeline!());
        st.wtx(|tx| {
            for d in deps {
                let mut seq = DISTILL_SEQ_BASE;
                while tx.contains(&(d.session.clone(), seq)) {
                    seq += 1;
                }
                tx.upsert(
                    &(d.session.clone(), seq),
                    &Envelope::memory(&d.session, d.ts_ms, d.event.clone()),
                );
            }
        });
    };
    let rep = distill::run(snap, &opts, &model, &mut deposit);
    // fold the sidecar's journal once per run that wrote: it is small, and
    // an unfolded journal is replayed by every brief that opens it
    if rep.days + rep.windows + rep.rulings > 0 {
        let mut st = db::open(memp.clone(), || memory_pipeline!());
        st.checkpoint();
    }
    if !dry_run && rep.pending == 0 && rep.failures.is_empty() {
        distill::save_mark(&memp, newest);
    }
    if let Some(p) = phase {
        p.done();
    }
    if sweep {
        return;
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rep).unwrap());
        return;
    }
    for f in &rep.failures {
        ui::error(&format!("distill: {f}"));
    }
    if dry_run {
        ui::note(&format!(
            "{} days and windows are stale; `peat distill` writes them",
            rep.pending
        ));
        return;
    }
    ui::note(&format!(
        "distilled {} days, {} windows · {} rulings · {} model calls ({})",
        rep.days, rep.windows, rep.rulings, rep.calls, opts.by
    ));
    if rep.uncited_dropped > 0 {
        ui::note(&format!(
            "{} rulings discarded: no message of the person's carries their words",
            rep.uncited_dropped
        ));
        for d in &rep.dropped {
            eprintln!("    {}", ui::dim(&clip(d, 160)));
        }
    }
    if rep.pending > 0 {
        ui::note(&format!(
            "{} more are stale — run `peat distill` again (or raise --limit)",
            rep.pending
        ));
    }
}

/// The days a distill run considers, read from the ledger before any model
/// is called: each local day's leaves from `from` on, everything the
/// person said in that stretch (for citations), and the newest session
/// end. `None` when no session has grown past `mark` — the sweep's way to
/// skip the scan entirely.
#[allow(clippy::type_complexity)]
fn read_leaves(
    dbp: &std::path::Path,
    from: u64,
    mark: u64,
    offset: i64,
) -> Option<(
    std::collections::BTreeMap<String, distill::DayLeaves>,
    Vec<(EventId, u64, String)>,
    u64,
)> {
    let st = db::open(dbp.to_path_buf(), || peat_pipeline!());
    st.rtx(|(_, _, _, _, sessions, ledger)| {
        let sessions: Vec<(String, SessStats)> = sessions.iter().collect();
        let newest = sessions.iter().map(|(_, s)| s.end_ms).max().unwrap_or(0);
        if mark > 0 && newest <= mark {
            return None;
        }
        let desks = sessions
            .iter()
            .map(|(id, s)| (id.clone(), s.cwd.clone()))
            .collect();
        let days = distill::gather(
            ledger
                .iter()
                .filter(|(_, e): &(EventId, Envelope)| e.ts_ms >= from),
            &desks,
            offset,
        );
        let said = days
            .values()
            .flat_map(|d| d.values())
            .flat_map(|s| s.said.iter().cloned())
            .collect();
        Some((days, said, newest))
    })
}

/// `peat hook` on a prompt: search memory for it and inject what both
/// lanes agree on. Bounded and silent — two seconds for the lock, no
/// view rebuild, and on any miss the prompt goes through untouched.
fn run_push(session: &str, prompt: &str) {
    let dbp = db::db_path();
    let current = std::fs::read_to_string(beside(&dbp, ".view-version"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    if current != Some(VIEW_VERSION) {
        return;
    }
    let _ = db::LOCK_WAIT_SECS.set(2);
    let _ = db::LOCK_FAIL_FALLBACK.set(String::new());
    // each line is pushed once per session: repeating it buys nothing
    let seen_path = db::peat_dir().join(format!("pushed-{session}"));
    let seen: std::collections::HashSet<EventId> = std::fs::read_to_string(&seen_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (s, q) = l.rsplit_once(' ')?;
            Some((s.to_string(), q.parse().ok()?))
        })
        .collect();
    let st = db::open(dbp, || peat_pipeline!());
    let hits = st.rtx(|(_, _, (kw, vec, texts), _, _, _)| {
        brief::push_hits(
            prompt,
            session,
            |q, n| kw.search(q, n),
            |v| vec.search(v),
            |id| texts.get(id),
            &seen,
            3,
        )
    });
    drop(st);
    if hits.is_empty() {
        return;
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&seen_path)
    {
        for (id, _) in &hits {
            let _ = writeln!(f, "{} {}", id.0, id.1);
        }
    }
    println!(
        "{}",
        hook::nudge_json("UserPromptSubmit", &brief::push_text(&hits, now_ms()))
    );
}

/// The command that reads a subject's full trail: bare `peat <name>` when
/// the name is one shell-safe token, the explicit flag form otherwise.
pub fn subject_handle_pub(name: &str) -> String {
    subject_handle(name)
}

fn subject_handle(name: &str) -> String {
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        format!("▸ peat {name}")
    } else {
        format!("▸ peat recall --subject {name:?}")
    }
}

/// Repo state at deposit time, stamped by construction — a convention the
/// depositor must remember is a check that gets deleted. Never fatal:
/// `None` outside a repo or without git. In a colocated jj repo, HEAD is
/// `@-` (the last named commit) — stable, unlike the working-copy commit.
fn deposit_basis() -> Option<Basis> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let commit = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if commit.is_empty() {
        return None;
    }
    // tracked files only: untracked noise (scratch dirs, data dirs that
    // self-ignore late) must not brand every deposit dirty
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain", "-uno"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    Some(Basis { commit, dirty })
}

/// The brief's band budget: flag beats env beats default.
fn band_budget(flag: Option<usize>) -> usize {
    flag.or_else(|| {
        std::env::var("PEAT_BRIEF_BUDGET")
            .ok()
            .and_then(|v| v.parse().ok())
    })
    .unwrap_or(8)
}

/// Bumped when any view row changes shape (view rows are postcard and
/// positional, same as the ledger — but views are disposable). On mismatch
/// the ledger mirror, whose envelopes parse forever, is replayed through
/// the current pipeline into a fresh database that replaces the old one.
///
/// History: 1 = v0.1.x shapes (implicit — no marker file);
/// 2 = `ObsRow`/`SubjStats` carry the deposit-time `Basis`.
/// Checkpointing rotates the memtable and waits for the flush — and that
/// flush rewrites index structures sized by the whole corpus, not by the
/// delta just captured. On a large shared ledger it is tens of megabytes
/// and minutes, spent inside whatever hook triggered it: a Stop capture of
/// three new events would sit in `rotate_memtable_and_wait` while the user
/// watched a frozen session (observed at 326 MB / ~67k events: >7 min for a
/// 15-line delta).
///
/// So pay it when there is bulk work to justify it, or when the journal has
/// grown enough that the next open would replay it anyway — and otherwise
/// let the journal absorb the write. The tradeoff is deliberate: an
/// un-checkpointed capture is durable across process exit (the journal is
/// on disk and replayed on open) but not across power loss, and `peat
/// compact` folds it on demand.
const CHECKPOINT_MIN_EVENTS: usize = 500;
const CHECKPOINT_JOURNAL_BYTES: u64 = 48 << 20;

fn should_checkpoint(events: usize, journal_bytes: u64) -> bool {
    events >= CHECKPOINT_MIN_EVENTS || journal_bytes >= CHECKPOINT_JOURNAL_BYTES
}

/// Bytes of unfolded journal beside the database (`*.jnl`).
fn journal_bytes(db: &std::path::Path) -> u64 {
    std::fs::read_dir(db)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "jnl"))
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Rows in the vector lane above which the exact scan is past its design
/// point and the store-resident graph (spec 2026-09-02, B′) is due. Measured
/// growth puts this 18+ months out; a 10× fabric reaches it within a year.
const FLAT_LANE_TRIP_WIRE: usize = 100_000;

const VIEW_VERSION: u32 = 2;

/// A path beside the db dir: `.peat/db` → `.peat/db<suffix>`.
fn beside(db: &std::path::Path, suffix: &str) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    db.with_file_name(name)
}

/// Rebuild every view from the ledger when the view schema has moved on.
/// Runs before the main open; self-healing by construction — hook-driven
/// invocations must never be the ones that fail. The previous database is
/// kept at `db.old` (one generation), never deleted: the ledger is the
/// investment, and this is the code path that touches it.
fn migrate_views(dbdir: &std::path::Path) {
    let marker = beside(dbdir, ".view-version");
    let on_disk: Option<u32> = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|s| s.trim().parse().ok());
    if on_disk == Some(VIEW_VERSION) {
        return;
    }
    if !dbdir.exists() {
        // fresh database: no rows exist in any shape yet
        let _ = std::fs::write(&marker, format!("{VIEW_VERSION}\n"));
        return;
    }
    let phase = ui::Phase::new("view schema changed — rebuilding views from the ledger");
    // reading only the ledger mirror is safe under any view schema:
    // envelopes are additive-only, and nothing here reads a view row
    let events: Vec<(EventId, Envelope)> = {
        let st = db::open(dbdir.to_path_buf(), || peat_pipeline!());
        st.rtx(|(_, _, _, _, _, ledger)| ledger.iter().collect())
    };
    let fresh_dir = beside(dbdir, ".rebuild");
    let _ = std::fs::remove_dir_all(&fresh_dir);
    {
        let mut fresh = db::open(fresh_dir.clone(), || peat_pipeline!());
        fresh.wtx(|tx| {
            for (id, e) in &events {
                tx.upsert(id, e);
            }
        });
        fresh.checkpoint();
    }
    let backup = beside(dbdir, ".old");
    let _ = std::fs::remove_dir_all(&backup);
    let swap = std::fs::rename(dbdir, &backup).and_then(|()| std::fs::rename(&fresh_dir, dbdir));
    if let Err(e) = swap {
        ui::error(&format!(
            "view rebuild could not swap databases at {}: {e} — the \
original is intact (possibly at {})",
            dbdir.display(),
            backup.display()
        ));
        std::process::exit(1);
    }
    let _ = std::fs::write(&marker, format!("{VIEW_VERSION}\n"));
    phase.done();
    if events.is_empty() {
        ui::note(
            "views rebuilt from an empty ledger mirror — if this db held \
sessions, re-run `peat capture` on their transcripts to backfill",
        );
    } else {
        ui::note(&format!(
            "view schema v{VIEW_VERSION}: rebuilt from {} ledger events \
(previous db kept at {})",
            events.len(),
            backup.display()
        ));
    }
}

fn main() {
    let cli = Cli::parse();

    // hooks run under a harness deadline and mostly need no ledger at
    // all; handle them before the open, and cap the wait when they do
    let cli = match cli.cmd {
        Some(Cmd::Hook { cmd: Some(sub), .. }) => {
            hook_install(sub);
            return;
        }
        Some(Cmd::Skill) => {
            print!("{}", hook::SKILL);
            return;
        }
        Some(Cmd::Hook { event, cmd: None }) => match hook::run(event) {
            hook::Action::Brief => {
                let _ = db::LOCK_WAIT_SECS.set(15);
                Cli {
                    query: vec![],
                    json: false,
                    budget: None,
                    cmd: Some(Cmd::Brief {
                        task: vec![],
                        json: false,
                        budget: None,
                    }),
                }
            }
            hook::Action::Push { session, prompt } => {
                run_push(&session, &prompt);
                return;
            }
            _ => return,
        },
        Some(Cmd::Distill {
            since,
            limit,
            dry_run,
            sweep,
            json,
        }) => {
            run_distill(since, limit, dry_run, sweep, json);
            return;
        }
        _ => cli,
    };

    migrate_views(&db::db_path());
    // the memory sidecar is read first, so no read ever holds the ledger
    // while it waits on the sidecar's lock
    let reads_memory = matches!(
        cli.cmd,
        None | Some(Cmd::Brief { .. } | Cmd::Zoom { .. } | Cmd::Asof { .. } | Cmd::Rulings { .. })
    );
    let trail = if reads_memory {
        memory_trail()
    } else {
        Vec::new()
    };
    let mut st = db::open(db::db_path(), || peat_pipeline!());

    // ---- shape dispatch: bare `peat` orients; `peat <thing>` looks
    // closer, inferring window / session / search from the argument's
    // shape. The header of every non-obvious read names its
    // interpretation, and explicit subcommands remain the unambiguous
    // spellings of the same reads.
    let cmd = match cli.cmd {
        Some(c) => c,
        None if cli.query.is_empty() => Cmd::Brief {
            task: vec![],
            json: cli.json,
            budget: cli.budget,
        },
        None => {
            let q = &cli.query;
            let now_bucket = now_ms() / DAY_MS;
            let is_sess =
                |s: &str| s.len() >= 6 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
            if q.len() == 1 && ladder::parse_window(&q[0], now_bucket).is_some() {
                Cmd::Zoom {
                    window: q[0].clone(),
                    json: cli.json,
                }
            } else if is_sess(&q[0])
                && (q.len() == 1 || (q.len() == 2 && q[1].parse::<u32>().is_ok()))
            {
                Cmd::Show {
                    session: q[0].clone(),
                    seq: q.get(1).and_then(|s| s.parse().ok()),
                }
            } else if q.len() == 1
                && st.rtx(|(_, _, _, (subjects, _), _, _)| subjects.get(&q[0]).is_some())
            {
                // an exact subject name reads its full evidence trail —
                // the expansion path every clipped belief line points at
                Cmd::Recall {
                    query: vec![],
                    limit: 12,
                    filter: Default::default(),
                    subject: Some(q[0].clone()),
                    json: cli.json,
                }
            } else {
                let words = q.join(" ");
                if !cli.json {
                    println!("{}", ui::h1(&format!("== search: {words:?} ==")));
                }
                Cmd::Recall {
                    query: q.clone(),
                    limit: 12,
                    filter: Default::default(),
                    subject: None,
                    json: cli.json,
                }
            }
        }
    };

    match cmd {
        Cmd::Capture {
            transcript,
            session,
            final_msg,
        } => {
            let Ok(jsonl) = std::fs::read_to_string(&transcript) else {
                ui::error(&format!("cannot read {}", transcript.display()));
                std::process::exit(1);
            };
            // capture cursor: transcripts are append-only and seq is a pure
            // function of the raw line index, so the already-ingested prefix
            // is skipped without JSON-parsing it — hooks re-capture on every
            // stop, and a long session must not pay O(whole transcript) each
            // time. Backed off by an overlap margin (tool_use/result pairs
            // can straddle the boundary); idempotent upserts absorb overlap.
            // A missing or stale-low cursor degrades to a full parse.
            const CURSOR_MARGIN: usize = 200;
            let cursor_path = transcript.file_stem().map(|stem| {
                db::db_path()
                    .parent()
                    .unwrap_or(&db::db_path())
                    .join("cursor")
                    .join(stem)
            });
            let skip = cursor_path
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(0)
                .saturating_sub(CURSOR_MARGIN);
            let total_lines = jsonl.lines().count();
            let Some(mut parsed) = transcript::parse_from(&jsonl, session.as_deref(), skip) else {
                ui::error("no session id found; pass --session");
                std::process::exit(1);
            };
            if let Some(text) = final_msg.filter(|t| !t.trim().is_empty()) {
                transcript::override_final_msg(&mut parsed, &text);
            }
            let n = parsed.events.len();
            let phase = ui::Phase::new(&format!("capturing {n} events"));
            st.wtx(|tx| {
                for (id, env) in &parsed.events {
                    tx.upsert(id, env);
                }
            });
            // fsync + fold the journal into the LSM — but only when the work
            // justifies the stall it costs (see `should_checkpoint`); after a
            // bulk backfill this is what keeps later opens from replaying the
            // whole journal.
            if should_checkpoint(n, journal_bytes(&db::db_path())) {
                st.checkpoint();
            }
            if let Some(p) = &cursor_path {
                let _ = std::fs::create_dir_all(p.parent().unwrap());
                let _ = std::fs::write(p, total_lines.to_string());
            }
            phase.done();
            ui::note(&format!(
                "captured {n} events from session {}",
                parsed.session
            ));
        }

        Cmd::Obs {
            subject,
            text,
            from,
            session,
            at,
        } => {
            let ts = match &at {
                None => now_ms(),
                Some(d) => match transcript::local_day_ms(d, "12:00:00.000") {
                    Some(ts) => ts,
                    None => {
                        ui::error(&format!("bad --at date {d:?}; expected YYYY-MM-DD"));
                        std::process::exit(1);
                    }
                },
            };
            let Some(session) = db::current_session(session) else {
                ui::error(
                    "no session id (.peat/current-session missing here and \
beside the shared db); pass --session",
                );
                std::process::exit(1);
            };
            let text = text.join(" ");
            let env = Envelope::new(
                &session,
                ts,
                Event::Obs2 {
                    subject: subject.clone(),
                    text: text.clone(),
                    derived_from: from,
                    basis: deposit_basis(),
                },
            );
            // Advisory lint, never blocking: observations are read months
            // later by agents with zero shared context, so phrasing that
            // leans on the present moment quietly rots. The envelope
            // already carries the timestamp; the ledger already holds the
            // story (cite it with --from).
            let text_l = text.to_lowercase();
            for (marker, why) in [
                ("tonight", "time-deictic"),
                ("today", "time-deictic"),
                ("yesterday", "time-deictic"),
                ("this session", "session-deictic"),
                ("this evening", "time-deictic"),
                ("this morning", "time-deictic"),
                ("just now", "time-deictic"),
                ("earlier", "time-deictic"),
                ("the reviewer", "person-deictic"),
                ("this change", "change-deictic"),
                ("this fix", "change-deictic"),
                ("in flight", "status-log"),
                ("now live", "status-log"),
                ("mirrors ", "status-log"),
                ("mirrored to", "status-log"),
                ("shipped", "status-log"),
                ("landed", "status-log"),
            ] {
                if text_l.contains(marker) {
                    ui::note(&format!(
                        "style: {why:?} phrase {marker:?} — future readers lack this context; state the rule standalone (the timestamp is recorded, the story is citable via --from)"
                    ));
                }
            }

            // one transaction: hint, seq scan, insert, and count all see
            // the same state (and the count sees our own write)
            let count = st.wtx(|tx| {
                let near: Vec<String> = tx.rtx(|(_, _, _, (subjects, _), _, _)| {
                    subjects
                        .iter()
                        .filter(|(s, _): &(String, SubjStats)| {
                            s != &subject && (s.contains(&subject) || subject.contains(s.as_str()))
                        })
                        .map(|(s, v)| format!("{s} ({} obs)", v.count))
                        .take(4)
                        .collect()
                });
                if !near.is_empty() {
                    ui::note(&format!("near subjects: {}", near.join(" · ")));
                }
                let mut seq = OBS_SEQ_BASE;
                while tx.contains(&(session.clone(), seq)) {
                    seq += 1;
                }
                tx.upsert(&(session.clone(), seq), &env);
                tx.rtx(|(_, _, _, (subjects, _), _, _)| {
                    subjects
                        .get(&subject)
                        .map(|s: SubjStats| s.count)
                        .unwrap_or(1)
                })
            });
            // no checkpoint here: one observation is a few journal bytes,
            // and per-command rotation emits one-row SSTs across every
            // keyspace (L0 shredding — fjall stalls writes at 20 L0 runs).
            // Bulk capture checkpoints; the journal absorbs single events.
            if text.chars().count() > 240 {
                ui::note(
                    "long for a claim — briefs clip at ~120 chars (trails read whole); \
consider splitting into separate observations",
                );
            }
            ui::note(&format!("recorded → {subject} (support {count})"));
        }

        Cmd::Brief { task, json, budget } => {
            let dist = memory_rows(&trail, None);
            let brief = make_brief!(st, &task.join(" "), now_ms(), band_budget(budget), &dist);
            brief::emit(&brief, json);
        }

        Cmd::Hook { .. } | Cmd::Skill | Cmd::Distill { .. } => {
            unreachable!("handled before the ledger opens")
        }

        Cmd::Rulings { all, json } => {
            let lane = Lane::Ruling;
            let now = now_ms();
            // current rows by default; --all reads the whole trail, so a
            // withdrawn or superseded ruling is still on the record
            let heads = memory_rows(&trail, None);
            let is_head = |r: &DistRow| {
                heads
                    .iter()
                    .any(|h| (h.session.as_str(), h.seq) == (r.session.as_str(), r.seq))
            };
            let mut rows: Vec<(DistRow, bool)> = trail
                .iter()
                .filter(|r| r.lane == lane)
                .map(|r| (r.clone(), is_head(r)))
                .filter(|(r, head)| all || (*head && r.open))
                .collect();
            rows.sort_by_key(|(r, _)| std::cmp::Reverse((r.ts_ms, r.seq)));
            if json {
                let v: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|(r, head)| {
                        serde_json::json!({
                            "key": r.key, "text": r.text, "open": r.open, "current": head,
                            "age": age_label(now, r.ts_ms), "by": r.by,
                            "session": r.session, "seq": r.seq,
                            "cites": r.cites.iter().map(|(s, q)| serde_json::json!({"session": s, "seq": q})).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            } else if rows.is_empty() {
                println!(
                    "{}",
                    ui::dim(&format!(
                        "no {}s yet (`peat distill` writes them)",
                        lane.tag()
                    ))
                );
            } else {
                for (r, head) in &rows {
                    let state = match (r.open, head) {
                        (true, true) => "",
                        (true, false) => ", superseded",
                        (false, _) => ", withdrawn",
                    };
                    let handle = match r.cites.first() {
                        Some((s, q)) => format!("▸ peat {} {q}", short_sess(s)),
                        None => format!("▸ peat {}", short_sess(&r.session)),
                    };
                    println!(
                        "  {} {} {}  {}",
                        ui::accent(&r.key),
                        ui::dim(&format!(
                            "({}{state}, by {}):",
                            age_label(now, r.ts_ms),
                            r.by
                        )),
                        r.text,
                        ui::dim(&handle)
                    );
                }
            }
        }

        Cmd::Recall {
            query,
            limit,
            filter,
            subject,
            json,
        } => {
            let now = now_ms();
            if let Some(subj) = subject {
                // the claims register read: current text plus the full
                // evidence trail, straight from the evidence multimap
                let (head, mut rows, days) = st.rtx(|(days, _, _, (subjects, evidence), _, _)| {
                    (
                        subjects.get(&subj) as Option<SubjStats>,
                        evidence.get(&subj) as Vec<ObsRow>,
                        days.iter().collect::<DayCommits>(),
                    )
                });
                rows.sort_by_key(|r| std::cmp::Reverse(r.ts_ms));
                print_subject(&subj, head, &rows, &days, now, json);
                return;
            }
            let query = query.join(" ");
            if query.trim().is_empty() {
                ui::error("recall needs a query (or --subject)");
                std::process::exit(1);
            }

            /// One recall hit; `--json` serializes it verbatim.
            #[derive(serde::Serialize)]
            struct Hit {
                score: f64,
                kind: String,
                cited: bool,
                age: String,
                session: String,
                seq: u32,
                /// rendered anchor disposition (`@abc1234+ · ~4 commits
                /// since`); obs hits only, and only when anchored
                basis: Option<String>,
                text: String,
            }
            let (hits, lane) = st.rtx(|(days, _, (kw, vec, texts), _, _, ledger)| {
                let day_commits: DayCommits = days.iter().collect();
                // the vector lane is an exact scan; this count is the
                // trip-wire for building the store-resident graph (spec
                // 2026-09-02). It appears where the cost is felt, nowhere else.
                let lane = vec.len();
                let hits: Vec<Hit> = brief::rrf(
                    &kw.search(&query, limit * 2),
                    &vec.search(&ese::encode_single(&query)),
                )
                .into_iter()
                .filter_map(|(id, score)| {
                    let t = texts.get(&id)?;
                    if !filter.matches(now, &id.0, &t.kind, t.ts_ms) {
                        return None;
                    }
                    // the anchor lives on the envelope, not the text row:
                    // a point-read per obs hit, bounded by `limit`
                    let basis = (t.kind == "obs")
                        .then(|| ledger.get(&id))
                        .flatten()
                        .and_then(|e: Envelope| match e.kind {
                            Event::Obs2 { basis, .. } => basis,
                            _ => None,
                        })
                        .map(|b| b.label_with(pipeline::commits_since(&day_commits, t.ts_ms)));
                    Some(Hit {
                        score: (score * 1000.0).round() / 1000.0,
                        kind: t.kind,
                        cited: t.cited,
                        age: age_label(now, t.ts_ms),
                        session: short_sess(&id.0),
                        seq: id.1,
                        basis,
                        text: t.text,
                    })
                })
                .take(limit)
                .collect();
                (hits, lane)
            });
            if lane > FLAT_LANE_TRIP_WIRE {
                ui::note(&format!(
                    "vector lane holds {lane} rows — past the exact scan's design point; \
time to build the store-resident graph (.design/2026-09-02-vector-index-flat-then-graph-in-store.md)"
                ));
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&hits).unwrap());
            } else if hits.is_empty() {
                println!("{}", ui::dim(&format!("no hits for {query:?}")));
            } else {
                for h in &hits {
                    let cited = if h.kind == "obs" && h.cited {
                        "·cited"
                    } else {
                        ""
                    };
                    println!(
                        "  {} {} {}",
                        ui::dim(&format!(
                            "[{}{} · {}{}]",
                            h.kind,
                            cited,
                            h.age,
                            match &h.basis {
                                Some(b) => format!(" · {b}"),
                                None => String::new(),
                            }
                        )),
                        clip(&h.text, 200),
                        ui::dim(&format!("▸ peat {} {}", h.session, h.seq)),
                    );
                }
            }
        }

        Cmd::Events { filter, json } => {
            let now = now_ms();
            let mut rows: Vec<(EventId, Envelope)> = st.rtx(|(_, _, _, _, _, ledger)| {
                ledger
                    .iter()
                    .filter(|((sess, _), e): &(EventId, Envelope)| {
                        filter.matches(now, sess, e.kind.tag(), e.ts_ms)
                    })
                    .collect()
            });
            // table iteration is (session, seq) key order; the ledger view
            // is chronological
            rows.sort_unstable_by(|a, b| (a.1.ts_ms, &a.0).cmp(&(b.1.ts_ms, &b.0)));
            if json {
                for ((sess, seq), e) in &rows {
                    println!(
                        "{}",
                        serde_json::json!({
                            "session": sess, "seq": seq, "ts_ms": e.ts_ms,
                            "v": e.v, "event": &e.kind,
                        })
                    );
                }
                return;
            }
            use std::fmt::Write;
            let mut out = String::with_capacity(rows.len() * 96);
            for ((sess, seq), e) in &rows {
                let _ = writeln!(
                    out,
                    "{} {} {:>10}  {:<7} {}",
                    ui::dim(&transcript::date_label(e.ts_ms)),
                    ui::dim(&short_sess(sess)),
                    ui::dim(&seq.to_string()),
                    ui::accent(e.kind.tag()),
                    e.kind.summary(),
                );
            }
            out.push_str(&ui::dim(&format!("({} events)\n", rows.len())));
            ui::page(&out);
        }

        Cmd::Subjects { json } => {
            let now = now_ms();
            let (mut subj, days): (Vec<(String, SubjStats)>, DayCommits) =
                st.rtx(|(days, _, _, (subjects, _), _, _)| {
                    (subjects.iter().collect(), days.iter().collect())
                });
            subj.sort_by_key(|(_, s)| std::cmp::Reverse(s.last_ms));
            if json {
                let rows: Vec<serde_json::Value> = subj
                    .iter()
                    .map(|(name, s)| {
                        serde_json::json!({
                            "subject": name, "support": s.count, "cited": s.cited,
                            "age": age_label(now, s.last_ms), "text": s.text,
                            "basis": s.basis.as_ref().map(|b| serde_json::json!({
                                "commit": b.commit, "dirty": b.dirty,
                                "commits_since": pipeline::commits_since(&days, s.last_ms),
                            })),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows).unwrap());
            } else if subj.is_empty() {
                println!("{}", ui::dim("no subjects yet"));
            } else {
                for (name, s) in subj {
                    println!(
                        "  {} {} {}  {}",
                        ui::accent(&name),
                        ui::dim(&format!(
                            "({} obs{}, {}{}):",
                            s.count,
                            if s.cited { "" } else { ", uncited" },
                            age_label(now, s.last_ms),
                            match &s.basis {
                                Some(b) => format!(
                                    ", {}",
                                    b.label_with(pipeline::commits_since(&days, s.last_ms))
                                ),
                                None => String::new(),
                            }
                        )),
                        clip(&s.text, 120),
                        ui::dim(&subject_handle(&name))
                    );
                }
            }
        }

        Cmd::Show { session, seq } => {
            let now = now_ms();
            let Some(seq) = seq else {
                // no seq: one session's overview — summary row, its
                // observations, and the handle to its raw events
                let found = st.rtx(|(_, _, _, (subjects, evidence), sessions, _ledger)| {
                    let hit: Option<(String, SessStats)> = sessions
                        .iter()
                        .find(|(sess, _): &(String, SessStats)| sess.starts_with(session.as_str()));
                    let mut obs: Vec<(String, ObsRow)> = Vec::new();
                    if let Some((sess, _)) = &hit {
                        for (name, _) in subjects.iter().collect::<Vec<(String, SubjStats)>>() {
                            for r in evidence.get(&name) {
                                if r.session == *sess {
                                    obs.push((name.clone(), r));
                                }
                            }
                        }
                    }
                    obs.sort_by_key(|(_, r)| r.ts_ms);
                    (hit, obs)
                });
                let (Some((sess, s)), obs) = found else {
                    ui::error(&format!("no session matching {session}*"));
                    std::process::exit(1);
                };
                let place = s.cwd.rsplit('/').next().unwrap_or(&s.cwd);
                // branch is noise when it restates the worktree name
                let branch = if !s.branch.is_empty()
                    && !s.branch.ends_with(place)
                    && !place.ends_with(s.branch.as_str())
                {
                    format!(" · {}", s.branch)
                } else {
                    String::new()
                };
                println!(
                    "{} {}",
                    ui::h1(&format!("== session {} ==", short_sess(&sess))),
                    ui::dim(&format!(
                        "{place}{branch} · active {} – {} ago · {} commits",
                        age_label(now, s.start_ms),
                        age_label(now, s.end_ms),
                        s.commits
                    ))
                );
                if !s.final_msg.is_empty() {
                    println!(
                        "\n{}\n  {}",
                        ui::h1("closing message:"),
                        clip(&s.final_msg, 400)
                    );
                }
                if !obs.is_empty() {
                    println!("\n{}", ui::h1("observations:"));
                    for (subj, r) in &obs {
                        println!(
                            "  {} {}",
                            ui::dim(&format!(
                                "[{subj} · {}{}]",
                                age_label(now, r.ts_ms),
                                if r.derived_from.is_empty() {
                                    ""
                                } else {
                                    " · cited"
                                }
                            )),
                            r.text
                        );
                    }
                }
                println!(
                    "\n{}",
                    ui::dim(&format!("▸ peat events --session {}", short_sess(&sess)))
                );
                return;
            };
            let (hit, citing) = st.rtx(|(_, _, _, (subjects, evidence), sessions, ledger)| {
                // resolve the session prefix against the small sessions
                // table, then point-read the ledger — never scan it
                let hit: Option<(EventId, Envelope)> = sessions
                    .iter()
                    .map(|(sess, _): (String, SessStats)| sess)
                    .find(|sess| sess.starts_with(session.as_str()))
                    .and_then(|sess| {
                        let id = (sess, seq);
                        ledger.get(&id).map(|e: Envelope| (id, e))
                    });
                // the citer walk is only worth paying on a hit, and only
                // matching rows earn a subject-name clone
                let mut citing: Vec<(String, ObsRow)> = Vec::new();
                if let Some(((sess, _), _)) = &hit {
                    for (name, _) in subjects.iter().collect::<Vec<(String, SubjStats)>>() {
                        for r in evidence.get(&name) {
                            if r.session == *sess && r.derived_from.contains(&seq) {
                                citing.push((name.clone(), r));
                            }
                        }
                    }
                }
                (hit, citing)
            });
            match hit {
                None => {
                    ui::error(&format!("no event ({session}*, {seq})"));
                    std::process::exit(1);
                }
                Some(((sess, q), env)) => {
                    println!(
                        "{} {}",
                        ui::h1(&format!("event ({sess}, {q})")),
                        ui::dim(&format!("· {} · v{}", age_label(now, env.ts_ms), env.v))
                    );
                    println!("{}", serde_json::to_string_pretty(&env.kind).unwrap());
                    for (subj, r) in citing {
                        println!(
                            "{} {}",
                            ui::dim(&format!(
                                "cited by obs [{subj} · {}]:",
                                age_label(now, r.ts_ms)
                            )),
                            r.text
                        );
                    }
                }
            }
        }

        Cmd::Zoom { window, json } => {
            let now = now_ms();
            let now_bucket = now / DAY_MS;
            let Some((start, end, label)) = ladder::parse_window(&window, now_bucket) else {
                ui::error(&format!(
                    "not a window: {window:?} (want w33, 2026-w33, 2026-07, 2026-08-14, q3, 2026, or a..b)"
                ));
                std::process::exit(1);
            };
            let lo_ms = start * DAY_MS;
            let hi_ms = (end + 1) * DAY_MS;
            // the distilled paragraphs: this window's and its children's
            let digests: std::collections::BTreeMap<String, DistRow> = memory_rows(&trail, None)
                .into_iter()
                .filter(|r| r.lane == Lane::Digest)
                .map(|r| (r.key.clone(), r))
                .collect();
            let (head, kids, sess_rows, finals, obs) =
                st.rtx(|(days, _, _, (subjects, evidence), sessions, ledger)| {
                    let day_rows: std::collections::BTreeMap<u64, pipeline::DayStats> =
                        days.iter().collect();
                    let mut obs_per_day: std::collections::BTreeMap<u64, i64> = Default::default();
                    let mut obs_rows: Vec<(String, ObsRow)> = Vec::new();
                    for (name, _) in subjects.iter().collect::<Vec<(String, SubjStats)>>() {
                        for r in evidence.get(&name) {
                            *obs_per_day.entry(r.ts_ms / DAY_MS).or_default() += 1;
                            if (lo_ms..hi_ms).contains(&r.ts_ms) {
                                obs_rows.push((name.clone(), r));
                            }
                        }
                    }
                    obs_rows.sort_by_key(|(_, r)| r.ts_ms);
                    let head = ladder::digest(
                        &day_rows,
                        &obs_per_day,
                        start,
                        end,
                        label.clone(),
                        String::new(),
                        String::new(),
                    );
                    let kids: Vec<ladder::Band> = ladder::children(start, end)
                        .into_iter()
                        .map(|(s, e, l, h)| {
                            ladder::digest(&day_rows, &obs_per_day, s, e, l, String::new(), h)
                        })
                        .filter(|b| b.tools + b.commits + b.obs > 0)
                        .collect();
                    // sessions overlapping the window, newest first
                    let mut sess_rows: Vec<(String, SessStats)> = sessions
                        .iter()
                        .filter(|(_, s): &(String, SessStats)| {
                            s.start_ms < hi_ms && s.end_ms >= lo_ms
                        })
                        .collect();
                    sess_rows.sort_by_key(|(_, s)| std::cmp::Reverse(s.end_ms));
                    // lane B from the ledger mirror: what was said, verbatim
                    let mut finals: Vec<(EventId, u64, String)> = Vec::new();
                    for (id, e) in ledger.iter().collect::<Vec<(EventId, Envelope)>>() {
                        if !(lo_ms..hi_ms).contains(&e.ts_ms) {
                            continue;
                        }
                        match &e.kind {
                            Event::FinalMsg { text } | Event::CompactSummary { text } => {
                                finals.push((id, e.ts_ms, text.clone()));
                            }
                            _ => {}
                        }
                    }
                    finals.sort_by_key(|(_, ts, _)| std::cmp::Reverse(*ts));
                    finals.truncate(4);
                    (head, kids, sess_rows, finals, obs_rows)
                });
            let mut head = head;
            head.digest = ladder::window_key(&window, start, end)
                .and_then(|k| digests.get(&k))
                .map(|r| r.text.clone());
            let mut kids = kids;
            for b in &mut kids {
                b.digest = b
                    .handle
                    .strip_prefix("peat ")
                    .and_then(|k| digests.get(k))
                    .map(|r| r.text.clone());
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "window": label, "start_day": start, "end_day": end,
                        "digest": head, "children": kids,
                        "sessions": sess_rows.iter().map(|(id, s)| serde_json::json!({
                            "session": short_sess(id), "cwd": s.cwd, "branch": s.branch,
                            "commits": s.commits, "start_ms": s.start_ms, "end_ms": s.end_ms,
                        })).collect::<Vec<_>>(),
                        "said": finals.iter().map(|(id, ts, t)| serde_json::json!({
                            "session": short_sess(&id.0), "seq": id.1, "ts_ms": ts, "text": t,
                        })).collect::<Vec<_>>(),
                        "observations": obs.iter().map(|(subj, r)| serde_json::json!({
                            "subject": subj, "session": short_sess(&r.session), "seq": r.seq,
                            "cited": !r.derived_from.is_empty(), "text": r.text,
                        })).collect::<Vec<_>>(),
                    }))
                    .unwrap()
                );
                return;
            }
            let fails = if head.fails > 0 {
                format!(" ({} fail)", head.fails)
            } else {
                String::new()
            };
            let span = {
                let s = ladder::Civil::from_bucket(start);
                let e = ladder::Civil::from_bucket(end);
                ladder::span_label(s, e)
            };
            println!(
                "{} {}",
                ui::h1(&format!(
                    "== {label}{} ==",
                    if label.contains(' ') || span.is_empty() {
                        String::new()
                    } else {
                        format!(" · {span}")
                    }
                )),
                ui::dim(&format!(
                    "{} tools{fails} · {} commits · {} sessions{}",
                    ui::knum(head.tools),
                    head.commits,
                    sess_rows.len(),
                    if head.obs > 0 {
                        format!(" · {} obs", head.obs)
                    } else {
                        String::new()
                    }
                ))
            );
            if let Some(d) = &head.digest {
                println!("\n  {d}");
            }
            if !kids.is_empty() {
                println!("\n{}", ui::h1("within:"));
                for b in &kids {
                    let f = if b.fails > 0 {
                        format!(" ({} fail)", b.fails)
                    } else {
                        String::new()
                    };
                    let files = if b.files.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", b.files.join(", "))
                    };
                    let (said, files) = match &b.digest {
                        Some(d) => (format!("{d} {} ", ui::dim("·")), String::new()),
                        None => (String::new(), files),
                    };
                    println!(
                        "  {} {said}{} tools{f} · {} commits{}{files}  {}",
                        ui::dim(&format!("[{}]", b.label)),
                        ui::knum(b.tools),
                        b.commits,
                        if b.obs > 0 {
                            format!(" · {} obs", b.obs)
                        } else {
                            String::new()
                        },
                        ui::dim(&format!("▸ {}", b.handle)),
                    );
                }
            }
            if start == end && !sess_rows.is_empty() {
                println!("\n{}", ui::h1("sessions:"));
                for (id, s) in &sess_rows {
                    let place = s.cwd.rsplit('/').next().unwrap_or(&s.cwd);
                    println!(
                        "  {} {place} · {} commits  {}",
                        ui::dim(&format!("[{}]", short_sess(id))),
                        s.commits,
                        ui::dim(&format!("▸ peat {}", short_sess(id))),
                    );
                }
            }
            if !finals.is_empty() {
                println!("\n{}", ui::h1("closing messages:"));
                for (id, ts, t) in &finals {
                    println!(
                        "  {} {}",
                        ui::dim(&format!(
                            "[{} · {}]",
                            short_sess(&id.0),
                            age_label(now, *ts)
                        )),
                        clip(t, 180)
                    );
                }
            }
            if !obs.is_empty() {
                println!("\n{}", ui::h1("observations:"));
                for (subj, r) in obs.iter().rev().take(8) {
                    println!(
                        "  {} {}",
                        ui::dim(&format!(
                            "[{subj} · {}{}]",
                            age_label(now, r.ts_ms),
                            if r.derived_from.is_empty() {
                                ""
                            } else {
                                " · cited"
                            }
                        )),
                        clip(&r.text, 160)
                    );
                }
            }
        }

        Cmd::Compact => {
            let before = journal_bytes(&db::db_path());
            let phase = ui::Phase::new(&format!(
                "folding {} MB of journal into the LSM",
                before / (1 << 20)
            ));
            st.checkpoint();
            phase.done();
            ui::note(&format!(
                "compacted — journal {} MB → {} MB",
                before / (1 << 20),
                journal_bytes(&db::db_path()) / (1 << 20)
            ));
        }

        Cmd::Asof { date, task, json } => {
            // end of DATE in the caller's local day, not UTC — the fold
            // never sees timezones; this is the render/capture boundary
            let Some(cutoff) = transcript::local_day_ms(&date, "23:59:59.999") else {
                ui::error(&format!("bad date {date:?}; expected YYYY-MM-DD"));
                std::process::exit(1);
            };
            // the ledger mirror is what makes this possible: read every
            // event at-or-before the cutoff...
            let events: Vec<(EventId, Envelope)> = st.rtx(|(_, _, _, _, _, ledger)| {
                ledger
                    .iter()
                    .filter(|(_, e): &(EventId, Envelope)| e.ts_ms <= cutoff)
                    .collect()
            });
            drop(st);
            // ...and fold that prefix through the SAME pipeline into a
            // scratch database. Determinism (oracle 2) is what makes the
            // result the truth of that day rather than a reconstruction.
            let scratch = std::env::temp_dir().join(format!("peat-asof-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&scratch);
            let phase = ui::Phase::new(&format!("replaying {} events to {date}", events.len()));
            let mut past = db::open(scratch.clone(), || peat_pipeline!());
            past.wtx(|tx| {
                for (id, e) in &events {
                    tx.upsert(id, e);
                }
            });
            phase.done();
            let dist = memory_rows(&trail, Some(cutoff));
            let mut brief = make_brief!(past, &task.join(" "), cutoff, band_budget(None), &dist);
            brief.today = format!("{date} · as of that day · {} events", events.len());
            if events.is_empty() {
                ui::note(&format!(
                    "no events at or before {date} — either this ledger's \
history starts later, or the db predates the ledger mirror \
(re-run `peat capture` on the transcripts to backfill it)"
                ));
            }
            brief::emit(&brief, json);
            drop(past);
            let _ = std::fs::remove_dir_all(&scratch);
        }
    }
}

/// Print one subject's evidence trail (`recall --subject`).
fn print_subject(
    subj: &str,
    head: Option<SubjStats>,
    rows: &[ObsRow],
    days: &DayCommits,
    now: u64,
    json: bool,
) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "subject": subj, "current": head.as_ref().map(|h| &h.text),
                "support": head.as_ref().map(|h| h.count),
                "evidence": rows.iter().map(|r| serde_json::json!({
                    "session": r.session, "seq": r.seq,
                    "age": age_label(now, r.ts_ms),
                    "cited": !r.derived_from.is_empty(),
                    "basis": r.basis.as_ref().map(|b| serde_json::json!({
                        "commit": b.commit, "dirty": b.dirty,
                        "commits_since": pipeline::commits_since(days, r.ts_ms),
                    })),
                    "text": r.text,
                })).collect::<Vec<_>>(),
            }))
            .unwrap()
        );
        return;
    }
    if rows.is_empty() {
        println!("{}", ui::dim(&format!("no such subject: {subj}")));
        return;
    }
    if let Some(h) = head {
        println!(
            "{} — {} {}",
            ui::accent(subj),
            h.text,
            ui::dim(&format!("({} obs)", h.count))
        );
    }
    for r in rows {
        println!(
            "  {} {}",
            ui::dim(&format!(
                "[{} · {}{}{}]",
                short_sess(&r.session),
                age_label(now, r.ts_ms),
                if r.derived_from.is_empty() {
                    ""
                } else {
                    " · cited"
                },
                match &r.basis {
                    Some(b) => format!(
                        " · {}",
                        b.label_with(pipeline::commits_since(days, r.ts_ms))
                    ),
                    None => String::new(),
                }
            )),
            r.text
        );
    }
}

#[cfg(test)]
mod tests;
