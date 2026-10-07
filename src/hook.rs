//! `peat hook` — the one command every harness hook runs.
//!
//! Claude Code and Codex hand each hook a JSON object on stdin that names
//! its own moment (`hook_event_name`), so a single verb can serve all six
//! moments: the harness config says `peat hook` and nothing else, and the
//! logic that used to live in copied bash (jq extraction, once-per-session
//! markers, `nohup` detachment, `PEAT_DB` anchors, `READY` gates) lives
//! here, versioned with the binary. Upgrading peat upgrades the hooks.
//!
//! Contract: never fail a session. Every path exits 0; a missing field or
//! an absent ledger is a silent no-op, never an error.

use std::path::{Path, PathBuf};

use crate::db;

/// Which harness wrote the config we are installing into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fabric {
    Claude,
    /// Claude Code's per-user, git-ignored settings layer.
    ClaudeLocal,
    Codex,
}

impl Fabric {
    /// The project-relative config file each harness reads hooks from.
    pub fn config_path(self) -> &'static str {
        match self {
            Fabric::Claude => ".claude/settings.json",
            Fabric::ClaudeLocal => ".claude/settings.local.json",
            Fabric::Codex => ".codex/hooks.json",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Fabric::Claude | Fabric::ClaudeLocal => "claude",
            Fabric::Codex => "codex",
        }
    }
    fn install_flag(self) -> &'static str {
        match self {
            Fabric::Claude => "",
            Fabric::ClaudeLocal => " --local",
            Fabric::Codex => " --codex",
        }
    }
}

/// The six moments peat covers, in the harness's spelling.
pub const EVENTS: [&str; 5] = [
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "PreCompact",
    "SessionEnd",
];

/// The fields of the stdin object peat reads. Everything optional: a
/// harness that omits a field gets the no-op branch, not a crash.
#[derive(Default, Debug, PartialEq)]
pub struct Input {
    pub event: String,
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub last_assistant_message: Option<String>,
    /// UserPromptSubmit: what the user just typed.
    pub prompt: Option<String>,
}

impl Input {
    pub fn parse(raw: &str) -> Input {
        let v: serde_json::Value = serde_json::from_str(raw).unwrap_or_default();
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        Input {
            event: s("hook_event_name").unwrap_or_default(),
            session_id: s("session_id").filter(|x| !x.is_empty()),
            transcript_path: s("transcript_path").filter(|x| !x.is_empty()),
            last_assistant_message: s("last_assistant_message").filter(|x| !x.trim().is_empty()),
            prompt: s("prompt").filter(|x| !x.trim().is_empty()),
        }
    }
}

/// What a hook invocation decided to do. Pure — computed from the input
/// and a few filesystem facts, so it is testable without a harness.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Nothing to do (unknown event, missing field, push not opted in).
    Nothing,
    /// Print the brief.
    Brief,
    /// Search memory for this prompt and inject what both lanes agree on.
    /// Needs the open ledger, so it is handed back to `main` like the brief.
    Push { session: String, prompt: String },
    /// Detach a capture of this transcript with this closing message.
    Capture {
        transcript: PathBuf,
        final_msg: Option<String>,
    },
}

