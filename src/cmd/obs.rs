//! `peat obs`: the one claim an agent records by hand. Shown only when
//! asked for (`peat subjects`, `peat <subject>`); nothing prompts for it.

use crate::db::{self, Db};
use crate::event::{Basis, Envelope, Event, OBS_SEQ_BASE};
use crate::pipeline::SubjStats;
use crate::ui;

/// Deposit one observation under `subject`.
pub fn obs(
    st: &mut Db,
    subject: String,
    text: Vec<String>,
    from: Vec<u32>,
    session: Option<String>,
    at: Option<String>,
) {
    let ts = match &at {
        None => db::now_ms(),
        Some(d) => match crate::transcript::local_day_ms(d, "12:00:00.000") {
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

/// Repo state at deposit time, stamped by construction — a convention the
/// depositor must remember is a check that gets deleted. Never fatal:
/// `None` outside a repo or without the tool that owns it.
fn deposit_basis() -> Option<Basis> {
    basis_at(&std::env::current_dir().ok()?)
}

/// The basis of the nearest repo at or above `dir`. jj wins over git: a
/// secondary jj workspace has no `.git`, so bare git walks up and answers
/// about whatever repo encloses it (often `$HOME`) — a wrong commit,
/// silently. A `.jj` beside `.git` is a colocated checkout: still jj.
fn basis_at(dir: &std::path::Path) -> Option<Basis> {
    let root = dir
        .ancestors()
        .find(|d| d.join(".jj").exists() || d.join(".git").exists())?;
    if root.join(".jj").exists() {
        jj_basis(root)
    } else {
        git_basis(root)
    }
}

/// `@-` is the last named commit — what HEAD means in a colocated repo,
/// and stable, unlike the working-copy commit. `--ignore-working-copy`
/// because a read must never write the repo: the cost is that an edit
/// made since the last jj command reads clean until something snapshots.
fn jj_basis(root: &std::path::Path) -> Option<Basis> {
    let log = |rev: &str| {
        std::process::Command::new("jj")
            .current_dir(root)
            .args([
                "--ignore-working-copy",
                "log",
                "--no-graph",
                "-r",
                rev,
                "-T",
                "commit_id",
            ])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let commit = log("@-").filter(|c| !c.is_empty())?;
    // a non-empty `@` is uncommitted work (jj tracks no untracked noise:
    // ignored paths never enter `@`)
    let dirty = log("@ & ~empty()").is_some_and(|c| !c.is_empty());
    Some(Basis { commit, dirty })
}

/// git pinned to `root`: `-C` plus a cleared env so neither cwd nor an
/// inherited `GIT_DIR` can redirect it, and a toplevel check so a root
/// git does not own (a stray `.git` file, say) answers nothing.
fn git_basis(root: &std::path::Path) -> Option<Basis> {
    let git = |args: &[&str]| {
        let mut c = std::process::Command::new("git");
        c.arg("-C").arg(root).args(args);
        for v in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
        ] {
            c.env_remove(v);
        }
        c.output().ok().filter(|o| o.status.success())
    };
    let top = git(&["rev-parse", "--show-toplevel"])?;
    let top = std::path::PathBuf::from(String::from_utf8_lossy(&top.stdout).trim());
    if std::fs::canonicalize(&top).ok()? != std::fs::canonicalize(root).ok()? {
        return None;
    }
    let out = git(&["rev-parse", "HEAD"])?;
    let commit = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if commit.is_empty() {
        return None;
    }
    // tracked files only: untracked noise (scratch dirs, data dirs that
    // self-ignore late) must not brand every deposit dirty
    let dirty = git(&["status", "--porcelain", "-uno"]).is_some_and(|o| !o.stdout.is_empty());
    Some(Basis { commit, dirty })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// A command with no inherited git/jj steering: the test owns the repos.
    fn cmd(prog: &str, dir: &Path) -> Command {
        let mut c = Command::new(prog);
        c.current_dir(dir);
        for v in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
        ] {
            c.env_remove(v);
        }
        c.env("JJ_USER", "t").env("JJ_EMAIL", "t@t");
        c
    }

    fn run(prog: &str, dir: &Path, args: &[&str]) -> String {
        let o = cmd(prog, dir).args(args).output().expect("spawn");
        assert!(
            o.status.success(),
            "{prog} {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn have(prog: &str) -> bool {
        Command::new(prog)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("peat-basis-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The jj-workspace bug: a desk with no `.git` under a home that has
    /// one. git walks up and answers about home; the basis must come from
    /// the desk's own jj repo instead.
    #[test]
    fn basis_in_jj_desk_ignores_enclosing_git_repo() {
        if !have("git") || !have("jj") {
            eprintln!("skipping: git or jj missing");
            return;
        }
        let home = scratch("jjdesk");
        run("git", &home, &["init", "-q"]);
        let g = ["-c", "user.name=t", "-c", "user.email=t@t"];
        run(
            "git",
            &home,
            &[&g[..], &["commit", "-q", "--allow-empty", "-m", "home"]].concat(),
        );
        let home_head = run("git", &home, &["rev-parse", "HEAD"]);

        let desk = home.join("desk");
        std::fs::create_dir(&desk).unwrap();
        run("jj", &desk, &["git", "init", "--no-colocate"]);
        assert!(!desk.join(".git").exists(), "desk must be jj-only");
        std::fs::write(desk.join("f"), "one").unwrap();
        run("jj", &desk, &["commit", "-m", "named"]);
        let named = run(
            "jj",
            &desk,
            &["log", "--no-graph", "-r", "@-", "-T", "commit_id"],
        );

        let b = super::basis_at(&desk).expect("jj desk has a basis");
        assert_ne!(
            b.commit, home_head,
            "basis leaked from the enclosing git repo"
        );
        assert_eq!(b.commit, named);
        assert!(!b.dirty, "fresh @ after commit is empty");

        // an edit jj has snapshotted marks the basis dirty (from a subdir too)
        std::fs::create_dir(desk.join("sub")).unwrap();
        std::fs::write(desk.join("f"), "two").unwrap();
        run("jj", &desk, &["status"]);
        let b = super::basis_at(&desk.join("sub")).expect("basis from subdir");
        assert_eq!(b.commit, named);
        assert!(b.dirty, "snapshotted edit in @ is dirty");

        let _ = std::fs::remove_dir_all(&home);
    }

    /// Plain git still works, from a subdirectory, tracked files only.
    #[test]
    fn basis_in_git_repo() {
        if !have("git") {
            eprintln!("skipping: git missing");
            return;
        }
        let root = scratch("git");
        run("git", &root, &["init", "-q"]);
        std::fs::write(root.join("f"), "one").unwrap();
        run("git", &root, &["add", "f"]);
        let g = ["-c", "user.name=t", "-c", "user.email=t@t"];
        run(
            "git",
            &root,
            &[&g[..], &["commit", "-q", "-m", "c"]].concat(),
        );
        let head = run("git", &root, &["rev-parse", "HEAD"]);
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/untracked"), "x").unwrap();

        let b = super::basis_at(&root.join("sub")).expect("git basis");
        assert_eq!(b.commit, head);
        assert!(!b.dirty, "untracked files are not dirt");
        std::fs::write(root.join("f"), "two").unwrap();
        assert!(super::basis_at(&root).expect("git basis").dirty);

        let _ = std::fs::remove_dir_all(&root);
    }
}
