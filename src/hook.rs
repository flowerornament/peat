//! `peat hook` — the one command every harness hook runs.
//!
//! Claude Code and Codex hand each hook a JSON object on stdin that names
//! its own moment (`hook_event_name`), so a single verb serves every
//! moment: the harness config says `peat hook` and nothing else, and the
//! logic that used to live in copied bash (jq extraction, once-per-session
//! markers, `nohup` detachment, `PEAT_DB` anchors, `READY` gates) lives
//! here, versioned with the binary. Upgrading peat upgrades the hooks.
//!
//! Contract: never fail a session. Every path exits 0; a missing field or
//! an absent ledger is a silent no-op, never an error.

use std::path::{Path, PathBuf};

use crate::db;

/// The moments peat covers, in the harness's spelling: brief at the
/// start, capture at every way a stretch of work can end. Events peat used
/// to wire (UserPromptSubmit for push, PostToolUse for the commit nudge)
/// are retired; `install` removes peat from them.
pub const EVENTS: [&str; 4] = ["SessionStart", "Stop", "PreCompact", "SessionEnd"];

/// The fields of the stdin object peat reads. Everything optional: a
/// harness that omits a field gets the no-op branch, not a crash.
#[derive(Default, Debug, PartialEq)]
pub struct Input {
    pub event: String,
    pub session_id: Option<String>,
    /// Why a SessionStart fired: `startup`, `resume`, `clear`, `compact`.
    pub source: Option<String>,
    pub transcript_path: Option<String>,
    pub last_assistant_message: Option<String>,
}

impl Input {
    pub fn parse(raw: &str) -> Input {
        let v: serde_json::Value = serde_json::from_str(raw).unwrap_or_default();
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        Input {
            event: s("hook_event_name").unwrap_or_default(),
            session_id: s("session_id").filter(|x| !x.is_empty()),
            source: s("source").filter(|x| !x.is_empty()),
            transcript_path: s("transcript_path").filter(|x| !x.is_empty()),
            last_assistant_message: s("last_assistant_message").filter(|x| !x.trim().is_empty()),
        }
    }
}

/// What a hook invocation decided to do. Pure — computed from the input
/// and a few filesystem facts, so it is testable without a harness.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Nothing to do (unknown or retired event, missing field).
    Nothing,
    /// Print the agent brief. Needs the open ledger, so it is handed back
    /// to `main`; `session` is the starting session, never briefed on itself.
    Brief { session: Option<String> },
    /// Detach a capture of this transcript with this closing message.
    Capture {
        transcript: PathBuf,
        final_msg: Option<String>,
    },
}

/// Decide the action. Pure — the input alone decides, so it is testable
/// without a harness. peat asks nothing of the agent: no deposit nudges,
/// no prompt-time push (see `.design/2026-10-19-peat-is-the-record.md`).
pub fn plan(input: &Input) -> Action {
    match input.event.as_str() {
        "SessionStart" => Action::Brief {
            session: input.session_id.clone(),
        },
        "Stop" | "PreCompact" | "SessionEnd" => {
            let transcript = input
                .transcript_path
                .clone()
                .map(PathBuf::from)
                .or_else(|| input.session_id.as_deref().and_then(codex_rollout));
            match transcript {
                Some(t) => Action::Capture {
                    transcript: t,
                    final_msg: if input.event == "Stop" {
                        input.last_assistant_message.clone()
                    } else {
                        None
                    },
                },
                None => Action::Nothing,
            }
        }
        _ => Action::Nothing,
    }
}

/// Codex's Stop hook does not always carry a transcript path; its rollout
/// lives under `~/.codex/sessions/**/rollout-*<session>*.jsonl`.
fn codex_rollout(session: &str) -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var("HOME").ok()?)
        .join(".codex")
        .join("sessions");
    fn walk(dir: &Path, session: &str, depth: u8) -> Option<PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if depth > 0
                    && let Some(hit) = walk(&p, session, depth - 1)
                {
                    return Some(hit);
                }
            } else if let Some(name) = p.file_name().and_then(|n| n.to_str())
                && name.starts_with("rollout-")
                && name.ends_with(".jsonl")
                && name.contains(session)
            {
                return Some(p);
            }
        }
        None
    }
    walk(&root, session, 6)
}

