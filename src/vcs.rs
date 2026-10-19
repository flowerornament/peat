//! Which captured agent session made this commit?
//!
//! peat once guessed commits from `git commit` command lines in the
//! transcript, which broke on fragments and never saw a hash. The repos
//! peat serves stamp every jj commit description with trailers written by
//! a SessionStart hook (`scripts/jj-identity`):
//!
//! ```text
//! Jj-Workspace: peat-1a
//! Agent: claude-code
//! Session: e4acd12f-b1bd-4524-b96f-c21d5522a932
//! ```
//!
//! So resolution is a lookup in jj at query time, not something capture
//! records: `lookup` reads the changes a revision names, `parse_trailers`
//! pulls the identity out of each description, and `sessions_for` joins a
//! change to the ledger's sessions — by the `Session:` trailer when there
//! is one, else by desk and time.
//!
//! Only jj is consulted, never git: a jj workspace has no `.git`, and git
//! walks up to whatever repository encloses it (here `~/.git`), silently
//! answering about the wrong history. Every jj call is read-only
//! (`--ignore-working-copy`: no snapshot, no operation recorded). No wall
//! clock is read here; the only time is the committer timestamp jj reports.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The identity trailers of one change, each present only when the final
/// paragraph of the description carried it with a plain-identifier value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trailers {
    /// `Session:` — the agent session id (full or a prefix of it).
    pub session: Option<String>,
    /// `Jj-Workspace:` (or murail's older `Seat:`) — the desk name, which
    /// is the last path component of the session's cwd.
    pub workspace: Option<String>,
    /// `Agent:` — the harness, e.g. `claude-code` or `codex`.
    pub agent: Option<String>,
}

/// One commit as jj reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Full hex commit id.
    pub commit_id: String,
    /// Full change id (jj's reverse-hex alphabet). Divergent commits share
    /// one change id but differ in `commit_id`.
    pub change_id: String,
    /// The full description, trailers included.
    pub description: String,
    /// Committer timestamp, milliseconds since the Unix epoch. jj rewrites
    /// the committer on every rewrite (rebase, describe), so this is when
    /// the commit last took its present form.
    pub committer_ms: u64,
    /// The identity trailers parsed from `description`.
    pub trailers: Trailers,
}

/// What a session spans, as the ledger knows it: where it ran and the
/// first and last event times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpan {
    pub cwd: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Why a session was attributed to a change — the reader needs to know
/// whether the commit named the session or peat inferred it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// The commit's `Session:` trailer names this session.
    Trailer,
    /// No `Session:` trailer: the session ran in the trailer's workspace
    /// and was live (within slack) when the commit was made.
    PlaceAndTime,
}

/// The most changes one `lookup` returns. A revision like `::main` can
/// name thousands of commits; a query surface must not flood.
pub const LOOKUP_CAP: usize = 50;

/// How long after a session's last recorded event a commit can still be
/// attributed to it by place and time: the commit is often the final act
/// before the session ends, and the ledger's last event may precede it.
pub const END_SLACK_MS: u64 = 10 * 60 * 1000;