/// Decide the action. `push` answers "has this ledger opted in to
/// prompt-time push?" and is passed in so the decision stays pure.
///
/// peat no longer asks agents to write observations (the nudges at the
/// first prompt, after commits and after compaction are gone): agents
/// wrote them rarely and mostly as status, and the distiller now writes
/// memory from what the harness already records. See
/// `.design/2026-10-07-distill-reads-the-harness.md`.
pub fn plan(input: &Input, push: bool) -> Action {
    match input.event.as_str() {
        "SessionStart" => Action::Brief,
        "UserPromptSubmit" => match push_for(input, push) {
            Some((sid, prompt)) => Action::Push {
                session: sid.to_string(),
                prompt: prompt.to_string(),
            },
            None => Action::Nothing,
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
    let db = db::db_path();
    if !db.is_dir() {
        return false;
    }
    let off = |dir: Option<&Path>| dir.is_some_and(|d| d.join("off").exists());
    !(off(Some(&db::peat_dir())) || off(db.parent()))
}

/// Is prompt-time push switched on for this ledger? Opt-in, like the
/// distiller: a file named `push` beside the database. It changes what
/// every agent on the ledger sees, so an upgrade must not start it.
fn push_enabled() -> bool {
    db::db_path()
        .parent()
        .is_some_and(|d| d.join("push").exists())
}

/// A prompt worth searching memory for: the user's own words, with
/// enough of them to mean something. Slash commands and harness
/// wrappers are not questions.
fn pushable(prompt: &str) -> bool {
    let p = prompt.trim();
    p.len() >= 24 && !p.starts_with('/') && !p.starts_with('<') && !p.starts_with("[hail ")
}

/// The session and prompt to push memory for, if this is a prompt worth
/// it on a ledger that opted in. Pure, so the decision is testable.
fn push_for(input: &Input, enabled: bool) -> Option<(&str, &str)> {
    if input.event != "UserPromptSubmit" || !enabled {
        return None;
    }
    let (sid, prompt) = (input.session_id.as_deref()?, input.prompt.as_deref()?);
    pushable(prompt).then_some((sid, prompt))
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
    // a distiller's model command may itself be a harness with hooks:
    // its session must never be briefed, captured, or distilled in turn
    if !enabled() || std::env::var_os(crate::distill::GUARD_ENV).is_some() {
        return Action::Nothing;
    }
    let action = plan(&input, push_enabled());
    match &action {
        Action::Brief => {
            if let Some(sid) = &input.session_id {
                let dir = db::peat_dir();
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(dir.join("current-session"), sid);
            }
            // the sweep: distill whatever went quiet since the last
            // session started. Detached, and a no-op where this ledger
            // paused it (`peat distill --sweep` checks `.peat/distill-off`).
            detach(
                &["distill", "--sweep", "--since", "14", "--limit", "12"].map(std::ffi::OsStr::new),
            );
        }
        Action::Capture {
            transcript,
            final_msg,
            // a turn ending is not a stretch ending; the two moments that
            // close one (context replaced, session over) distill it
        } => detach_capture(transcript, final_msg.as_deref()),
        Action::Nothing | Action::Push { .. } => {}
    }
    action
}

/// The `additionalContext` envelope. Nested under `hookSpecificOutput`
/// with the event name: the shape Codex requires and Claude Code accepts.
pub fn nudge_json(event: &str, text: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": text,
        }
    })
    .to_string()
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

// ---- install ---------------------------------------------------------

/// The peat skill: the judgment half (when to read, how to deposit, how
/// far to trust a line), shipped inside the binary so `install` can lay
/// it beside the hooks and `peat skill` can print it.
pub const SKILL: &str = include_str!("../skills/peat/SKILL.md");

/// Where each harness discovers project skills.
pub fn skill_path(fabric: Fabric) -> &'static str {
    match fabric {
        Fabric::Claude | Fabric::ClaudeLocal => ".claude/skills/peat/SKILL.md",
        Fabric::Codex => ".agents/skills/peat/SKILL.md",
    }
}

/// Write the skill file; returns whether it changed. `check` only reports.
pub fn install_skill(root: &Path, fabric: Fabric, check: bool) -> Result<String, String> {
    let path = root.join(skill_path(fabric));
    let current = std::fs::read_to_string(&path).ok();
    if check {
        return if current.as_deref() == Some(SKILL) {
            Ok(format!("✓ {}: current", path.display()))
        } else {
            Err(format!(
                "{}: missing or stale — run: peat hook install",
                path.display()
            ))
        };
    }
    if current.as_deref() == Some(SKILL) {
        return Ok(format!("= {}: already current", path.display()));
    }
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    std::fs::write(&path, SKILL).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!("✓ {}: written", path.display()))
}

