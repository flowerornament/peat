//! `peat <ref>`: the sessions behind whatever an agent is looking at — a
//! commit or change id, a bead id (or any literal token the work mentions),
//! or a file path — and the moment in each: what the person said around
//! it, how the session closed, and the commands that read the whole thing.
//!
//! Commits resolve through jj at read time (the `Session:` trailer each
//! repo's jj-identity hook stamps), never through capture: see
//! `.design/2026-10-19-peat-is-the-record.md`.

use std::path::Path;

use crate::db::Db;
use crate::event::EventId;
use crate::pipeline::{SessStats, TextRow};
use crate::ui::{self, age_label, clip, short_sess};
use crate::vcs;

/// What an argument names, by its shape.
#[derive(Debug, PartialEq)]
pub enum Target {
    /// A commit id prefix (hex) or a jj change id (k–z letters).
    Change(String),
    /// A path to a file sessions may have edited.
    File(String),
    /// A literal token sessions may have written: a bead id, a ticket.
    Token(String),
}

/// Classify one argument. `None` when it has no ref shape (the caller
/// searches instead). A hex string is offered as a change only after the
/// caller found no session with that prefix.
pub fn classify(arg: &str, desk: &Path) -> Option<Target> {
    let a = arg.trim();
    let here = std::env::current_dir().unwrap_or_else(|_| desk.to_path_buf());
    if a.contains('/') || here.join(a).is_file() {
        return Some(Target::File(a.to_string()));
    }
    let hex = a.len() >= 6 && a.len() <= 40 && a.bytes().all(|b| b.is_ascii_hexdigit());
    // jj change ids use the k–z alphabet; eight letters keeps ordinary
    // words ("sort", "tops") out
    let change = a.len() >= 8 && a.len() <= 32 && a.bytes().all(|b| (b'k'..=b'z').contains(&b));
    if hex || change {
        return Some(Target::Change(a.to_ascii_lowercase()));
    }
    if is_token(a, desk) {
        return Some(Target::Token(a.to_string()));
    }
    None
}

/// A bead id or the like: `prefix-id` with an optional `.n` tail
/// (`peat-520.4`, `herald-8reeh`). The id must carry a digit, or the prefix
/// must be this repository's name (bd's default prefix), so hyphenated
/// words (`dry-run`, `follow-up`) stay with search.
fn is_token(a: &str, desk: &Path) -> bool {
    let Some((head, tail)) = a.split_once('-') else {
        return false;
    };
    let id = tail.split('.').next().unwrap_or("");
    let shaped = head.len() >= 2
        && head
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && head.as_bytes()[0].is_ascii_lowercase()
        && (2..=8).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && tail
            .split('.')
            .skip(1)
            .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let repo = crate::db::worktree_anchor(desk).unwrap_or_else(|| desk.to_path_buf());
    let ours = repo.file_name().is_some_and(|n| n == head);
    shaped && (id.bytes().any(|b| b.is_ascii_digit()) || ours)
}

/// How many sessions one read lists before it points at search.
const SESSIONS_SHOWN: usize = 8;

/// One session behind a ref, with the moment that ties it to the ref.
#[derive(serde::Serialize)]
struct Hit {
    session: String,
    place: String,
    /// how the session was tied to the ref: "trailer", "place and time",
    /// "edited", "mentioned"
    basis: String,
    end_ms: u64,
    /// when the ref happened in this session (`asof` reads that day)
    at_ms: u64,
    closing: String,
    /// the person's words (or the session's mentions) at the moment
    moment: Vec<Line>,
}

#[derive(serde::Serialize)]
struct Line {
    seq: u32,
    ts_ms: u64,
    kind: String,
    text: String,
}