/// Parse the identity trailers from a description, git-trailer style.
///
/// Only the final paragraph counts, and only when it is not also the first
/// (the subject is never a trailer block) and every line in it has the
/// `Key: value` shape (indented continuation lines allowed) — so a body
/// paragraph that happens to mention `Session: x` is not read as identity.
/// Keys match case-insensitively. A value that is not a plain identifier
/// (`[A-Za-z0-9._-]+`) is ignored, as is everything else in the block.
/// When a key repeats, the last occurrence wins.
pub fn parse_trailers(description: &str) -> Trailers {
    let mut out = Trailers::default();
    let description = description.replace("\r\n", "\n");
    let paragraphs: Vec<&str> = description
        .split("\n\n")
        .map(|p| p.trim_matches('\n'))
        .filter(|p| !p.trim().is_empty())
        .collect();
    let [_, .., last] = paragraphs.as_slice() else {
        return out;
    };
    let lines: Vec<&str> = last.lines().map(str::trim_end).collect();
    let is_block = lines.iter().all(|l| {
        l.starts_with([' ', '\t'])
            || l.split_once(':').is_some_and(|(k, _)| {
                !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            })
    });
    if !is_block {
        return out;
    }
    for line in lines {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if !is_ident(value) {
            continue;
        }
        let slot = match key.to_ascii_lowercase().as_str() {
            "session" => &mut out.session,
            "jj-workspace" | "seat" => &mut out.workspace,
            "agent" => &mut out.agent,
            _ => continue,
        };
        *slot = Some(value.to_string());
    }
    out
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// The machine template: one line per commit, four space-separated
/// fields. The ids are hex / reverse-hex and the timestamp is digits, so
/// none can contain a space; the description goes last, JSON-escaped, so
/// its newlines (and any other control byte) cannot break the record.
const TEMPLATE: &str = r#"commit_id ++ " " ++ change_id ++ " " ++ committer.timestamp().utc().format("%s%3f") ++ " " ++ description.escape_json() ++ "\n""#;

/// The changes `rev` names in the jj repo at `repo_dir`, newest first (jj's
/// log order), at most `LOOKUP_CAP` of them. `rev` is any revset: a
/// commit-id or change-id prefix, a bookmark, a range.
///
/// `Err` carries jj's stderr for an unknown or ambiguous revision, or says
/// that jj is missing or `repo_dir` is not a jj repo. There is no git
/// fallback (see the module doc). Use `lookup_bounded` to learn whether
/// the cap cut the result.
pub fn lookup(repo_dir: &Path, rev: &str) -> Result<Vec<Change>, String> {
    lookup_bounded(repo_dir, rev).map(|(changes, _)| changes)
}

/// `lookup`, plus whether `rev` named more than `LOOKUP_CAP` commits (and
/// the result was cut to the newest `LOOKUP_CAP`).
pub fn lookup_bounded(repo_dir: &Path, rev: &str) -> Result<(Vec<Change>, bool), String> {
    let mut changes = match jj_log(repo_dir, rev) {
        // jj refuses a bare change id that has gone divergent (two visible
        // commits share it). Both are the change the reader asked about,
        // so answer with all of them rather than jj's refusal.
        Err(e) if e.contains("is divergent") && is_ident(rev) => {
            jj_log(repo_dir, &format!("change_id({rev})"))?
        }
        r => r?,
    };
    let truncated = changes.len() > LOOKUP_CAP;
    changes.truncate(LOOKUP_CAP);
    Ok((changes, truncated))
}

/// One read-only `jj log` over `rev`, fetching one past the cap so the
/// caller can tell a full result from a cut one.
fn jj_log(repo_dir: &Path, rev: &str) -> Result<Vec<Change>, String> {
    let out = Command::new("jj")
        .current_dir(repo_dir)
        // jj does not read these, but a caller's git environment has no
        // business steering a jj query.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .args([
            "log",
            "--ignore-working-copy",
            "--no-graph",
            "--color=never",
            "--no-pager",
            "--limit",
            &(LOOKUP_CAP + 1).to_string(),
            "--template",
            TEMPLATE,
        ])
        // `=` form so a revset starting with `-` is never read as a flag.
        .arg(format!("--revisions={rev}"))
        .output()
        .map_err(|e| format!("cannot run jj: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("jj log exited with {}", out.status)
        } else {
            err
        });
    }
    parse_log(&String::from_utf8_lossy(&out.stdout))
}

/// Parse the output of `TEMPLATE`. Kept separate from the subprocess so it
/// is tested against recorded output with no jj installed.
fn parse_log(stdout: &str) -> Result<Vec<Change>, String> {
    stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let bad = || format!("unexpected jj log line: {line}");
            let mut fields = line.splitn(4, ' ');
            let (Some(commit_id), Some(change_id), Some(ms), Some(desc)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(bad());
            };
            let committer_ms = ms.parse::<u64>().map_err(|_| bad())?;
            let description: String = serde_json::from_str(desc).map_err(|_| bad())?;
            Ok(Change {
                commit_id: commit_id.to_string(),
                change_id: change_id.to_string(),
                trailers: parse_trailers(&description),
                description,
                committer_ms,
            })
        })
        .collect()
}