/// Is peat switched on for this desk? The ledger must exist (or resolve
/// through a redirect / worktree anchor), and no `.peat/off` marker may
/// be present beside the desk or beside the ledger — the pause switch.
pub fn enabled() -> bool {
    db::db_path().is_dir() && !paused()
}

/// Is a `.peat/off` marker present beside the desk or beside the ledger?
pub fn paused() -> bool {
    let off = |dir: Option<&Path>| dir.is_some_and(|d| d.join("off").exists());
    off(Some(&db::peat_dir())) || off(db::db_path().parent())
}

/// How close together two identical SessionStarts must fire to be one
/// moment wired twice (a user-level hook plus a leftover project copy)
/// rather than two moments (a later resume or compaction).
pub const DUP_WINDOW_MS: u64 = 10_000;

/// Markers older than this are swept; they only need to outlive a window.
const MARKER_TTL_MS: u64 = 3_600_000;

/// Is this the first time this exact moment fired? Keyed by `key` in
/// `dir`, one file per moment holding the time it last fired. Hooks wired
/// twice run concurrently, so the read-decide-write runs under an
/// exclusive lock on the marker itself: of two invocations within
/// [`DUP_WINDOW_MS`], exactly one sees `true`. Any filesystem failure
/// answers `true` — a duplicate brief is better than none.
///
/// `now_ms` is the wall clock, read here at the capture boundary, never
/// in a fold path.
pub fn first_fire(dir: &Path, key: &str, now_ms: u64) -> bool {
    use std::io::{Read, Seek, Write};
    if std::fs::create_dir_all(dir).is_err() {
        return true;
    }
    // sweep markers of moments long past (best effort; a marker swept
    // while another process holds it costs at most one duplicate brief)
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|t| now_ms.saturating_sub(t.as_millis() as u64) > MARKER_TTL_MS);
        if old && e.file_name() != key {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let Ok(mut f) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(key))
    else {
        return true;
    };
    if f.lock().is_err() {
        return true;
    }
    let mut text = String::new();
    let _ = f.read_to_string(&mut text);
    let last: Option<u64> = text.trim().parse().ok();
    if last.is_some_and(|t| now_ms.saturating_sub(t) < DUP_WINDOW_MS) {
        return false;
    }
    let _ = f.set_len(0);
    let _ = f.rewind();
    let _ = write!(f, "{now_ms}");
    true
}

/// The marker name for one SessionStart: session, event and source, made
/// filename-safe.
fn fire_key(input: &Input) -> Option<String> {
    let sid = input.session_id.as_deref()?;
    let raw = format!(
        "{sid}.{}.{}",
        input.event,
        input.source.as_deref().unwrap_or("-")
    );
    Some(
        raw.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect(),
    )
}

/// Spawn `peat <args>` in its own process group with all stdio closed,
/// so the hook returns in milliseconds and a harness deadline (or
/// teardown) cannot cancel the work.
fn detach(args: &[&std::ffi::OsStr]) {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("peat"));
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let _ = cmd.spawn();
}

/// Run the hook: read stdin, decide, and perform everything except the
/// brief, which needs the open ledger and is handed back to `main`.
pub fn run(event_override: Option<String>) -> Action {
    let mut raw = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw);
    let mut input = Input::parse(&raw);
    if let Some(e) = event_override {
        input.event = e;
    }
    if !enabled() {
        return Action::Nothing;
    }
    let action = plan(&input);
    match &action {
        Action::Brief { .. } => {
            let dir = db::peat_dir();
            if let Some(sid) = &input.session_id {
                db::ensure_ignored_dir(&dir);
                let _ = std::fs::write(dir.join("current-session"), sid);
            }
            // the same moment wired twice briefs once
            if let Some(key) = fire_key(&input)
                && !first_fire(&dir.join("fired"), &key, db::now_ms())
            {
                return Action::Nothing;
            }
        }
        Action::Capture {
            transcript,
            final_msg,
        } => detach_capture(transcript, final_msg.as_deref()),
        Action::Nothing => {}
    }
    action
}