/// Resolve and print. Returns false when the target resolves to nothing,
/// so the caller can fall back to search.
pub fn show(st: &Db, target: &Target, desk: &Path, now: u64, json: bool) -> bool {
    let arg = match target {
        Target::Change(a) | Target::File(a) | Target::Token(a) => a.clone(),
    };
    let (title, hits) = match target {
        Target::Change(rev) => match changes(st, rev, desk) {
            Some(r) => r,
            None => return false,
        },
        Target::File(path) => files(st, path, desk),
        Target::Token(tok) => tokens(st, tok),
    };
    if hits.is_empty() && !matches!(target, Target::Change(_)) {
        return false;
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"target": title, "sessions": hits}))
                .unwrap()
        );
        return true;
    }
    println!("{}", ui::h1(&format!("== {title} ==")));
    if hits.is_empty() {
        println!(
            "  {}",
            ui::dim(
                "no captured session made this (no Session: trailer, and no desk session spans its time)"
            )
        );
    }
    for h in hits.iter().take(SESSIONS_SHOWN) {
        println!(
            "\n{} {}",
            ui::accent(&format!("{} · {}", short_sess(&h.session), h.place)),
            ui::dim(&format!(
                "({}, active until {} ago)",
                h.basis,
                age_label(now, h.end_ms)
            ))
        );
        for l in &h.moment {
            println!(
                "  {} {}  {}",
                ui::dim(&format!("[{} · {}]", l.kind, age_label(now, l.ts_ms))),
                clip(&l.text, 240),
                ui::dim(&format!("▸ peat {} {}", short_sess(&h.session), l.seq))
            );
        }
        if !h.closing.is_empty() {
            println!(
                "  {} {}  {}",
                ui::dim("closed with:"),
                clip(&h.closing, 300),
                ui::dim(&format!("▸ peat {}", short_sess(&h.session)))
            );
        }
        println!(
            "  {}",
            ui::dim(&format!(
                "▸ peat {}    ▸ peat asof {}",
                short_sess(&h.session),
                crate::transcript::date_label(h.at_ms)
            ))
        );
    }
    if hits.len() > SESSIONS_SHOWN {
        println!(
            "\n{}",
            ui::dim(&format!(
                "… {} more sessions  ▸ peat ref --json {arg}",
                hits.len() - SESSIONS_SHOWN
            ))
        );
    }
    true
}

/// Every session row, newest first.
fn sessions(st: &Db) -> Vec<(String, SessStats)> {
    let mut s: Vec<(String, SessStats)> =
        st.rtx(|(_, _, _, _, sessions, _)| sessions.iter().collect());
    s.sort_by_key(|(_, s)| std::cmp::Reverse(s.end_ms));
    s
}

/// The text rows of the given sessions, oldest first — one scan of the
/// text table, which holds only what people and agents wrote.
fn texts_of(st: &Db, want: &[&str]) -> Vec<(EventId, TextRow)> {
    let mut rows: Vec<(EventId, TextRow)> = st.rtx(|(_, _, (_, _, texts), _, _, _)| {
        texts
            .iter()
            .filter(|((s, _), _): &(EventId, TextRow)| want.contains(&s.as_str()))
            .collect()
    });
    rows.sort_by_key(|(_, t)| t.ts_ms);
    rows
}

fn place(cwd: &str) -> String {
    cwd.rsplit('/').next().unwrap_or(cwd).to_string()
}

/// The person's last two messages at or before `at` in one session: why
/// the session did what it did at that moment.
fn said_before(rows: &[(EventId, TextRow)], session: &str, at: u64) -> Vec<Line> {
    let mut out: Vec<Line> = rows
        .iter()
        .filter(|((s, _), t)| s == session && t.kind == "user" && t.ts_ms <= at)
        .rev()
        .filter_map(|((_, seq), t)| {
            Some(Line {
                seq: *seq,
                ts_ms: t.ts_ms,
                kind: "the person".into(),
                text: persons_words(&t.text)?,
            })
        })
        .take(2)
        .collect();
    out.reverse();
    out
}

/// The person's own words in a user-role message, or `None` when a
/// harness or another agent put it there (tags, hail envelopes, system
/// notices). A skill invocation is the harness's text with the person's
/// words in its trailing `ARGUMENTS:` line; those words are kept.
fn persons_words(text: &str) -> Option<String> {
    let t = text.trim_start();
    // a slash command: the harness wraps the person's words in tags
    if let Some((_, rest)) = t.split_once("<command-args>") {
        let args = rest.split("</command-args>").next().unwrap_or("").trim();
        return (!args.is_empty()).then(|| args.to_string());
    }
    if t.starts_with("Base directory for this skill") {
        return t
            .rsplit_once("ARGUMENTS:")
            .map(|(_, a)| a.trim().to_string())
            .filter(|a| !a.is_empty());
    }
    let harness = t.starts_with('<')
        || t.starts_with("[hail ")
        || t.starts_with("[SYSTEM")
        || t.starts_with("Caveat:");
    (!harness).then(|| text.to_string())
}

