//! Where the ledger lives and how it is opened: path policy, session-id
//! policy, the shared-database open-with-retry, view migration, and the
//! checkpoint policy.

use std::path::PathBuf;

use fold::pipeline::{Keyed, Push};
use fold::stream::KeyedStream;

use crate::event::{Envelope, EventId};
use crate::ui;

/// The database: `$PEAT_DB` if set, else `.peat/db` at the repo root —
/// unless `.peat/redirect` names another `.peat` directory (one line,
/// resolved relative to the repo root), in which case the ledger lives
/// there. The beads convention, borrowed: a worktree desk redirects to its
/// anchor so every seat reads and writes one shared memory, while
/// desk-local files (`current-session`, the once-per-session markers)
/// stay beside the redirect.
///
/// A git worktree or secondary jj workspace with no ledger and no
/// redirect of its own resolves to its anchor repo's ledger automatically
/// (when that one exists), so a desk Claude Code or jj created on the fly
/// remembers into the same mind without any per-desk setup.
pub fn db_path() -> PathBuf {
    resolve(false)
}

/// Where `peat init` puts the ledger: what [`db_path`] resolves to once
/// the ledger exists. The same rules, except that a worktree or secondary
/// workspace names its anchor's ledger even before that one exists, so
/// opting a desk in opts its whole repo in.
pub fn init_path() -> PathBuf {
    resolve(true)
}

fn resolve(anchor_always: bool) -> PathBuf {
    if let Ok(p) = std::env::var("PEAT_DB") {
        return PathBuf::from(p);
    }
    let dir = peat_dir();
    if let Ok(target) = std::fs::read_to_string(dir.join("redirect")) {
        let target = target.trim();
        if !target.is_empty() {
            let base = dir.parent().map(PathBuf::from).unwrap_or_default();
            return base.join(target).join("db");
        }
    }
    let local = dir.join("db");
    if !local.is_dir()
        && let Some(anchor) = worktree_anchor(dir.parent().unwrap_or(&dir))
    {
        let shared = anchor.join(".peat").join("db");
        if anchor_always || shared.is_dir() {
            return shared;
        }
    }
    local
}

/// The main repository root of a git worktree (`.git` is a file naming
/// `<main>/.git/worktrees/<name>`) or a secondary jj workspace
/// (`.jj/repo` is a file naming `<main>/.jj/repo`). `None` for the main
/// checkout itself, or outside any repo.
pub fn worktree_anchor(root: &std::path::Path) -> Option<PathBuf> {
    let git = root.join(".git");
    if git.is_file() {
        let text = std::fs::read_to_string(&git).ok()?;
        let target = text.trim().strip_prefix("gitdir:")?.trim();
        let target = root.join(target);
        // …/<main>/.git/worktrees/<name>
        let main = target.parent()?.parent()?.parent()?;
        return Some(main.to_path_buf());
    }
    let jj = root.join(".jj").join("repo");
    if jj.is_file() {
        let text = std::fs::read_to_string(&jj).ok()?;
        let target = root.join(".jj").join(text.trim());
        // …/<main>/.jj/repo
        let main = target.parent()?.parent()?;
        return Some(normalize(main));
    }
    None
}