/// The sessions behind `change`, newest `end_ms` first (ties by id).
///
/// With a `Session:` trailer, the answer is the sessions whose id equals
/// it or starts with it — the commit said who made it. Without one, fall
/// back to place and time: sessions whose cwd ends in the trailer's
/// workspace and whose `[start_ms, end_ms + END_SLACK_MS]` contains the
/// commit time. Without a workspace either, nothing: several desks run at
/// once, and time alone would attribute one desk's commit to another.
pub fn sessions_for(change: &Change, sessions: &[(String, SessionSpan)]) -> Vec<(String, Basis)> {
    let t = &change.trailers;
    let mut hits: Vec<(&String, &SessionSpan, Basis)> = if let Some(sid) = &t.session {
        sessions
            .iter()
            .filter(|(id, _)| id.starts_with(sid.as_str()))
            .map(|(id, s)| (id, s, Basis::Trailer))
            .collect()
    } else if let Some(ws) = &t.workspace {
        let at = change.committer_ms;
        sessions
            .iter()
            .filter(|(_, s)| {
                last_component(&s.cwd) == Some(ws.as_str())
                    && s.start_ms <= at
                    && at <= s.end_ms.saturating_add(END_SLACK_MS)
            })
            .map(|(id, s)| (id, s, Basis::PlaceAndTime))
            .collect()
    } else {
        Vec::new()
    };
    hits.sort_by(|a, b| b.1.end_ms.cmp(&a.1.end_ms).then_with(|| a.0.cmp(b.0)));
    hits.into_iter().map(|(id, _, b)| (id.clone(), b)).collect()
}

fn last_component(cwd: &str) -> Option<&str> {
    cwd.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
}