/// The hook entry every event gets.
fn entry() -> serde_json::Value {
    serde_json::json!({ "type": "command", "command": "peat hook" })
}

/// The full snippet for one fabric — what `--print` shows and what
/// `install` merges in.
pub fn snippet() -> serde_json::Value {
    let mut hooks = serde_json::Map::new();
    for ev in EVENTS {
        let mut group = serde_json::Map::new();
        group.insert("hooks".into(), serde_json::Value::Array(vec![entry()]));
        hooks.insert(ev.into(), serde_json::Value::Array(vec![group.into()]));
    }
    serde_json::json!({ "hooks": hooks })
}

/// A hook command peat used to install by hand: anything invoking peat
/// that is not the one-verb form. Removed on install so a project that
/// copied the old snippet ends up with exactly one peat hook per event.
fn is_legacy(cmd: &str) -> bool {
    cmd != "peat hook" && (cmd.contains("peat ") || cmd.contains("/peat "))
}

/// Merge `peat hook` into an existing config value. Returns whether
/// anything changed. Other keys and other tools' hooks are untouched.
pub fn merge(
    config: &mut serde_json::Value,
    matcher_for: impl Fn(&str) -> Option<&'static str>,
) -> bool {
    if !config.is_object() {
        *config = serde_json::json!({});
    }
    let root = config.as_object_mut().unwrap();
    let hooks = root.entry("hooks").or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let hooks = hooks.as_object_mut().unwrap();
    let mut changed = false;
    // events peat no longer uses (PostToolUse carried the commit nudge):
    // drop every peat entry there, legacy snippet or one-verb, so an
    // upgrade's install really removes what the release removed
    let retired: Vec<String> = hooks
        .keys()
        .filter(|k| !EVENTS.contains(&k.as_str()))
        .cloned()
        .collect();
    for ev in retired {
        let Some(groups) = hooks.get_mut(&ev).and_then(|g| g.as_array_mut()) else {
            continue;
        };
        for g in groups.iter_mut() {
            if let Some(list) = g.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                let before = list.len();
                list.retain(|h| {
                    !h.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|c| c == "peat hook" || is_legacy(c))
                });
                changed |= list.len() != before;
            }
        }
        let before = groups.len();
        groups.retain(|g| {
            g.get("hooks")
                .and_then(|h| h.as_array())
                .is_none_or(|l| !l.is_empty())
        });
        changed |= groups.len() != before;
        if groups.is_empty() {
            hooks.remove(&ev);
            changed = true;
        }
    }
    for ev in EVENTS {
        let groups = hooks
            .entry(ev)
            .or_insert_with(|| serde_json::Value::Array(vec![]));
        if !groups.is_array() {
            *groups = serde_json::Value::Array(vec![]);
        }
        let groups = groups.as_array_mut().unwrap();
        // strip legacy peat entries, then empty groups
        for g in groups.iter_mut() {
            if let Some(list) = g.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                let before = list.len();
                list.retain(|h| {
                    !h.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(is_legacy)
                });
                changed |= list.len() != before;
            }
        }
        let before = groups.len();
        groups.retain(|g| {
            g.get("hooks")
                .and_then(|h| h.as_array())
                .is_none_or(|l| !l.is_empty())
        });
        changed |= groups.len() != before;
        let present = groups.iter().any(|g| {
            g.get("hooks")
                .and_then(|h| h.as_array())
                .is_some_and(|l| l.iter().any(|h| h == &entry()))
        });
        if !present {
            let mut group = serde_json::Map::new();
            if let Some(m) = matcher_for(ev) {
                group.insert("matcher".into(), m.into());
            }
            group.insert("hooks".into(), serde_json::Value::Array(vec![entry()]));
            groups.push(group.into());
            changed = true;
        }
    }
    changed
}