/// Lexically resolve `..` components (canonicalize would follow symlinks
/// and fail on a missing path; neither is wanted here).
fn normalize(p: &std::path::Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Create a peat data dir that ignores itself (cargo's `target/`
/// pattern): without its `.gitignore`, jj snapshots whatever lands in it
/// into the working commit on the very next command, and git accumulates
/// it as untracked noise.
pub fn ensure_ignored_dir(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let marker = dir.join(".gitignore");
    if !marker.exists() {
        let _ = std::fs::write(&marker, "*\n");
    }
}

/// Exit with `code` — unless a hook is running (it set
/// [`LOCK_FAIL_FALLBACK`]), which must never fail a session: it prints
/// its fallback line and exits 0 whatever went wrong.
pub fn bail(code: i32) -> ! {
    if let Some(fallback) = LOCK_FAIL_FALLBACK.get() {
        if !fallback.is_empty() {
            ui::out(&format!("{fallback}\n"));
        }
        std::process::exit(0);
    }
    std::process::exit(code)
}

/// A process-wide cap on the lock wait, set by callers that run under a
/// harness deadline (`peat hook`). Wins over `PEAT_LOCK_WAIT_SECS`.
pub static LOCK_WAIT_SECS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

/// Set by hook paths that must never fail a session: on a lock timeout
/// this is printed to stdout (it may be empty) and the process exits 0.
pub static LOCK_FAIL_FALLBACK: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// `.peat/` beside the nearest git/jj root above cwd, else cwd. Cached —
/// callers hit this several times per invocation and the answer is fixed.
pub fn peat_dir() -> PathBuf {
    static C: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        let mut dir = std::env::current_dir().unwrap();
        loop {
            if dir.join(".git").exists() || dir.join(".jj").exists() {
                return dir.join(".peat");
            }
            if !dir.pop() {
                return std::env::current_dir().unwrap().join(".peat");
            }
        }
    })
    .clone()
}

