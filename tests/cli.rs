//! The binary end to end: `peat hook install`, `peat init` and the
//! SessionStart duplicate guard, each run as a process in a temp HOME
//! (with `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `PEAT_DB` cleared), so no
//! test can read or write the real user configs.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn tmp(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("peat-cli-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `peat <args>` in `cwd` with `home` as the whole user environment.
fn peat(home: &Path, cwd: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_peat"));
    c.args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("PEAT_DB")
        .env_remove("PEAT_LOCK_WAIT_SECS");
    c
}

fn run(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    peat(home, cwd, args).output().unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn install_check_fails_on_drift_and_passes_after_install() {
    let home = tmp("check");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(home.join(".codex/config.toml"), "model = \"x\"\n").unwrap();
    let check = run(&home, &home, &["hook", "install", "--check"]);
    assert_eq!(check.status.code(), Some(1), "{}", text(&check));
    let printed = run(&home, &home, &["hook", "install", "--print"]);
    let printed = String::from_utf8(printed.stdout).unwrap();
    assert!(
        printed.contains(&format!(
            "# {}\n",
            home.join(".claude/settings.json").display()
        )) && printed.contains("[[hooks.SessionStart]]"),
        "{printed}"
    );
    assert!(!home.join(".claude").exists(), "--print writes nothing");
    let installed = run(&home, &home, &["hook", "install"]);
    assert!(installed.status.success(), "{}", text(&installed));
    let check = run(&home, &home, &["hook", "install", "--check"]);
    assert!(check.status.success(), "{}", text(&check));
    let toml = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    assert!(toml.starts_with("model = \"x\"\n"), "{toml}");
    assert_eq!(toml.matches("command = \"peat hook\"").count(), 4, "{toml}");
    let again = run(&home, &home, &["hook", "install"]);
    assert!(
        String::from_utf8_lossy(&again.stdout).contains("already current"),
        "{}",
        text(&again)
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// A copied hook file in a repo whose VCS cannot vouch for it (these
/// test repos are bare `.jj`/`.git` dirs, so `jj`/`git` answer nothing)
/// counts as tracked: reported, never edited, until `--strip-project`.
#[test]
fn tracked_repo_copies_are_reported_and_strip_project_removes_them() {
    let home = tmp("strip");
    let repo = home.join("code/r");
    std::fs::create_dir_all(repo.join(".jj")).unwrap();
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    let settings = repo.join(".claude/settings.json");
    let copy = r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"bd prime"},{"type":"command","command":"peat hook"}]}]}}"#;
    std::fs::write(&settings, copy).unwrap();
    let installed = run(&home, &repo, &["hook", "install"]);
    assert!(installed.status.success(), "{}", text(&installed));
    let out = String::from_utf8_lossy(&installed.stdout).to_string();
    assert!(out.contains("--strip-project"), "{out}");
    assert!(
        out.contains("peat init"),
        "a repo with no ledger is told how to opt in: {out}"
    );
    assert_eq!(
        std::fs::read_to_string(&settings).unwrap(),
        copy,
        "untouched"
    );
    let check = run(&home, &repo, &["hook", "install", "--check"]);
    assert!(
        check.status.success(),
        "a repo copy is a note, not drift: {}",
        text(&check)
    );
    assert!(String::from_utf8_lossy(&check.stdout).contains("fires twice"));
    let stripped = run(&home, &repo, &["hook", "install", "--strip-project"]);
    assert!(stripped.status.success(), "{}", text(&stripped));
    let left = std::fs::read_to_string(&settings).unwrap();
    assert!(
        left.contains("bd prime") && !left.contains("peat"),
        "{left}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// `~` may be a repo (`~/.git`), but its `.claude/` is the user config:
/// never stripped, never told to `peat init`.
#[test]
fn home_is_never_a_repo() {
    let home = tmp("home-repo");
    std::fs::create_dir_all(home.join(".git")).unwrap();
    let o = run(&home, &home, &["hook", "install", "--strip-project"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(
        !String::from_utf8_lossy(&o.stdout).contains("peat init"),
        "{}",
        text(&o)
    );
    let check = run(&home, &home, &["hook", "install", "--check"]);
    assert!(check.status.success(), "{}", text(&check));
    let settings = std::fs::read_to_string(home.join(".claude/settings.json")).unwrap();
    assert_eq!(settings.matches("\"peat hook\"").count(), 4, "{settings}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn project_install_is_the_old_per_repo_form() {
    let home = tmp("project");
    let repo = home.join("r");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    assert!(
        run(&home, &repo, &["hook", "install", "--codex"])
            .status
            .code()
            == Some(2),
        "--codex needs --project"
    );
    let o = run(&home, &repo, &["hook", "install", "--project"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(repo.join(".claude/settings.json").is_file());
    assert!(repo.join(".claude/skills/peat/SKILL.md").is_file());
    assert!(
        repo.join(".peat/db").is_dir(),
        "the project form creates the ledger"
    );
    assert!(
        !home.join(".claude").exists(),
        "and leaves the user configs alone"
    );
    std::fs::remove_file(repo.join(".claude/skills/peat/SKILL.md")).unwrap();
    let check = run(&home, &repo, &["hook", "install", "--project", "--check"]);
    assert!(
        check.status.success(),
        "a missing desk skill passes: {}",
        text(&check)
    );
    // an intentional per-repo setup beside the user-level hook stays green
    assert!(run(&home, &repo, &["hook", "install"]).status.success());
    let check = run(&home, &repo, &["hook", "install", "--check"]);
    assert!(check.status.success(), "{}", text(&check));
    assert!(repo.join(".claude/settings.json").is_file());
    let _ = std::fs::remove_dir_all(&home);
}

/// The skill is user-level: install reports it and never writes it,
/// `--check` stays green without it, and `peat skill --user` writes it.
#[test]
fn user_skill_is_reported_by_install_and_written_by_skill_user() {
    let home = tmp("skill");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    let o = run(&home, &home, &["hook", "install"]);
    assert!(o.status.success(), "{}", text(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("no peat skill for Claude Code")
            && out.contains("no peat skill for Codex")
            && out.contains("peat skill --user"),
        "{}",
        text(&o)
    );
    assert!(
        !home.join(".claude/skills").exists(),
        "install writes no skill"
    );
    let check = run(&home, &home, &["hook", "install", "--check"]);
    assert!(
        check.status.success(),
        "a missing skill is not drift: {}",
        text(&check)
    );
    let w = run(&home, &home, &["skill", "--user"]);
    assert!(w.status.success(), "{}", text(&w));
    let printed = run(&home, &home, &["skill"]).stdout;
    for p in [
        ".claude/skills/peat/SKILL.md",
        ".agents/skills/peat/SKILL.md",
    ] {
        assert_eq!(std::fs::read(home.join(p)).unwrap(), printed, "{p}");
    }
    let check = run(&home, &home, &["hook", "install", "--check"]);
    assert!(check.status.success(), "{}", text(&check));
    assert_eq!(
        String::from_utf8_lossy(&check.stdout)
            .matches("peat skill current")
            .count(),
        2,
        "{}",
        text(&check)
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn init_creates_the_ledger_once_and_reads_require_it() {
    let home = tmp("init");
    let repo = home.join("r");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let read = run(&home, &repo, &["subjects"]);
    assert_eq!(read.status.code(), Some(1), "{}", text(&read));
    assert!(String::from_utf8_lossy(&read.stderr).contains("peat init"));
    assert!(
        !repo.join(".peat/db").exists(),
        "a read never opts a repo in"
    );
    let first = run(&home, &repo, &["init"]);
    let out = String::from_utf8_lossy(&first.stdout).to_string();
    assert!(out.starts_with("✓ ledger created at "), "{}", text(&first));
    assert!(out.contains("peat hook install"), "no user hook yet: {out}");
    assert!(repo.join(".peat/db").is_dir());
    assert_eq!(
        std::fs::read_to_string(repo.join(".peat/.gitignore")).unwrap(),
        "*\n"
    );
    let second = run(&home, &repo, &["init"]);
    assert!(
        String::from_utf8_lossy(&second.stdout).starts_with("= ledger already at "),
        "{}",
        text(&second)
    );
    // a fresh ledger's view marker is current: the next open rebuilds nothing
    let read = run(&home, &repo, &["subjects"]);
    assert!(read.status.success(), "{}", text(&read));
    assert!(
        !String::from_utf8_lossy(&read.stderr).contains("rebuil"),
        "{}",
        text(&read)
    );
    assert!(!repo.join(".peat/db.old").exists());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn init_in_a_jj_workspace_opts_in_the_anchor() {
    let home = tmp("init-ws");
    let main = home.join("main");
    let ws = home.join("main-dev");
    std::fs::create_dir_all(main.join(".jj/repo")).unwrap();
    std::fs::create_dir_all(ws.join(".jj")).unwrap();
    std::fs::write(ws.join(".jj/repo"), "../../main/.jj/repo\n").unwrap();
    let o = run(&home, &ws, &["init"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(main.join(".peat/db").is_dir(), "{}", text(&o));
    assert!(!ws.join(".peat/db").exists());
    let _ = std::fs::remove_dir_all(&home);
}

/// Fire one SessionStart per `(session, source)`, all at once; each must
/// exit 0. Returns their stdouts.
fn session_starts(home: &Path, repo: &Path, fires: &[(&str, &str)]) -> Vec<Output> {
    let children: Vec<_> = fires
        .iter()
        .map(|(session, source)| {
            let payload = format!(
                r#"{{"hook_event_name":"SessionStart","session_id":"{session}","source":"{source}"}}"#
            );
            let mut c = peat(home, repo, &["hook"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            c.stdin
                .take()
                .unwrap()
                .write_all(payload.as_bytes())
                .unwrap();
            c
        })
        .collect();
    children
        .into_iter()
        .map(|c| {
            let o = c.wait_with_output().unwrap();
            assert!(o.status.success(), "a hook exits 0: {}", text(&o));
            o
        })
        .collect()
}

fn briefs(outs: &[Output]) -> usize {
    outs.iter().filter(|o| !o.stdout.is_empty()).count()
}

/// A user-level hook and a leftover project copy fire the same
/// SessionStart concurrently: exactly one of them briefs.
#[test]
fn twin_session_starts_brief_once() {
    let home = tmp("twins");
    let repo = home.join("r");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    assert!(run(&home, &repo, &["init"]).status.success());
    // warm: the ledger opens without any first-open work
    assert!(run(&home, &repo, &["subjects"]).status.success());
    let outs = session_starts(
        &home,
        &repo,
        &[("sess-1", "startup"), ("sess-1", "startup")],
    );
    assert_eq!(briefs(&outs), 1, "one brief, one silence: {outs:?}");
    let outs = session_starts(
        &home,
        &repo,
        &[("sess-1", "compact"), ("sess-1", "compact")],
    );
    assert_eq!(
        briefs(&outs),
        1,
        "another moment of the same session briefs again: {outs:?}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// Distinct sessions starting together right after `peat init` — the
/// ledger's very first opens — all brief and all exit 0.
#[test]
fn first_session_starts_after_init_all_brief() {
    let home = tmp("first");
    let repo = home.join("r");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    assert!(run(&home, &repo, &["init"]).status.success());
    let outs = session_starts(&home, &repo, &[("a-1", "startup"), ("b-2", "startup")]);
    assert_eq!(briefs(&outs), 2, "{outs:?}");
    let _ = std::fs::remove_dir_all(&home);
}

// ---- view migration across an upgrade -------------------------------
//
// After an upgrade every ledger's views are stale. The rebuild of a large
// ledger takes minutes (murail: 87 s), so a hook must never run it: the
// SessionStart briefs with the lookup line only and starts the rebuild
// detached; captures (already detached by the hook) wait for it and land
// in the rebuilt views. Debug builds stretch the rebuild with
// PEAT_TEST_REBUILD_DELAY_MS so the tests can act while it runs.

const REBUILD_DELAY_MS: u64 = 4_000;
const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/transcript-nx-rs-planread.jsonl"
);
const FIXTURE_SESSION: &str = "aac931de-b7aa-4efb-bcd1-b53010b4b9dd";

/// A repo with an initialised ledger holding two observations (and the
/// fixture session when `captured`), its view marker forced to `marker`.
fn stale_repo(tag: &str, marker: &str, captured: bool) -> (PathBuf, PathBuf) {
    let home = tmp(tag);
    let repo = home.join("r");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    assert!(run(&home, &repo, &["init"]).status.success());
    for (subj, claim) in [("alpha", "the first claim"), ("beta", "the second claim")] {
        let o = run(&home, &repo, &["obs", subj, claim, "--session", "s-obs"]);
        assert!(o.status.success(), "{}", text(&o));
    }
    if captured {
        let o = run(&home, &repo, &["capture", FIXTURE]);
        assert!(o.status.success(), "{}", text(&o));
    }
    std::fs::write(repo.join(".peat/db.view-version"), marker).unwrap();
    (home, repo)
}

/// Run `peat hook` with `payload` on stdin; returns its output and wall time.
fn hook_with(home: &Path, repo: &Path, payload: &str) -> (Output, std::time::Duration) {
    use std::io::Write;
    let t = std::time::Instant::now();
    let mut c = peat(home, repo, &["hook"])
        .env("PEAT_TEST_REBUILD_DELAY_MS", REBUILD_DELAY_MS.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let o = c.wait_with_output().unwrap();
    (o, t.elapsed())
}

fn start_payload(session: &str) -> String {
    format!(r#"{{"hook_event_name":"SessionStart","session_id":"{session}","source":"startup"}}"#)
}

/// A capture hook's payload for `transcript`. No closing message: two
/// captures of one session waiting behind a rebuild land in either order,
/// and only a Stop carries one, so its override would make the outcome
/// order-dependent.
fn capture_payload(event: &str, session: &str, transcript: &Path) -> String {
    format!(
        r#"{{"hook_event_name":"{event}","session_id":"{session}","transcript_path":"{}"}}"#,
        transcript.display()
    )
}

fn marker(repo: &Path) -> String {
    std::fs::read_to_string(repo.join(".peat/db.view-version"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Wait until the views are current and no migrator holds the lock.
fn await_rebuild(repo: &Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        let lock = std::fs::File::open(repo.join(".peat/db.migrate.lock")).unwrap();
        let free = lock.try_lock().is_ok();
        drop(lock);
        if free && marker(repo) == current_version() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the background rebuild never finished (marker {:?})",
            marker(repo)
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// This binary's view version, as a fresh ledger records it.
fn current_version() -> String {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        let (home, repo) = (tmp("version"), tmp("version-r"));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        assert!(run(&home, &repo, &["init"]).status.success());
        let v = marker(&repo);
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&repo);
        v
    })
    .clone()
}

/// Every --json read whose answer is a pure function of the views (no
/// clock), on the ledger at `db`.
fn view_reads(home: &Path, repo: &Path, db: &Path) -> Vec<String> {
    let reads: [&[&str]; 5] = [
        &["events", "--json"],
        &["subjects", "--json"],
        &["show", FIXTURE_SESSION, "--json"],
        &["recall", "--json", "--subject", "alpha"],
        &["recall", "--json", "cache preflight parser"],
    ];
    reads
        .iter()
        .map(|args| {
            let o = peat(home, repo, args).env("PEAT_DB", db).output().unwrap();
            assert!(o.status.success(), "{args:?}: {}", text(&o));
            String::from_utf8(o.stdout).unwrap()
        })
        .collect()
}

/// The views at `repo` must equal a from-scratch fold of its ledger: a
/// copy whose marker is forced stale is rebuilt inline by a plain read.
fn assert_views_equal_a_fresh_fold(home: &Path, repo: &Path) -> Vec<String> {
    let live = repo.join(".peat/db");
    let copy = home.join("fresh/db");
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    let cp = Command::new("cp")
        .arg("-R")
        .arg(&live)
        .arg(&copy)
        .status()
        .unwrap();
    assert!(cp.success());
    std::fs::write(home.join("fresh/db.view-version"), "1\n").unwrap();
    let fresh = view_reads(home, repo, &copy);
    assert!(home.join("fresh/db.old").exists(), "the copy was rebuilt");
    let got = view_reads(home, repo, &live);
    assert_eq!(got, fresh, "rebuilt views differ from a fresh fold");
    got
}

/// Stale views + concurrent SessionStarts: each hook returns at once with
/// the lookup line and exits 0; exactly one detached rebuild runs; the
/// views end up equal to a fresh fold.
#[test]
fn stale_views_brief_the_lookup_line_and_rebuild_in_the_background() {
    let (home, repo) = stale_repo("stale", "3\n", true);
    let outs: Vec<_> = ["a-1", "b-2", "c-3"]
        .map(|s| {
            let (home, repo) = (home.clone(), repo.clone());
            std::thread::spawn(move || hook_with(&home, &repo, &start_payload(s)))
        })
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    for (o, took) in &outs {
        assert!(o.status.success(), "a hook exits 0: {}", text(o));
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(
            out.starts_with("peat: to see the sessions behind") && out.lines().count() == 1,
            "the fallback line only: {}",
            text(o)
        );
        assert!(
            took.as_millis() < u128::from(REBUILD_DELAY_MS / 2),
            "the hook waited on the rebuild ({took:?}): {}",
            text(o)
        );
    }
    assert_eq!(marker(&repo), "3", "nothing rebuilt inline");
    await_rebuild(&repo);
    // the losers of the spawn race may still be exiting
    std::thread::sleep(std::time::Duration::from_millis(500));
    let log = std::fs::read_to_string(repo.join(".peat/db.rebuild.log")).unwrap();
    assert_eq!(
        log.matches("rebuilt from").count(),
        1,
        "one migrator: {log}"
    );
    assert!(
        log.lines()
            .all(|l| l.matches("peat: ").count() + l.matches("view schema v").count() == 1),
        "each message on its own line: {log}"
    );
    assert!(repo.join(".peat/db.old").exists(), "previous db kept");
    let reads = assert_views_equal_a_fresh_fold(&home, &repo);
    assert!(
        reads[1].contains("the second claim"),
        "the ledger survived: {}",
        reads[1]
    );
    // and the next session briefs normally
    let (o, _) = hook_with(&home, &repo, &start_payload("d-4"));
    assert!(o.status.success(), "{}", text(&o));
    let _ = std::fs::remove_dir_all(&home);
}

/// Captures fired while the background rebuild runs (a Stop of one
/// session, the SessionEnd of another that never fires again) are not
/// lost: they wait for the rebuild and land in the rebuilt views.
#[test]
fn captures_during_a_background_rebuild_are_not_lost() {
    let (home, repo) = stale_repo("capture", "3\n", false);
    // a second session: the fixture under another id
    const OTHER: &str = "bbbbbbbb-0000-4000-8000-000000000002";
    let other = home.join("other.jsonl");
    let text_ = std::fs::read_to_string(FIXTURE).unwrap();
    std::fs::write(&other, text_.replace(FIXTURE_SESSION, OTHER)).unwrap();
    let sessions = [
        ("Stop", FIXTURE_SESSION, PathBuf::from(FIXTURE)),
        ("SessionEnd", OTHER, other),
    ];

    let (o, took) = hook_with(&home, &repo, &start_payload("a-1"));
    assert!(o.status.success(), "{}", text(&o));
    assert!(took.as_millis() < u128::from(REBUILD_DELAY_MS / 2));
    for (event, session, transcript) in &sessions {
        let (o, took) = hook_with(&home, &repo, &capture_payload(event, session, transcript));
        assert!(o.status.success() && o.stdout.is_empty(), "{}", text(&o));
        assert!(took.as_millis() < u128::from(REBUILD_DELAY_MS / 2));
    }
    assert_eq!(marker(&repo), "3", "the rebuild is still running");
    await_rebuild(&repo);
    // the detached captures follow the rebuild; wait for both to land,
    // each exactly as a direct capture into a fresh ledger reads
    let reference = home.join("reference/db");
    for (_, session, transcript) in &sessions {
        let o = peat(&home, &repo, &["capture", transcript.to_str().unwrap()])
            .env("PEAT_DB", &reference)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", text(&o));
        let want = peat(&home, &repo, &["show", session, "--json"])
            .env("PEAT_DB", &reference)
            .output()
            .unwrap()
            .stdout;
        assert!(!want.is_empty());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            let o = run(&home, &repo, &["show", session, "--json"]);
            if o.status.success() && o.stdout == want {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the capture of {session} fired during the rebuild never landed: {}",
                text(&o)
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
    let reads = assert_views_equal_a_fresh_fold(&home, &repo);
    assert!(reads[1].contains("the first claim"), "{}", reads[1]);
    let _ = std::fs::remove_dir_all(&home);
}

/// A binary older than the views never rebuilds them: the hook is a no-op
/// with one stderr line, any other command an error naming both versions.
#[test]
fn an_older_binary_refuses_newer_views() {
    let newer = (current_version().parse::<u32>().unwrap() + 1).to_string();
    let (home, repo) = stale_repo("newer", &format!("{newer}\n"), false);
    for payload in [
        start_payload("a-1"),
        capture_payload("Stop", FIXTURE_SESSION, Path::new(FIXTURE)),
    ] {
        let (o, _) = hook_with(&home, &repo, &payload);
        assert!(o.status.success(), "{}", text(&o));
        assert!(
            o.stdout.is_empty(),
            "nothing in agent context: {}",
            text(&o)
        );
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(err.lines().count(), 1, "{}", text(&o));
        assert!(err.contains("newer peat"), "{}", text(&o));
    }
    let o = run(&home, &repo, &["subjects"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains(&format!("v{newer}"))
            && err.contains(&format!("v{}", current_version()))
            && err.contains("upgrade peat"),
        "{}",
        text(&o)
    );
    // give a wrongly spawned rebuild or capture the time to show itself
    std::thread::sleep(std::time::Duration::from_millis(REBUILD_DELAY_MS + 1_000));
    assert_eq!(marker(&repo), newer, "never rebuilt");
    assert!(!repo.join(".peat/db.old").exists(), "never rebuilt");
    assert!(!repo.join(".peat/cursor").exists(), "never captured");
    assert!(!repo.join(".peat/db.rebuild.log").exists(), "never spawned");
    let _ = std::fs::remove_dir_all(&home);
}

/// A command that finds another process rebuilding the views waits for it
/// only so long, then exits 75; a capture that gives up writes nothing,
/// cursor included, so the next capture of that transcript redoes it.
#[test]
fn a_hung_migrator_bounds_the_wait() {
    let (home, repo) = stale_repo("hung", "3\n", false);
    // a migrator that never finishes: the lock, held by this test
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(repo.join(".peat/db.migrate.lock"))
        .unwrap();
    lock.lock().unwrap();
    for args in [&["subjects"][..], &["capture", FIXTURE][..]] {
        let t = std::time::Instant::now();
        let o = peat(&home, &repo, args)
            .env("PEAT_TEST_MIGRATE_WAIT_SECS", "1")
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(75), "{args:?}: {}", text(&o));
        assert!(t.elapsed() < std::time::Duration::from_secs(20), "{args:?}");
        assert!(String::from_utf8_lossy(&o.stderr).contains("giving up"));
    }
    assert!(!repo.join(".peat/cursor").exists(), "no cursor advanced");
    drop(lock);
    let o = run(&home, &repo, &["capture", FIXTURE]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(repo.join(".peat/cursor").exists());
    let _ = std::fs::remove_dir_all(&home);
}

/// The detached rebuild keeps none of the hook's descriptors: a harness
/// reading a pipe the hook inherited (here fd 3) sees EOF when the hook
/// exits, not when the background rebuild ends.
#[cfg(unix)]
#[test]
fn the_detached_rebuild_holds_no_inherited_descriptor() {
    use std::io::Read;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    let (home, repo) = stale_repo("fds", "3\n", false);
    let mut fds = [0i32; 2];
    // SAFETY: pipe fills two fresh descriptors we then own; both are
    // close-on-exec, so only the dup2'd fd 3 reaches the hook
    let (read_end, write_end) = unsafe {
        assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
        for fd in fds {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        (
            std::fs::File::from(OwnedFd::from_raw_fd(fds[0])),
            OwnedFd::from_raw_fd(fds[1]),
        )
    };
    let w = fds[1];
    let mut c = peat(&home, &repo, &["hook"]);
    c.env("PEAT_TEST_REBUILD_DELAY_MS", REBUILD_DELAY_MS.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: dup2 is async-signal-safe
    unsafe {
        c.pre_exec(move || {
            if libc::dup2(w, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let t = std::time::Instant::now();
    let mut child = c.spawn().unwrap();
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(start_payload("a-1").as_bytes()).unwrap();
    }
    drop(write_end);
    let reader = std::thread::spawn(move || {
        let mut sink = Vec::new();
        let mut r = read_end;
        let _ = r.read_to_end(&mut sink);
        t.elapsed()
    });
    assert!(child.wait().unwrap().success());
    let eof = reader.join().unwrap();
    assert!(
        eof.as_millis() < u128::from(REBUILD_DELAY_MS / 2),
        "fd 3 stayed open in the detached rebuild ({eof:?})"
    );
    await_rebuild(&repo);
    let _ = std::fs::remove_dir_all(&home);
}