/// What the agent said first at or after `at`: its account of the change.
/// The session's own closing message is often days later.
fn agent_after(rows: &[(EventId, TextRow)], session: &str, at: u64) -> Option<Line> {
    rows.iter()
        .find(|((s, _), t)| {
            s == session && (t.kind == "said" || t.kind == "final") && t.ts_ms >= at
        })
        .map(|((_, seq), t)| Line {
            seq: *seq,
            ts_ms: t.ts_ms,
            kind: "the agent".into(),
            text: t.text.clone(),
        })
}

fn changes(st: &Db, rev: &str, desk: &Path) -> Option<(String, Vec<Hit>)> {
    let (found, cut) = vcs::lookup_bounded(desk, rev).ok()?;
    let first = found.first()?;
    let title = if found.len() == 1 {
        let subject = first.description.lines().next().unwrap_or("").trim();
        format!(
            "{} · change {} · {}",
            &first.commit_id[..first.commit_id.len().min(12)],
            &first.change_id[..first.change_id.len().min(8)],
            clip(subject, 80)
        )
    } else {
        format!(
            "{rev}: {} commits{}",
            found.len(),
            if cut { ", newest shown" } else { "" }
        )
    };
    let sess = sessions(st);
    let spans: Vec<(String, vcs::SessionSpan)> = sess
        .iter()
        .map(|(id, s)| {
            (
                id.clone(),
                vcs::SessionSpan {
                    cwd: s.cwd.clone(),
                    start_ms: s.start_ms,
                    end_ms: s.end_ms,
                },
            )
        })
        .collect();
    let mut tied: Vec<(String, vcs::Basis, u64)> = Vec::new();
    for c in &found {
        for (id, basis) in vcs::sessions_for(c, &spans) {
            if !tied.iter().any(|(t, _, _)| *t == id) {
                tied.push((id, basis, c.committer_ms));
            }
        }
    }
    let want: Vec<&str> = tied.iter().map(|(id, _, _)| id.as_str()).collect();
    let rows = texts_of(st, &want);
    let hits = tied
        .iter()
        .filter_map(|(id, basis, at)| {
            let (_, s) = sess.iter().find(|(sid, _)| sid == id)?;
            // committer time is the change's last rewrite: for a landed
            // commit, its final describe — so the moment sits at the commit
            let at = (*at).min(s.end_ms);
            let mut moment = said_before(&rows, id, at);
            moment.extend(agent_after(&rows, id, at));
            Some(Hit {
                session: id.clone(),
                place: place(&s.cwd),
                basis: match basis {
                    vcs::Basis::Trailer => "Session: trailer".into(),
                    vcs::Basis::PlaceAndTime => "workspace and time".into(),
                },
                end_ms: s.end_ms,
                at_ms: at,
                // the session's own closing is printed only when the
                // moment has no account of the change
                closing: if moment.iter().any(|l| l.kind == "the agent") {
                    String::new()
                } else {
                    s.final_msg.clone()
                },
                moment,
            })
        })
        .collect();
    Some((title, hits))
}

/// Sessions that edited `path` in this repository. The index holds the
/// absolute paths sessions wrote to, and the same file has one per
/// workspace, so the path is made relative to the workspace it lies in and
/// tried under every workspace root of this repo — never under another
/// repo's directory, where a file of the same name is a different file.
fn files(st: &Db, path: &str, desk: &Path) -> (String, Vec<Hit>) {
    let here = std::env::current_dir().unwrap_or_else(|_| desk.to_path_buf());
    let abs = normalize(&here.join(path));
    let mut roots = vcs::workspace_roots(desk);
    if roots.is_empty() {
        roots.push(desk.to_path_buf());
    }
    let rel = roots
        .iter()
        .find_map(|r| abs.strip_prefix(r).ok().map(Path::to_path_buf));
    let mut candidates: Vec<String> = vec![abs.display().to_string()];
    if let Some(rel) = &rel {
        for r in &roots {
            let c = r.join(rel).display().to_string();
            if !candidates.contains(&c) {
                candidates.push(c);
            }
        }
    }
    let sess = sessions(st);
    let mut touched: Vec<String> =
        st.rtx(|(_, files, _, _, _, _)| candidates.iter().flat_map(|c| files.get(c)).collect());
    touched.sort();
    touched.dedup();
    let hits = sess
        .iter()
        .filter(|(id, _)| touched.contains(id))
        .map(|(id, s)| Hit {
            session: id.clone(),
            place: place(&s.cwd),
            basis: "edited".into(),
            end_ms: s.end_ms,
            at_ms: s.end_ms,
            closing: s.final_msg.clone(),
            moment: Vec::new(),
        })
        .collect();
    let title = rel.unwrap_or(abs).display().to_string();
    (title, hits)
}