/// Resolve the current session id: an explicit value wins, then this
/// worktree's `.peat/current-session`, then the file beside the shared db
/// (a desk is not the anchor — hooks may have written it there).
pub fn current_session(explicit: Option<String>) -> Option<String> {
    explicit
        .or_else(|| std::fs::read_to_string(peat_dir().join("current-session")).ok())
        .or_else(|| {
            let anchor = db_path().parent()?.join("current-session");
            std::fs::read_to_string(anchor).ok()
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Variables Codex sets on each process it runs (seen in the codex 0.163
/// binary). User-level settings such as `CODEX_HOME` are not evidence: a
/// person may export them in every shell, Claude sessions included.
const CODEX_PROCESS_VARS: &[&str] = &[
    "CODEX_THREAD_ID",
    "CODEX_TOOL_CALL_ID",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
];

/// The session running this process, on exact evidence only: the id the
/// harness put in our environment. `current-session` is not evidence —
/// several seats share it and the last writer wins. A variable Codex sets
/// on the processes it runs ([`CODEX_PROCESS_VARS`]) means we run under
/// Codex, where an inherited `CLAUDE_CODE_SESSION_ID` (a codex started from
/// a Claude pane) is never trusted; `CODEX_THREAD_ID` (its rollout id) is
/// the id when present. `vars` is the environment, so tests pass pairs
/// instead of mutating the process.
pub fn calling_session(vars: impl IntoIterator<Item = (String, String)>) -> Option<String> {
    let (mut codex, mut thread, mut claude) = (false, None, None);
    for (k, v) in vars {
        let v = Some(v.trim().to_string()).filter(|v| !v.is_empty());
        match k.as_str() {
            "CODEX_THREAD_ID" => thread = v,
            "CLAUDE_CODE_SESSION_ID" => claude = v,
            _ => {}
        }
        codex |= CODEX_PROCESS_VARS.contains(&k.as_str());
    }
    if codex { thread } else { claude }
}

/// [`calling_session`] over the process environment.
pub fn calling_session_env() -> Option<String> {
    // vars_os: a non-UTF-8 variable must not panic a read
    calling_session(
        std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))),
    )
}

/// Restores the process panic hook when dropped, whichever way the open
/// loop exits — the hook is muted during retries because `catch_unwind`
/// does not silence it, and a caught-and-retried lock conflict must not
/// print a backtrace.
type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

struct QuietPanics(Option<PanicHook>);

impl QuietPanics {
    fn engage() -> Self {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        QuietPanics(Some(prev))
    }
}

impl Drop for QuietPanics {
    fn drop(&mut self) {
        if let Some(hook) = self.0.take() {
            std::panic::set_hook(hook);
        }
    }
}

/// Open the ledger, waiting out lock contention.
///
/// fold is single-writer and fjall's lock is exclusive even for reads, so
/// with several worktree agents sharing one `PEAT_DB`, invocations can
/// collide; peat processes are short-lived, so waiting is correct. Retries
/// with backoff up to `PEAT_LOCK_WAIT_SECS` (default 120 — a bulk capture
/// can legitimately hold the lock for minutes), then exits `EX_TEMPFAIL`
/// with an explanation. Non-lock panics (corruption, schema) fail
/// immediately.
///
/// `make` re-creates the pipeline per attempt (the value is consumed by a
/// failed open).
pub fn open<P>(path: PathBuf, make: impl Fn() -> P) -> Ledger<P>
where
    P: Push<Keyed<EventId, Envelope>>,
{
    let wait_max_ms: u64 = LOCK_WAIT_SECS
        .get()
        .copied()
        .or_else(|| {
            std::env::var("PEAT_LOCK_WAIT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(120u64)
        * 1000;

    if let Some(parent) = path.parent() {
        ensure_ignored_dir(parent);
    }

    let phase = ui::Phase::new("opening ledger");
    let quiet = QuietPanics::engage();
    let mut delay_ms = 200u64;
    let mut waited = 0u64;
    let st = loop {
        let path = path.clone();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            KeyedStream::<EventId, Envelope, _>::new(path, make())
        })) {
            Ok(st) => break st,
            Err(p) => {
                let msg = p
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| p.downcast_ref::<&str>().copied())
                    .unwrap_or("");
                if !msg.contains("Locked") {
                    drop(quiet);
                    std::panic::resume_unwind(p);
                }
                if waited >= wait_max_ms {
                    drop(quiet);
                    if LOCK_FAIL_FALLBACK.get().is_some() {
                        bail(0);
                    }
                    ui::error(&format!(
                        "ledger still locked after {}s — another peat \
process holds it (reads are exclusive too; a bulk capture can hold it for \
minutes). Retry shortly, or raise PEAT_LOCK_WAIT_SECS.",
                        wait_max_ms / 1000
                    ));
                    bail(75); // EX_TEMPFAIL
                }
                if waited == 0 && !ui::fancy_err() && LOCK_FAIL_FALLBACK.get().is_none() {
                    // non-tty gets one plain line instead of a spinner
                    ui::eline("peat: ledger busy (another peat process); waiting…");
                }
                phase.tick(format!(
                    "ledger busy — another peat process · waiting {}s (gives up at {}s)",
                    waited / 1000,
                    wait_max_ms / 1000
                ));
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                waited += delay_ms;
                delay_ms = (delay_ms * 2).min(3_000);
            }
        }
    };
    phase.done();
    Ledger(Some(st))
}

/// The ledger with peat's own pipeline: what every command reads.
pub type Db = Ledger<crate::pipeline::Pipeline>;

/// Open peat's ledger at `path` (views migrated first).
///
/// The marker is checked again once the store's lock is held: a process
/// that passed [`migrate_views`] and then waited in [`open`] while a newer
/// peat rebuilt and swapped the views would otherwise open the swapped-in
/// database and write rows of the wrong shape into it. On a changed marker
/// it lets go and goes round again (migrate, or refuse newer views).
/// (Bounded: a marker that cannot be written — a read-only `.peat` — must
/// not spin; the third open is used as it is.)
pub fn open_db(path: PathBuf) -> Db {
    for _ in 0..2 {
        migrate_views(&path);
        let st = open(path.clone(), crate::pipeline::pipeline);
        if views(&path) == Views::Current {
            return st;
        }
    }
    migrate_views(&path);
    open(path, crate::pipeline::pipeline)
}

/// The wall clock, read only at the capture/render boundary (obs
/// timestamps, age labels, "today") — never inside a fold path.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// ---- view schema ------------------------------------------------------

