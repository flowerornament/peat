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

/// After an upgrade every ledger's views are stale; concurrent first
/// SessionStarts must not rebuild over each other.
#[test]
fn concurrent_starts_on_a_stale_view_schema_rebuild_once() {
    let home = tmp("stale");
    let repo = home.join("r");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    assert!(run(&home, &repo, &["init"]).status.success());
    std::fs::write(repo.join(".peat/db.view-version"), "1\n").unwrap();
    let outs = session_starts(
        &home,
        &repo,
        &[("a-1", "startup"), ("b-2", "startup"), ("c-3", "startup")],
    );
    for o in &outs {
        assert!(
            !String::from_utf8_lossy(&o.stderr).contains("panicked"),
            "{}",
            text(o)
        );
    }
    let rebuilt = outs
        .iter()
        .filter(|o| String::from_utf8_lossy(&o.stderr).contains("rebuilt"))
        .count();
    assert_eq!(rebuilt, 1, "exactly one migrator: {outs:?}");
    assert!(run(&home, &repo, &["subjects"]).status.success());
    let _ = std::fs::remove_dir_all(&home);
}