/// The root of every workspace of the jj repository at `repo_dir`: the same
/// file has one absolute path per workspace, and a session records the one
/// it edited. Read-only (`--ignore-working-copy`); empty when jj cannot
/// answer, never a guess.
pub fn workspace_roots(repo_dir: &Path) -> Vec<PathBuf> {
    let out = Command::new("jj")
        .current_dir(repo_dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .args([
            "workspace",
            "list",
            "--ignore-working-copy",
            "--color=never",
            "--no-pager",
            "-T",
            r#"root ++ "\n""#,
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(PathBuf::from)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "e4acd12f-b1bd-4524-b96f-c21d5522a932";

    fn change(trailers: Trailers, committer_ms: u64) -> Change {
        Change {
            commit_id: "c".into(),
            change_id: "k".into(),
            description: String::new(),
            committer_ms,
            trailers,
        }
    }

    fn span(cwd: &str, start_ms: u64, end_ms: u64) -> SessionSpan {
        SessionSpan {
            cwd: cwd.into(),
            start_ms,
            end_ms,
        }
    }

    #[test]
    fn trailers_full_block() {
        let d = format!(
            "area: subject (peat-1)\n\nBody text.\n\nJj-Workspace: peat-1a\nAgent: claude-code\nSession: {SID}\n"
        );
        assert_eq!(
            parse_trailers(&d),
            Trailers {
                session: Some(SID.into()),
                workspace: Some("peat-1a".into()),
                agent: Some("claude-code".into()),
            }
        );
    }

    #[test]
    fn trailers_last_paragraph_only() {
        // Trailer-shaped lines in the body do not count.
        let d = "subject\n\nSession: body-mention\nAgent: x\n\nCloses the loop.\n";
        assert_eq!(parse_trailers(d), Trailers::default());
        let d = "subject\n\nSession: early\n\nJj-Workspace: w\n";
        let t = parse_trailers(d);
        assert_eq!(t.session, None);
        assert_eq!(t.workspace.as_deref(), Some("w"));
    }

    #[test]
    fn trailers_subject_is_never_a_block() {
        assert_eq!(parse_trailers("Session: abc\n"), Trailers::default());
    }

    #[test]
    fn trailers_seat_alias_and_case() {
        let t = parse_trailers("subject\n\nseat: murail-2b\nSESSION: abc123\n");
        assert_eq!(t.workspace.as_deref(), Some("murail-2b"));
        assert_eq!(t.session.as_deref(), Some("abc123"));
    }

    #[test]
    fn trailers_junk_values_ignored() {
        let d = "subject\n\nSession: two words\nJj-Workspace: $(rm -rf)\nAgent:\nCo-Authored-By: A <a@b.c>\n";
        assert_eq!(parse_trailers(d), Trailers::default());
    }

    #[test]
    fn trailers_mixed_block_with_prose_is_not_a_block() {
        let d = "subject\n\nSession: abc\nthis line is prose\n";
        assert_eq!(parse_trailers(d), Trailers::default());
    }

    #[test]
    fn trailers_none_and_empty() {
        assert_eq!(parse_trailers(""), Trailers::default());
        assert_eq!(
            parse_trailers("subject\n\nA plain body.\n"),
            Trailers::default()
        );
    }

    #[test]
    fn trailers_repeat_last_wins_and_crlf_tolerated() {
        let t = parse_trailers("s\n\nSession: a\nSession: b\r\n");
        assert_eq!(t.session.as_deref(), Some("b"));
    }

    #[test]
    fn parse_log_recorded_output() {
        // Recorded from `jj log -T TEMPLATE` on this repo (jj 0.46).
        let out = concat!(
            "a4808dce18da49a0c2cc82c85167f93d3ecb3451 oxnspkmxutztstoooqtvxxnnrnxwmlun 1792384084000 ",
            "\"repo: tune-up \u{2014} own hooks (peat-520.1)\\n\\nJj-Workspace: peat-1a\\nAgent: claude-code\\nSession: e4acd12f-b1bd-4524-b96f-c21d5522a932\\n\"\n",
            "9167ce52bab4ff01140fb9135b2b2e2fc9fd7a6d txutzluvrpvrvzsptkotzowvplolurzy 1791360122000 ",
            "\"release 0.4.0: memory from what the harness already wrote\\n\"\n",
            "0000000000000000000000000000000000000000 zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz 0 \"\"\n",
        );
        let cs = parse_log(out).unwrap();
        assert_eq!(cs.len(), 3);
        assert_eq!(cs[0].commit_id, "a4808dce18da49a0c2cc82c85167f93d3ecb3451");
        assert_eq!(cs[0].change_id, "oxnspkmxutztstoooqtvxxnnrnxwmlun");
        assert_eq!(cs[0].committer_ms, 1_792_384_084_000);
        assert!(cs[0].description.starts_with("repo: tune-up \u{2014}"));
        assert_eq!(cs[0].trailers.session.as_deref(), Some(SID));
        assert_eq!(cs[0].trailers.workspace.as_deref(), Some("peat-1a"));
        assert_eq!(cs[1].trailers, Trailers::default());
        assert_eq!(cs[2].description, "");
        assert_eq!(cs[2].committer_ms, 0);
    }

    #[test]
    fn parse_log_escaped_controls_and_spaces_survive() {
        let out = "abc def 5 \"a b\\u0000c\\td\\n\\nSeat: s1\"\n";
        let cs = parse_log(out).unwrap();
        assert_eq!(cs[0].description, "a b\0c\td\n\nSeat: s1");
        assert_eq!(cs[0].trailers.workspace.as_deref(), Some("s1"));
    }

    #[test]
    fn parse_log_rejects_malformed() {
        assert!(parse_log("abc def notanumber \"x\"\n").is_err());
        assert!(parse_log("abc def 5\n").is_err());
        assert!(parse_log("abc def 5 unquoted\n").is_err());
        assert_eq!(parse_log("").unwrap(), vec![]);
    }

    #[test]
    fn sessions_by_trailer_exact_and_prefix() {
        let sessions = vec![
            (SID.to_string(), span("/x/peat-1a", 0, 100)),
            (format!("{SID}-other"), span("/x/peat-1b", 0, 300)),
            ("ffff".to_string(), span("/x/peat-1a", 0, 200)),
        ];
        let c = change(
            Trailers {
                session: Some(SID.into()),
                workspace: Some("peat-1a".into()),
                agent: None,
            },
            50,
        );
        assert_eq!(
            sessions_for(&c, &sessions),
            vec![
                (format!("{SID}-other"), Basis::Trailer),
                (SID.to_string(), Basis::Trailer),
            ]
        );
        // A short prefix in the trailer still resolves.
        let c = change(
            Trailers {
                session: Some("e4acd12f".into()),
                ..Default::default()
            },
            50,
        );
        assert_eq!(sessions_for(&c, &sessions).len(), 2);
        // A trailer naming no captured session answers nothing — it does
        // not fall back to place and time.
        let c = change(
            Trailers {
                session: Some("0123".into()),
                workspace: Some("peat-1a".into()),
                agent: None,
            },
            50,
        );
        assert!(sessions_for(&c, &sessions).is_empty());
    }

    #[test]
    fn sessions_by_place_and_time() {
        let sessions = vec![
            ("early".to_string(), span("/x/peat-1a", 0, 1_000)),
            ("late".to_string(), span("/x/peat-1a/", 2_000, 5_000)),
            ("other-desk".to_string(), span("/x/peat-1b", 0, 9_000)),
            ("future".to_string(), span("/x/peat-1a", 9_000, 9_500)),
        ];
        let ws = |at| {
            change(
                Trailers {
                    workspace: Some("peat-1a".into()),
                    ..Default::default()
                },
                at,
            )
        };
        assert_eq!(
            sessions_for(&ws(3_000), &sessions),
            vec![
                ("late".to_string(), Basis::PlaceAndTime),
                ("early".to_string(), Basis::PlaceAndTime),
            ]
        );
        // Slack: 10 minutes past `early`'s end still matches; one ms more
        // does not.
        let edge = 1_000 + END_SLACK_MS;
        assert!(
            sessions_for(&ws(edge), &sessions)
                .iter()
                .any(|(id, _)| id == "early")
        );
        assert!(
            !sessions_for(&ws(edge + 1), &sessions)
                .iter()
                .any(|(id, _)| id == "early")
        );
    }

    #[test]
    fn sessions_without_workspace_answer_nothing() {
        let sessions = vec![("s".to_string(), span("/x/peat-1a", 0, 9_000))];
        let c = change(
            Trailers {
                agent: Some("claude-code".into()),
                ..Default::default()
            },
            10,
        );
        assert!(sessions_for(&c, &sessions).is_empty());
    }

    /// Runs real jj against a repo it creates; skips when jj is absent.
    #[test]
    fn lookup_real_jj() {
        if Command::new("jj").arg("--version").output().is_err() {
            eprintln!("jj not installed; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("peat-vcs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let jj = |args: &[&str]| {
            let out = Command::new("jj")
                .current_dir(&dir)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env("JJ_USER", "t")
                .env("JJ_EMAIL", "t@example.com")
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        jj(&["git", "init"]);
        jj(&[
            "describe",
            "-m",
            "first: one\n\nJj-Workspace: desk-a\nSession: abc-123",
        ]);
        jj(&["new", "-m", "second"]);
        jj(&["bookmark", "create", "-r", "@-", "mark"]);

        let cs = lookup(&dir, "mark").unwrap();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].trailers.session.as_deref(), Some("abc-123"));
        assert_eq!(cs[0].trailers.workspace.as_deref(), Some("desk-a"));
        assert!(cs[0].committer_ms > 1_600_000_000_000);
        // Commit-id and change-id prefixes resolve to the same commit.
        let by_commit = lookup(&dir, &cs[0].commit_id[..12]).unwrap();
        let by_change = lookup(&dir, &cs[0].change_id[..12]).unwrap();
        assert_eq!(by_commit, cs);
        assert_eq!(by_change, cs);
        // Unknown revision: jj's own error.
        let err = lookup(&dir, "nosuchbookmark").unwrap_err();
        assert!(err.contains("nosuchbookmark"), "{err}");
        // A range is newest first.
        let all = lookup(&dir, "::@ ~ root()").unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].description, "second\n");

        // A divergent change id answers with every commit carrying it.
        let op = jj(&["op", "log", "--no-graph", "-n1", "-T", "id"]);
        jj(&["describe", "-r", "@", "-m", "second, edited"]);
        jj(&[
            "--at-op",
            op.trim(),
            "describe",
            "-r",
            "@",
            "-m",
            "second, forked",
        ]);
        let second = &all[0].change_id;
        let div = lookup(&dir, second).unwrap();
        assert_eq!(div.len(), 2, "{div:?}");
        assert!(div.iter().all(|c| &c.change_id == second));
        // A commit rewritten away (hidden) still resolves by commit id.
        assert_eq!(lookup(&dir, &all[0].commit_id).unwrap().len(), 1);

        // The cap: more than LOOKUP_CAP commits are cut and flagged.
        for i in 0..LOOKUP_CAP {
            jj(&["new", "root()", "-m", &format!("n{i}")]);
        }
        let (capped, cut) = lookup_bounded(&dir, "all()").unwrap();
        assert_eq!(capped.len(), LOOKUP_CAP);
        assert!(cut);
        let (few, cut) = lookup_bounded(&dir, "mark").unwrap();
        assert_eq!(few.len(), 1);
        assert!(!cut);

        // Not a jj repo: an error, never a git answer.
        let plain = std::env::temp_dir().join(format!("peat-vcs-plain-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        assert!(lookup(&plain, "@").is_err());
        let _ = std::fs::remove_dir_all(&plain);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trailers_survive_crlf_paragraphs() {
        let t = parse_trailers(
            "subject\r\n\r\nbody line\r\n\r\nSession: abc-123\r\nJj-Workspace: peat-1a\r\n",
        );
        assert_eq!(t.session.as_deref(), Some("abc-123"));
        assert_eq!(t.workspace.as_deref(), Some("peat-1a"));
    }
}