/// Bumped when any view row changes shape (view rows are postcard and
/// positional, same as the ledger — but views are disposable). When the
/// on-disk views are older, the ledger mirror, whose envelopes parse
/// forever, is replayed through the current pipeline into a fresh database
/// that replaces the old one ([`migrate_views`]); when they are newer, this
/// binary refuses them.
///
/// History: 1 = v0.1.x shapes (implicit — no marker file);
/// 2 = `ObsRow`/`SubjStats` carry the deposit-time `Basis`;
/// 3 = the `days` view is keyed by quarter-hour slot, not UTC day;
/// 4 = `SessStats` carries the session digest, and user text rows index
/// the person's own words only.
pub const VIEW_VERSION: u32 = 4;

/// A path beside the db dir: `.peat/db` → `.peat/db<suffix>`.
pub fn beside(db: &std::path::Path, suffix: &str) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    db.with_file_name(name)
}

/// Where the on-disk views stand against this binary's [`VIEW_VERSION`].
#[derive(Debug, PartialEq, Eq)]
pub enum Views {
    /// The marker names this binary's version: read them.
    Current,
    /// Older (or no marker: v1 wrote none): rebuild from the ledger.
    Stale,
    /// A newer peat wrote them: this binary must neither read nor rebuild
    /// them, or two binaries sharing a ledger rebuild over each other.
    Newer(u32),
}