/// Sessions whose text contains `tok` literally (case-insensitive), with
/// the first and the latest mention in each.
fn tokens(st: &Db, tok: &str) -> (String, Vec<Hit>) {
    let needle = tok.to_lowercase();
    let mut rows: Vec<(EventId, TextRow)> = st.rtx(|(_, _, (_, _, texts), _, _, _)| {
        texts
            .iter()
            .filter(|(_, t): &(EventId, TextRow)| t.text.to_lowercase().contains(&needle))
            .collect()
    });
    rows.sort_by_key(|(_, t)| t.ts_ms);
    let sess = sessions(st);
    let hits = sess
        .iter()
        .filter_map(|(id, s)| {
            let mine: Vec<&(EventId, TextRow)> =
                rows.iter().filter(|((sid, _), _)| sid == id).collect();
            let first = mine.first()?;
            let mut moment = vec![line(first)];
            if mine.len() > 1 {
                moment.push(line(mine.last().unwrap()));
            }
            Some(Hit {
                session: id.clone(),
                place: place(&s.cwd),
                basis: format!("mentioned {}×", mine.len()),
                end_ms: s.end_ms,
                at_ms: first.1.ts_ms,
                closing: s.final_msg.clone(),
                moment,
            })
        })
        .collect();
    (tok.to_string(), hits)
}

/// Lexically resolve `.` and `..` (the file may not exist any more).
fn normalize(p: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
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

fn line(((_, seq), t): &(EventId, TextRow)) -> Line {
    Line {
        seq: *seq,
        ts_ms: t.ts_ms,
        kind: t.kind.clone(),
        text: t.text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_classify() {
        let desk = Path::new("/nonexistent-desk");
        assert_eq!(
            classify("1bdaa47b", desk),
            Some(Target::Change("1bdaa47b".into()))
        );
        assert_eq!(
            classify("nmnrrusvqk", desk),
            Some(Target::Change("nmnrrusvqk".into()))
        );
        assert_eq!(
            classify("src/hook.rs", desk),
            Some(Target::File("src/hook.rs".into()))
        );
        for t in ["peat-520.4", "herald-8reeh", "murail-hbh8l"] {
            assert_eq!(classify(t, desk), Some(Target::Token(t.into())), "{t}");
        }
        for w in [
            "sort",
            "fold",
            "hook-contract-rule",
            "a-b",
            "peat-",
            "x",
            "dry-run",
            "follow-up",
            "peat-axn",
        ] {
            assert_eq!(classify(w, desk), None, "{w}");
        }
    }

    #[test]
    fn said_before_takes_the_last_two_user_lines_up_to_the_moment() {
        let row = |seq: u32, ts: u64, kind: &str, text: &str| {
            (
                ("s".to_string(), seq),
                TextRow {
                    text: text.into(),
                    kind: kind.into(),
                    ts_ms: ts,
                    cited: true,
                },
            )
        };
        let rows = vec![
            row(1, 10, "user", "a"),
            row(2, 20, "said", "agent"),
            row(3, 30, "user", "b"),
            row(4, 40, "user", "c"),
            row(5, 50, "user", "after"),
        ];
        let got: Vec<String> = said_before(&rows, "s", 45)
            .into_iter()
            .map(|l| l.text)
            .collect();
        assert_eq!(got, vec!["b", "c"]);
        assert!(said_before(&rows, "other", 45).is_empty());
    }

    #[test]
    fn only_the_persons_words_count() {
        assert_eq!(
            persons_words("fix the gate").as_deref(),
            Some("fix the gate")
        );
        assert_eq!(
            persons_words("Base directory for this skill: /x\n# T\nARGUMENTS: move peat onto jj")
                .as_deref(),
            Some("move peat onto jj")
        );
        assert_eq!(
            persons_words("<command-name>/template</command-name>\n<command-args>move peat onto jj</command-args>")
                .as_deref(),
            Some("move peat onto jj")
        );
        for h in [
            "Base directory for this skill: /x\nno args",
            "<task-notification>x",
            "[hail kind:fyi] x",
        ] {
            assert_eq!(persons_words(h), None, "{h}");
        }
    }
}