/// Detach `peat capture`. Arguments travel as argv — the closing
/// message is never re-quoted through a shell.
fn detach_capture(transcript: &Path, final_msg: Option<&str>) {
    let mut args: Vec<&std::ffi::OsStr> = vec!["capture".as_ref(), transcript.as_os_str()];
    if let Some(m) = final_msg {
        args.extend(["--final-msg".as_ref(), std::ffi::OsStr::new(m)]);
    }
    detach(&args);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(event: &str) -> Input {
        Input {
            event: event.into(),
            session_id: Some("abc123".into()),
            transcript_path: Some("/t/abc123.jsonl".into()),
            ..Default::default()
        }
    }

    #[test]
    fn parses_the_fields_hooks_carry() {
        let raw = r#"{"hook_event_name":"Stop","session_id":"s1","last_assistant_message":"done",
            "transcript_path":""}"#;
        let i = Input::parse(raw);
        assert_eq!(i.event, "Stop");
        assert_eq!(i.last_assistant_message.as_deref(), Some("done"));
        assert_eq!(i.transcript_path, None, "empty path is absent");
        assert_eq!(Input::parse("not json"), Input::default());
    }

    #[test]
    fn first_fire_admits_one_of_concurrent_twins() {
        let dir = std::env::temp_dir().join(format!("peat-fired-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for round in 0..20 {
            let key = format!("s{round}.SessionStart.startup");
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
            let admitted: usize = (0..4)
                .map(|_| {
                    let (dir, key, b) = (dir.clone(), key.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        b.wait();
                        first_fire(&dir, &key, 1_000_000)
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|h| h.join().unwrap() as usize)
                .sum();
            assert_eq!(admitted, 1, "round {round}");
        }
        let k = "s0.SessionStart.startup";
        assert!(!first_fire(&dir, k, 1_000_000 + DUP_WINDOW_MS - 1));
        assert!(
            first_fire(&dir, k, 1_000_000 + DUP_WINDOW_MS),
            "a later moment with the same key fires again"
        );
        assert!(first_fire(
            &dir,
            "s0.SessionStart.compact",
            1_000_000 + DUP_WINDOW_MS
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fire_keys_are_filename_safe_and_need_a_session() {
        let mut i = input("SessionStart");
        i.session_id = Some("../a b".into());
        i.source = Some("resume".into());
        assert_eq!(fire_key(&i).as_deref(), Some(".._a_b.SessionStart.resume"));
        i.session_id = None;
        assert_eq!(fire_key(&i), None);
    }

    #[test]
    fn session_start_briefs_and_retired_events_do_nothing() {
        assert_eq!(
            plan(&input("SessionStart")),
            Action::Brief {
                session: Some("abc123".into())
            }
        );
        assert_eq!(plan(&input("UserPromptSubmit")), Action::Nothing);
        assert_eq!(plan(&input("PostToolUse")), Action::Nothing);
    }

    #[test]
    fn captures_carry_the_final_message_only_at_stop() {
        let mut i = input("Stop");
        i.last_assistant_message = Some("done: `x` $HOME \"q\"".into());
        assert_eq!(
            plan(&i),
            Action::Capture {
                transcript: "/t/abc123.jsonl".into(),
                final_msg: Some("done: `x` $HOME \"q\"".into())
            }
        );
        i.event = "PreCompact".into();
        assert_eq!(
            plan(&i),
            Action::Capture {
                transcript: "/t/abc123.jsonl".into(),
                final_msg: None
            }
        );
        i.transcript_path = None;
        i.session_id = Some("no-such-session-anywhere".into());
        assert_eq!(plan(&i), Action::Nothing);
        assert_eq!(plan(&input("Whatever")), Action::Nothing);
    }
}