/// Read the view-version marker beside `dbdir`.
pub fn views(dbdir: &std::path::Path) -> Views {
    let v = std::fs::read_to_string(beside(dbdir, ".view-version"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    match v {
        Some(v) if v == VIEW_VERSION => Views::Current,
        Some(v) if v > VIEW_VERSION => Views::Newer(v),
        _ => Views::Stale,
    }
}

/// What a binary older than the views says: the hook one stderr line (it
/// then does nothing), every other command an error.
pub fn newer_views_message(v: u32) -> String {
    format!(
        "this ledger's views are v{v}, written by a newer peat; this peat \
({}) reads v{VIEW_VERSION} and will not rebuild them — upgrade peat",
        env!("CARGO_PKG_VERSION")
    )
}

/// Is this process a harness hook (it set [`LOCK_FAIL_FALLBACK`])?
fn in_hook() -> bool {
    LOCK_FAIL_FALLBACK.get().is_some()
}

/// The migration lock beside `dbdir`, opened (not yet locked).
fn migrate_lock(dbdir: &std::path::Path) -> Option<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(beside(dbdir, ".migrate.lock"))
        .ok()
}

/// Finish a view swap that a killed rebuild left between its two renames.
/// Cheap (renames only) and safe from any process, hooks included: it
/// runs only when it can take the migration lock at once — a live
/// migrator holds it and finishes its own swap. Run before anything asks
/// whether the ledger exists, since mid-swap `db` itself may be missing.
pub fn settle(dbdir: &std::path::Path) {
    if !beside(dbdir, ".swap").exists() {
        return;
    }
    let Some(lock) = migrate_lock(dbdir) else {
        return;
    };
    if lock.try_lock().is_ok() {
        let _ = finish_swap(dbdir);
    }
}

/// Is a swap pending (a migrator is between renames, or was killed there)?
pub fn swap_pending(dbdir: &std::path::Path) -> bool {
    beside(dbdir, ".swap").exists()
}

/// Swap the complete rebuild at `db.rebuild` in for `db` (kept as
/// `db.old`), then write the marker. Every step is idempotent and the
/// `db.swap` intent file is written only once the rebuild is complete
/// and closed, so a run killed anywhere in here is finished by the next
/// call — under the migration lock — from what is on disk:
///
/// - `db` and `db.rebuild` both present: nothing renamed yet.
/// - `db` absent, `db.rebuild` present: the first rename happened.
/// - `db` present, `db.rebuild` absent: both happened; only the marker is
///   missing.
///
/// The marker is written last, so a swap that has not finished is never
/// read as current views.
fn finish_swap(dbdir: &std::path::Path) -> std::io::Result<()> {
    let (fresh, backup, intent) = (
        beside(dbdir, ".rebuild"),
        beside(dbdir, ".old"),
        beside(dbdir, ".swap"),
    );
    if dbdir.exists() && fresh.exists() {
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::rename(dbdir, &backup)?;
    }
    if !dbdir.exists() {
        if fresh.exists() {
            std::fs::rename(&fresh, dbdir)?;
        } else if backup.exists() {
            // an intent with no rebuild beside it is not ours to finish:
            // put the ledger back, still stale, for a clean rebuild
            std::fs::rename(&backup, dbdir)?;
            return std::fs::remove_file(&intent);
        }
    }
    std::fs::write(beside(dbdir, ".view-version"), format!("{VIEW_VERSION}\n"))?;
    std::fs::remove_file(&intent)
}

/// Start the view rebuild in a detached `peat` (the hidden
/// `migrate-views` verb), unless one is already running (it holds the
/// migration lock). Its stderr goes to `db.rebuild.log` beside the db.
fn spawn_rebuild(dbdir: &std::path::Path) {
    if let Some(lock) = migrate_lock(dbdir) {
        if lock.try_lock().is_err() {
            return; // a migrator is at work
        }
        // released before the child starts; a second hook racing here
        // spawns a second child, which waits and finds the views current
        drop(lock);
    }
    crate::hook::detach(
        &["migrate-views".as_ref()],
        Some(&beside(dbdir, ".rebuild.log")),
    );
}

/// Bring the views up to this binary's schema before an open.
///
/// - Current: nothing.
/// - Newer: refuse. A hook says so in one stderr line and exits 0 with
///   nothing on stdout; any other command exits 1.
/// - Stale, in a hook: never rebuild inline — the rebuild of a large
///   ledger takes minutes, and a hook runs under a harness deadline that
///   would kill it and retry it on every start. Start it detached and
///   exit 0 with the fallback line ([`bail`]). Captures need nothing
///   special: the hook already runs them as detached `peat capture`
///   processes, which come through here as ordinary commands and wait for
///   the rebuild (or run it) before writing.
/// - Stale, anywhere else: rebuild now, under the migration lock (one
///   migrator at a time; the rest wait without deadline, then find the
///   views current).
///
/// The rebuild replays the ledger mirror through the current pipeline into
/// `db.rebuild` and swaps it in ([`finish_swap`]). The previous database is
/// kept at `db.old` (one generation), never deleted: the ledger is the
/// investment, and this is the code path that touches it.
pub fn migrate_views(dbdir: &std::path::Path) {
    match views(dbdir) {
        Views::Current => return,
        Views::Newer(v) => refuse_newer(v),
        Views::Stale => {}
    }
    // the marker lives beside the db: its directory must exist before a
    // fresh database's marker is written, or the next open would find no
    // marker and "rebuild" the db the first one just created
    if let Some(parent) = dbdir.parent() {
        ensure_ignored_dir(parent);
    }
    if in_hook() && (dbdir.exists() || swap_pending(dbdir)) {
        spawn_rebuild(dbdir);
        ui::eline(&format!(
            "peat: view schema changed — rebuilding the views (v{VIEW_VERSION}) in the \
background; the brief returns once that finishes"
        ));
        bail(0);
    }
    let lock = migrate_lock(dbdir);
    let _held = lock.as_ref().filter(|f| wait_for_migrator(f));
    match views(dbdir) {
        Views::Current => return,
        Views::Newer(v) => refuse_newer(v),
        Views::Stale => {}
    }
    if swap_pending(dbdir) {
        // a rebuild was killed mid-swap; its rebuild is complete
        if let Err(e) = finish_swap(dbdir) {
            swap_failed(dbdir, &e);
        }
        if views(dbdir) == Views::Current {
            return;
        }
    }
    if !dbdir.exists() {
        // fresh database: no rows exist in any shape yet
        let _ = std::fs::write(beside(dbdir, ".view-version"), format!("{VIEW_VERSION}\n"));
        return;
    }
    let phase = ui::Phase::new("view schema changed — rebuilding views from the ledger");
    let fresh_dir = beside(dbdir, ".rebuild");
    // a rebuild killed before its swap leaves a partial `db.rebuild`,
    // never read: start over
    let _ = std::fs::remove_dir_all(&fresh_dir);
    // reading only the ledger mirror is safe under any view schema:
    // envelopes are additive-only, and nothing here reads a view row.
    // The old db stays open (its store lock held) until the swap has
    // renamed it aside and the marker is written, so no process — an older
    // peat retrying its open, say — can write into a database the swap is
    // about to set aside (its capture cursor, kept outside the db, would
    // make that loss permanent). One that opens the swapped-in db after the
    // lock is released finds the new marker ([`open_db`]).
    let old = open(dbdir.to_path_buf(), crate::pipeline::pipeline);
    let events: Vec<(EventId, Envelope)> =
        old.rtx(|(_, _, _, _, _, ledger)| ledger.iter().collect());
    let n = events.len();
    {
        let mut fresh = open(fresh_dir.clone(), crate::pipeline::pipeline);
        fresh.wtx(|tx| {
            for (id, e) in &events {
                tx.upsert(id, e);
            }
        });
        fresh.checkpoint();
        retire_journal(&fresh_dir);
    }
    drop(events);
    test_delay();
    if let Err(e) = std::fs::write(beside(dbdir, ".swap"), "").and_then(|()| finish_swap(dbdir)) {
        swap_failed(dbdir, &e);
    }
    drop(old);
    phase.done();
    if n == 0 {
        ui::note(
            "views rebuilt from an empty ledger mirror — if this db held \
sessions, re-run `peat capture` on their transcripts to backfill",
        );
    } else {
        ui::note(&format!(
            "view schema v{VIEW_VERSION}: rebuilt from {n} ledger events \
(previous db kept at {})",
            beside(dbdir, ".old").display()
        ));
    }
}

/// How long a command waits for another process's view rebuild before
/// giving up. Generous: murail's rebuild takes 1–1.5 min and herald's
/// ledger is larger, under load several times that.
const MIGRATE_WAIT: std::time::Duration = std::time::Duration::from_secs(600);

/// Take the migration lock. A hook never waits ([`bail`]s at once); any
/// other command waits up to [`MIGRATE_WAIT`], then exits `EX_TEMPFAIL`.
/// A capture gives up before writing anything, cursor included, so the
/// next capture of that transcript redoes it whole — except after a
/// SessionEnd, which nothing follows: that transcript stays on disk,
/// unrecorded until it is captured again by hand.
fn wait_for_migrator(f: &std::fs::File) -> bool {
    if f.try_lock().is_ok() {
        return true;
    }
    if in_hook() {
        bail(0);
    }
    ui::eline("peat: another peat is rebuilding the views; waiting for it…");
    let wait = migrate_wait();
    let start = std::time::Instant::now();
    loop {
        match f.try_lock() {
            Ok(()) => return true,
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(_)) => return false,
        }
        if start.elapsed() >= wait {
            ui::error(&format!(
                "another peat has been rebuilding the views for over {}s; \
giving up — retry once it finishes (its log: db.rebuild.log beside the ledger)",
                wait.as_secs()
            ));
            bail(75); // EX_TEMPFAIL
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// [`MIGRATE_WAIT`]; debug builds take `PEAT_TEST_MIGRATE_WAIT_SECS`.
fn migrate_wait() -> std::time::Duration {
    #[cfg(debug_assertions)]
    if let Some(s) = std::env::var("PEAT_TEST_MIGRATE_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        return std::time::Duration::from_secs(s);
    }
    MIGRATE_WAIT
}

fn refuse_newer(v: u32) -> ! {
    if in_hook() {
        ui::eline(&format!("peat: {}", newer_views_message(v)));
        std::process::exit(0);
    }
    ui::error(&newer_views_message(v));
    std::process::exit(1)
}

fn swap_failed(dbdir: &std::path::Path, e: &std::io::Error) -> ! {
    ui::error(&format!(
        "view rebuild could not swap databases at {}: {e} — the ledger is \
intact (at {} or {}); the next peat command finishes the swap",
        dbdir.display(),
        beside(dbdir, ".old").display(),
        beside(dbdir, ".rebuild").display(),
    ));
    bail(1)
}

/// Journal files (`*.jnl`) in a database dir.
pub fn journal_files(db: &std::path::Path) -> usize {
    std::fs::read_dir(db)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "jnl"))
                .count()
        })
        .unwrap_or(0)
}

