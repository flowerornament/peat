//! A reader that closes stdout early (`peat … | head -1`) must never make
//! peat panic: user verbs end quietly by SIGPIPE, and the harness hook
//! drops write errors on stdout and stderr alike and exits 0.
#![cfg(unix)]

use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};

/// The write end of a pipe whose read end is already closed: every write
/// to it fails with EPIPE (or raises SIGPIPE), deterministically — no race
/// against how much the pipe buffer would have absorbed.
fn widowed_pipe() -> Stdio {
    let mut fds = [0i32; 2];
    // SAFETY: `pipe` fills two fresh descriptors we then own exclusively.
    unsafe {
        assert_eq!(libc::pipe(fds.as_mut_ptr()), 0, "pipe()");
        libc::close(fds[0]);
        Stdio::from(OwnedFd::from_raw_fd(fds[1]))
    }
}

struct Run {
    status: ExitStatus,
    stderr: String,
}

/// Run peat in a scratch desk whose ledger dir exists but carries no view
/// marker (so opening it prints a view-rebuild notice on stderr: inline
/// for a verb, a background rebuild for the hook), with stdout and/or
/// stderr widowed.
fn run(name: &str, args: &[&str], close_out: bool, close_err: bool) -> Run {
    let dir = scratch(name);
    let mut child = Command::new(env!("CARGO_BIN_EXE_peat"))
        .args(args)
        .current_dir(&dir)
        .env("PEAT_DB", dir.join("db"))
        .stdin(Stdio::null())
        .stdout(if close_out {
            widowed_pipe()
        } else {
            Stdio::null()
        })
        .stderr(if close_err {
            widowed_pipe()
        } else {
            Stdio::piped()
        })
        .spawn()
        .expect("spawn peat");
    let mut stderr = String::new();
    if let Some(mut e) = child.stderr.take() {
        e.read_to_string(&mut stderr).unwrap();
    }
    let status = child.wait().unwrap();
    remove(&dir);
    Run { status, stderr }
}

/// Remove a scratch desk once any background rebuild a hook started in
/// it has finished (it would otherwise recreate the ledger after us).
fn remove(dir: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        // no lock file: nothing ever opened the ledger
        let lock = dir.join("db.migrate.lock");
        let done = !lock.exists()
            || (dir.join("db.view-version").exists()
                && std::fs::File::open(&lock).is_ok_and(|f| f.try_lock().is_ok()));
        if done {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = std::fs::remove_dir_all(dir);
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("peat-epipe-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("db")).unwrap();
    dir
}

fn hook(name: &str, close_out: bool, close_err: bool) -> Run {
    run(
        name,
        &["hook", "--event", "SessionStart"],
        close_out,
        close_err,
    )
}

#[test]
fn user_verb_into_closed_pipe_ends_by_sigpipe_without_panic() {
    let r = run("skill", &["skill"], true, false);
    assert!(
        !r.stderr.contains("panicked"),
        "peat panicked on a closed pipe: {}",
        r.stderr
    );
    // the write cannot succeed, so only a quiet SIGPIPE death is right
    assert_eq!(
        r.status.signal(),
        Some(libc::SIGPIPE),
        "unexpected exit {:?}; stderr: {}",
        r.status,
        r.stderr
    );
}

#[test]
fn hook_brief_into_closed_stdout_exits_zero() {
    let r = hook("hook-out", true, false);
    assert!(
        !r.stderr.contains("panicked"),
        "hook panicked on a closed pipe: {}",
        r.stderr
    );
    assert!(
        r.status.success(),
        "hook must exit 0, got {:?}; stderr: {}",
        r.status,
        r.stderr
    );
}

#[test]
fn hook_with_closed_stderr_exits_zero() {
    for (name, close_out) in [("hook-err", false), ("hook-both", true)] {
        let r = hook(name, close_out, true);
        assert!(
            r.status.success(),
            "hook must exit 0 with stderr closed (stdout closed: {close_out}), got {:?}",
            r.status
        );
    }
}

/// The scratch desk really does print on both streams, so the tests above
/// exercise failing writes rather than silence.
#[test]
fn hook_scratch_desk_writes_both_streams() {
    let dir = scratch("hook-open");
    let out = Command::new(env!("CARGO_BIN_EXE_peat"))
        .args(["hook", "--event", "SessionStart"])
        .current_dir(&dir)
        .env("PEAT_DB", dir.join("db"))
        .stdin(Stdio::null())
        .output()
        .expect("run peat");
    remove(&dir);
    assert!(out.status.success());
    assert!(!out.stdout.is_empty(), "hook brief wrote nothing");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("rebuilding the views"),
        "no stderr notice: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