/// Which events already carry the one-verb hook in this config.
pub fn installed_events(config: &serde_json::Value) -> Vec<&'static str> {
    EVENTS
        .into_iter()
        .filter(|ev| {
            config
                .get("hooks")
                .and_then(|h| h.get(ev))
                .and_then(|g| g.as_array())
                .is_some_and(|groups| {
                    groups.iter().any(|g| {
                        g.get("hooks")
                            .and_then(|h| h.as_array())
                            .is_some_and(|l| l.iter().any(|h| h == &entry()))
                    })
                })
        })
        .collect()
}

/// Install (or check) the hooks for one fabric at the desk root. Returns
/// a one-line report; `Err` carries the reason a check failed.
pub fn install(root: &Path, fabric: Fabric, check: bool) -> Result<String, String> {
    let path = root.join(fabric.config_path());
    let mut config: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| {
            format!(
                "{}: not valid JSON ({e}); fix or move it first",
                path.display()
            )
        })?,
        Err(_) => serde_json::json!({}),
    };
    if check {
        let have = installed_events(&config);
        return if have.len() == EVENTS.len() {
            Ok(format!(
                "✓ {}: peat hook on all {} events",
                path.display(),
                EVENTS.len()
            ))
        } else {
            let missing: Vec<_> = EVENTS.iter().filter(|e| !have.contains(e)).collect();
            Err(format!(
                "{}: peat hook missing on {} — run: peat hook install{}",
                path.display(),
                missing
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                fabric.install_flag()
            ))
        };
    }
    let changed = merge(&mut config, |_| None);
    if changed {
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let text = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
        std::fs::write(&path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(format!(
            "✓ {}: peat hook on all {} events",
            path.display(),
            EVENTS.len()
        ))
    } else {
        Ok(format!("= {}: already current", path.display()))
    }
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
    fn push_needs_the_opt_in_a_prompt_and_a_session() {
        let raw = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"why does the land gate fail on clippy?"}"#;
        let i = Input::parse(raw);
        assert_eq!(
            push_for(&i, true),
            Some(("s1", "why does the land gate fail on clippy?"))
        );
        assert_eq!(push_for(&i, false), None, "not opted in");
        let no_sid = Input {
            session_id: None,
            ..Input::parse(raw)
        };
        assert_eq!(push_for(&no_sid, true), None);
        let other = Input {
            event: "Stop".into(),
            ..Input::parse(raw)
        };
        assert_eq!(push_for(&other, true), None);
        let short = Input {
            prompt: Some("ok go".into()),
            ..Input::parse(raw)
        };
        assert_eq!(push_for(&short, true), None);
    }

    #[test]
    fn only_a_real_question_is_worth_searching_memory_for() {
        let raw = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"why does the land gate fail on clippy?"}"#;
        assert!(pushable(Input::parse(raw).prompt.as_deref().unwrap()));
        for p in [
            "/model",
            "ok do it",
            "<command-name>/clear</command-name> and more words here",
            "[hail ask from:a to:b] a relayed message from a peer",
        ] {
            assert!(!pushable(p), "{p}");
        }
    }

    #[test]
    fn session_start_briefs_and_prompts_push_only_when_opted_in() {
        assert_eq!(plan(&input("SessionStart"), false), Action::Brief);
        let mut i = input("UserPromptSubmit");
        i.prompt = Some("why does the land gate fail on clippy?".into());
        assert_eq!(plan(&i, false), Action::Nothing, "no nudge, no push");
        assert!(matches!(plan(&i, true), Action::Push { .. }));
        assert_eq!(plan(&input("PostToolUse"), true), Action::Nothing);
    }

    #[test]
    fn captures_carry_the_final_message_only_at_stop() {
        let mut i = input("Stop");
        i.last_assistant_message = Some("done: `x` $HOME \"q\"".into());
        assert_eq!(
            plan(&i, false),
            Action::Capture {
                transcript: "/t/abc123.jsonl".into(),
                final_msg: Some("done: `x` $HOME \"q\"".into())
            }
        );
        i.event = "PreCompact".into();
        assert_eq!(
            plan(&i, false),
            Action::Capture {
                transcript: "/t/abc123.jsonl".into(),
                final_msg: None
            }
        );
        i.transcript_path = None;
        i.session_id = Some("no-such-session-anywhere".into());
        assert_eq!(plan(&i, false), Action::Nothing);
        assert_eq!(plan(&input("Whatever"), false), Action::Nothing);
    }

    #[test]
    fn nudge_json_is_the_nested_shape() {
        let v: serde_json::Value =
            serde_json::from_str(&nudge_json("UserPromptSubmit", "hi")).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
        assert_eq!(v["hookSpecificOutput"]["additionalContext"], "hi");
    }

    #[test]
    fn install_removes_peat_from_events_it_no_longer_uses() {
        let mut cfg = serde_json::json!({
            "hooks": {
                "PostToolUse": [
                    {"matcher": "Bash", "hooks": [{"type":"command","command":"in=$(cat); case \"$c\" in *git\\ commit*) echo '{\"additionalContext\":\"deposit peat obs\"}';; esac"}]},
                    {"matcher": "Bash", "hooks": [{"type":"command","command":"peat hook"}]}
                ],
                "Notification": [{"hooks": [{"type":"command","command":"say done"}]}]
            }
        });
        assert!(merge(&mut cfg, |_| None));
        assert!(cfg["hooks"].get("PostToolUse").is_none(), "{cfg}");
        assert_eq!(
            cfg["hooks"]["Notification"][0]["hooks"][0]["command"], "say done",
            "other tools' hooks survive, on any event"
        );
        assert!(!merge(&mut cfg, |_| None), "second merge changes nothing");
    }

    #[test]
    fn merge_is_idempotent_and_replaces_legacy_bash() {
        let mut cfg = serde_json::json!({
            "permissions": {"allow": ["Bash(ls)"]},
            "hooks": {
                "SessionStart": [
                    {"hooks": [{"type":"command","command":"bd prime"}]},
                    {"hooks": [{"type":"command","command":"in=$(cat); PEAT_DB=/x peat brief || true"}]}
                ],
                "Stop": [{"hooks": [{"type":"command","command":"in=$(cat); nohup peat capture \"$TP\" &"}]}]
            }
        });
        let m = |_: &str| None;
        assert!(merge(&mut cfg, m));
        assert_eq!(installed_events(&cfg).len(), EVENTS.len());
        assert_eq!(
            cfg["permissions"]["allow"][0], "Bash(ls)",
            "other keys survive"
        );
        let ss = cfg["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(
            ss[0]["hooks"][0]["command"], "bd prime",
            "other tools' hooks survive"
        );
        assert_eq!(
            ss.len(),
            2,
            "legacy peat group removed, one-verb group added"
        );
        assert_eq!(cfg["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(!merge(&mut cfg, m), "second merge changes nothing");
    }

    #[test]
    fn install_round_trips_on_disk() {
        let dir = std::env::temp_dir().join(format!("peat-hook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            install(&dir, Fabric::Claude, true).is_err(),
            "nothing installed yet"
        );
        install(&dir, Fabric::Claude, false).unwrap();
        install(&dir, Fabric::Claude, true).unwrap();
        assert!(
            install(&dir, Fabric::Claude, false)
                .unwrap()
                .starts_with("= ")
        );
        install(&dir, Fabric::Codex, false).unwrap();
        assert!(dir.join(".codex/hooks.json").exists());
        assert!(install_skill(&dir, Fabric::Claude, true).is_err());
        install_skill(&dir, Fabric::Claude, false).unwrap();
        install_skill(&dir, Fabric::Claude, true).unwrap();
        assert!(
            install_skill(&dir, Fabric::Claude, false)
                .unwrap()
                .starts_with("= ")
        );
        assert!(SKILL.starts_with("---\nname: peat\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