/// How long the rebuild waits for its sealed journal to be retired.
const RETIRE_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Wait, with the rebuilt database still open, until fjall deletes the
/// journal the rebuild was written through.
///
/// fjall 3.1 (read in its `journal/manager.rs` and `worker_pool.rs`): the
/// first flush of the checkpoint seals the journal the rebuild was written
/// through (the whole rebuild: 557 MB on murail's ledger), recording a
/// watermark per keyspace; journal maintenance deletes it once every one
/// of those keyspaces has persisted past its watermark, and runs after
/// each flush. Past `max_journaling_size` (512 MiB) the sealing flush
/// also asks the straggling keyspaces to rotate, and those flushes run on
/// the worker after the checkpoint has returned: on murail's ledger the
/// sealed journal was deleted about 2 s after the checkpoint. Closing
/// sooner leaves it on disk, and every later open replays it (12 s for
/// the first SessionStart after the swap). A reopen does not help: it
/// replays it again and its checkpoint, with empty memtables, rotates
/// nothing, so no maintenance runs. Waiting does; fold exposes no call
/// that waits for maintenance, and none is needed.
///
/// What remains is the one active journal, preallocated (64 MiB on disk,
/// empty), so the measure is the number of journal files, not their size.
/// On timeout the rebuild goes ahead (the journal costs time, not
/// correctness) and says so in its log.
fn retire_journal(dir: &std::path::Path) {
    let start = std::time::Instant::now();
    while journal_files(dir) > 1 {
        if start.elapsed() >= RETIRE_WAIT {
            ui::eline(&format!(
                "peat: WARNING the rebuilt views' journal was not retired within {}s \
({} journal files at {}); every open will replay it until `peat compact`",
                RETIRE_WAIT.as_secs(),
                journal_files(dir),
                dir.display()
            ));
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Debug builds only: `PEAT_TEST_REBUILD_DELAY_MS` stretches a rebuild
/// (after it is complete, before its swap, the old db still held) so
/// tests can act while one runs. Release builds carry no such knob.
fn test_delay() {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("PEAT_TEST_REBUILD_DELAY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

// ---- checkpoint policy ------------------------------------------------

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
pub const CHECKPOINT_MIN_EVENTS: usize = 500;
pub const CHECKPOINT_JOURNAL_BYTES: u64 = 48 << 20;

pub fn should_checkpoint(events: usize, journal_bytes: u64) -> bool {
    events >= CHECKPOINT_MIN_EVENTS || journal_bytes >= CHECKPOINT_JOURNAL_BYTES
}

/// Bytes of unfolded journal beside the database (`*.jnl`).
pub fn journal_bytes(db: &std::path::Path) -> u64 {
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

/// How long closing the ledger may take before peat stops waiting for it.
/// A clean close finishes the compaction a worker is running, which takes
/// seconds on a large ledger; anything past this is the hang below.
const CLOSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// The open ledger. Derefs to the store; its only addition is a bounded
/// close.
///
/// fjall 3.1.9's `DatabaseInner::drop` can hang forever: it checks that a
/// worker is alive, then makes a blocking send of `Close` on the bounded
/// worker channel; when the channel is full and the last worker exits
/// between the check and the send, nothing will ever receive. The process
/// then holds `.peat/db/lock` indefinitely and every seat sharing the
/// ledger waits behind it (seen: a detached capture parked 22 h on
/// murail's ledger). Every commit is durable once `wtx` returns, so a close
/// that overruns `CLOSE_DEADLINE` loses nothing by exiting: the watchdog
/// says so on stderr and exits 0, and the OS releases the lock.
pub struct Ledger<P: Push<Keyed<EventId, Envelope>>>(Option<KeyedStream<EventId, Envelope, P>>);

impl<P: Push<Keyed<EventId, Envelope>>> std::ops::Deref for Ledger<P> {
    type Target = KeyedStream<EventId, Envelope, P>;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("ledger is open until dropped")
    }
}

impl<P: Push<Keyed<EventId, Envelope>>> std::ops::DerefMut for Ledger<P> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("ledger is open until dropped")
    }
}

impl<P: Push<Keyed<EventId, Envelope>>> Drop for Ledger<P> {
    fn drop(&mut self) {
        let Some(store) = self.0.take() else { return };
        close_within(
            CLOSE_DEADLINE,
            move || drop(store),
            || {
                ui::eline(&format!(
                    "peat: closing the ledger hung for {}s (fjall shutdown race); \
exiting so the lock is released — every commit is already durable",
                    CLOSE_DEADLINE.as_secs()
                ));
                std::process::exit(0);
            },
        );
    }
}

/// Run `close`; if it has not returned within `deadline`, run `on_overrun`
/// from a watchdog thread (which, in the ledger's case, ends the process
/// — the only way out of a close parked forever on the main thread). A
/// close that returns in time disarms the watchdog, so a later reopen in
/// the same process is never hit by a stale one.
fn close_within(
    deadline: std::time::Duration,
    close: impl FnOnce(),
    on_overrun: impl FnOnce() + Send + 'static,
) {
    let (closed, wait) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        // a disconnect means the close returned (its sender dropped)
        if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = wait.recv_timeout(deadline) {
            on_overrun();
        }
    });
    close();
    drop(closed);
}

#[cfg(test)]
mod close_tests {
    use super::close_within;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    use std::time::Duration;

    fn overran(close_takes: Duration) -> bool {
        let fired = Arc::new(AtomicBool::new(false));
        let f = fired.clone();
        close_within(
            Duration::from_millis(100),
            || std::thread::sleep(close_takes),
            move || f.store(true, SeqCst),
        );
        // give a disarmed watchdog the chance to misfire
        std::thread::sleep(Duration::from_millis(300));
        fired.load(SeqCst)
    }

    #[test]
    fn a_close_that_hangs_trips_the_watchdog() {
        assert!(overran(Duration::from_millis(400)));
    }

    #[test]
    fn a_prompt_close_disarms_the_watchdog() {
        assert!(!overran(Duration::ZERO));
    }
}

#[cfg(test)]
mod anchor_tests {
    use super::*;

    #[test]
    fn worktree_anchor_reads_git_and_jj_pointer_files() {
        let tmp = std::env::temp_dir().join(format!("peat-anchor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let main = tmp.join("main");
        let wt = tmp.join("main").join(".claude").join("worktrees").join("x");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}/.git/worktrees/x\n", main.display()),
        )
        .unwrap();
        assert_eq!(worktree_anchor(&wt), Some(main.clone()));

        let ws = tmp.join("main-dev");
        std::fs::create_dir_all(ws.join(".jj")).unwrap();
        std::fs::write(ws.join(".jj").join("repo"), "../../main/.jj/repo\n").unwrap();
        assert_eq!(worktree_anchor(&ws), Some(main.clone()));

        std::fs::create_dir_all(main.join(".git")).unwrap();
        assert_eq!(
            worktree_anchor(&main),
            None,
            "a real checkout has no anchor"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
