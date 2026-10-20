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
pub fn open_db(path: PathBuf) -> Db {
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
/// positional, same as the ledger — but views are disposable). On mismatch
/// the ledger mirror, whose envelopes parse forever, is replayed through
/// the current pipeline into a fresh database that replaces the old one.
///
/// History: 1 = v0.1.x shapes (implicit — no marker file);
/// 2 = `ObsRow`/`SubjStats` carry the deposit-time `Basis`;
/// 3 = the `days` view is keyed by quarter-hour slot, not UTC day.
pub const VIEW_VERSION: u32 = 3;

/// A path beside the db dir: `.peat/db` → `.peat/db<suffix>`.
pub fn beside(db: &std::path::Path, suffix: &str) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    db.with_file_name(name)
}

/// Rebuild every view from the ledger when the view schema has moved on.
/// Runs before every open; self-healing by construction — hook-driven
/// invocations must never be the ones that fail. The previous database is
/// kept at `db.old` (one generation), never deleted: the ledger is the
/// investment, and this is the code path that touches it.
pub fn migrate_views(dbdir: &std::path::Path) {
    let marker = beside(dbdir, ".view-version");
    let current = || {
        std::fs::read_to_string(&marker)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            == Some(VIEW_VERSION)
    };
    if current() {
        return;
    }
    // the marker lives beside the db: its directory must exist before a
    // fresh database's marker is written, or the next open would find no
    // marker and "rebuild" the db the first one just created
    if let Some(parent) = dbdir.parent() {
        ensure_ignored_dir(parent);
    }
    // one migrator at a time: concurrent first opens (two SessionStarts
    // right after an upgrade) would otherwise rebuild into the same
    // `.rebuild` dir and swap over each other. The second waits, then
    // finds the marker current.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(beside(dbdir, ".migrate.lock"));
    let _held = lock.as_ref().ok().filter(|f| f.lock().is_ok());
    if current() {
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
        let st = open(dbdir.to_path_buf(), crate::pipeline::pipeline);
        st.rtx(|(_, _, _, _, _, ledger)| ledger.iter().collect())
    };
    let fresh_dir = beside(dbdir, ".rebuild");
    let _ = std::fs::remove_dir_all(&fresh_dir);
    {
        let mut fresh = open(fresh_dir.clone(), crate::pipeline::pipeline);
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
        bail(1);
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
