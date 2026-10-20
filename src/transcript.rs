//! Agent transcripts -> ledger events: one neutral event IR, one adapter
//! per harness format.
//!
//! Formats are detected by POSITIVE signature (a Claude Code line carries
//! `sessionId`; a Codex rollout opens with `type: "session_meta"`) and an
//! unrecognized file is an error, never a guess — a new harness silently
//! parsed as an old one is the worst failure mode. Each adapter makes its
//! lossy editorial choices (what counts as Said, what is skipped) locally
//! and documents them; the IR never learns harness vocabulary.
//!
//! Seq assignment is part of the ledger contract: ids are a pure function
//! of the file (`line_index * 16 + block`), which is what makes re-capture
//! idempotent — adapters may ADD slots in later versions but must never
//! renumber existing ones. Improving an adapter is therefore safe: fix,
//! re-capture, and changed mappings replace while new slots insert.
//!
//! Transcripts are heterogeneous (`mode`, `file-history-snapshot`, hook
//! attachments, `user`/`assistant` messages, compaction summaries) and the
//! format is not ours. The one hard rule: **unknown or unparseable lines are
//! skipped, never fatal** — capture must succeed on a transcript we have
//! never seen, because a session only gets recorded once.
//!
//! Event ids are `(session, line_index * 16 + block_index)`, which is a pure
//! function of the transcript — re-capturing the same file produces the same
//! ids and `upsert` makes the whole operation idempotent.

use serde_json::Value;

use crate::event::{
    DETAIL_CAP, Envelope, Event, EventId, FINAL_MSG_CAP, SAID_CAP, SAID_MIN, USER_MSG_CAP, cap,
};

/// Blocks per transcript line the seq scheme can address.
const SEQ_STRIDE: u32 = 16;

pub struct Parsed {
    pub session: String,
    pub events: Vec<(EventId, Envelope)>,
}

/// Detected transcript format, by positive signature only.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Format {
    ClaudeCode,
    CodexRollout,
}

fn detect(lines: &[Value]) -> Option<Format> {
    if lines
        .iter()
        .take(5)
        .any(|l| l.get("type").and_then(Value::as_str) == Some("session_meta"))
    {
        return Some(Format::CodexRollout);
    }
    if lines.iter().any(|l| l.get("sessionId").is_some()) {
        return Some(Format::ClaudeCode);
    }
    None
}

pub fn parse(jsonl: &str, fallback_session: Option<&str>) -> Option<Parsed> {
    parse_from(jsonl, fallback_session, 0)
}

/// Parse only from raw line `skip` on — the capture-cursor fast path.
/// Transcripts are append-only and seq is a pure function of the raw line
/// index, so the already-ingested prefix needs no JSON parsing at all;
/// format and session id still come from the head. Callers pass a skip
/// backed off by an overlap margin, and idempotent upserts absorb the
/// overlap.
pub fn parse_from(jsonl: &str, fallback_session: Option<&str>, skip: usize) -> Option<Parsed> {
    let head: Vec<Value> = jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(8)
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    // the SessionMeta source is the first cwd-carrying line; when it lies in
    // the skipped prefix it is already in the ledger and must not be re-minted
    // at a different seq. A substring scan of the prefix costs no JSON parsing.
    let meta_in_prefix = skip > 0 && jsonl.lines().take(skip).any(|l| l.contains("\"cwd\""));
    let lines: Vec<(usize, Value)> = jsonl
        .lines()
        .enumerate()
        .filter(|(i, l)| *i >= skip && !l.trim().is_empty())
        .filter_map(|(i, l)| serde_json::from_str(l).ok().map(|v| (i, v)))
        .collect();
    match detect(&head) {
        Some(Format::CodexRollout) => {
            // codex carries its session only in the rollout header
            let head_session = head
                .iter()
                .find(|l| l.get("type").and_then(Value::as_str) == Some("session_meta"))
                .and_then(|l| l.get("payload")?.get("session_id")?.as_str())
                .map(str::to_string);
            return parse_codex(
                &lines,
                head_session.as_deref().or(fallback_session),
                meta_in_prefix,
            );
        }
        Some(Format::ClaudeCode) => {}
        None => {
            // fallback_session lets an empty/unrecognized file still resolve
            // for Claude-style parsing of nothing; a nonempty unknown format
            // must fail loudly
            if !head.is_empty() {
                return None;
            }
        }
    }

    let session = lines
        .iter()
        .find_map(|(_, v)| v.get("sessionId").and_then(Value::as_str))
        .or(fallback_session)?
        .to_string();

    let mut events: Vec<(EventId, Envelope)> = Vec::new();
    let mut last_ts: u64 = 0;
    // tool_use id -> index into `events`, to mark `ok: false` when the
    // matching tool_result reports an error
    let mut call_sites: std::collections::HashMap<String, usize> = Default::default();
    let mut meta_done = meta_in_prefix;
    // borrowed until the end of the loop; capped exactly once when kept
    let mut final_msg: Option<(u32, u64, &str)> = None;

    for (li, line) in lines.iter().map(|(i, v)| (*i, v)) {
        let seq0 = (li as u32) * SEQ_STRIDE;
        let ts = line
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(iso_to_ms)
            .unwrap_or(last_ts);
        last_ts = ts;

        // one SessionMeta from the first line that carries cwd
        if !meta_done && let Some(cwd) = line.get("cwd").and_then(Value::as_str) {
            meta_done = true;
            events.push((
                (session.clone(), seq0),
                Envelope::new(
                    &session,
                    ts,
                    Event::SessionMeta {
                        cwd: cwd.to_string(),
                        branch: line
                            .get("gitBranch")
                            .and_then(Value::as_str)
                            .map(String::from),
                        ese_version: crate::event::ese_version(),
                    },
                ),
            ));
        }

        if line.get("isCompactSummary").and_then(Value::as_bool) == Some(true) {
            events.push((
                (session.clone(), seq0 + 1),
                Envelope::new(&session, ts, Event::Compaction {}),
            ));
            // keep the compactor's distillation itself — it is the summary
            // of everything the context window lost
            if let Some(text) = line
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str)
            {
                events.push((
                    (session.clone(), seq0 + 2),
                    Envelope::new(
                        &session,
                        ts,
                        Event::CompactSummary {
                            text: cap(text, FINAL_MSG_CAP),
                        },
                    ),
                ));
            }
            continue;
        }

        let ty = line.get("type").and_then(Value::as_str).unwrap_or("");
        if ty != "user" && ty != "assistant" {
            continue;
        }
        let Some(msg) = line.get("message") else {
            continue;
        };

        // content is either a plain string (user prompts) or a block array
        match msg.get("content") {
            Some(Value::String(text)) if ty == "user" => {
                events.push((
                    (session.clone(), seq0 + 2),
                    Envelope::new(
                        &session,
                        ts,
                        Event::UserMsg {
                            text: cap(text, USER_MSG_CAP),
                        },
                    ),
                ));
            }
            Some(Value::Array(blocks)) => {
                for (bi, block) in blocks.iter().enumerate().take(SEQ_STRIDE as usize - 2) {
                    let seq = seq0 + 2 + bi as u32;
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") if ty == "user" => {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                events.push((
                                    (session.clone(), seq),
                                    Envelope::new(
                                        &session,
                                        ts,
                                        Event::UserMsg {
                                            text: cap(text, USER_MSG_CAP),
                                        },
                                    ),
                                ));
                            }
                        }
                        Some("text") if ty == "assistant" => {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                // substantive assistant messages are recallable
                                if text.len() >= SAID_MIN {
                                    events.push((
                                        (session.clone(), seq),
                                        Envelope::new(
                                            &session,
                                            ts,
                                            Event::Said {
                                                text: cap(text, SAID_CAP),
                                            },
                                        ),
                                    ));
                                }
                                // remember the last assistant text: it becomes
                                // FinalMsg (replacing its Said at the same seq)
                                final_msg = Some((seq, ts, text));
                            }
                        }
                        Some("tool_use") => {
                            let tool = block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown");
                            // borrow: tool inputs can be arbitrarily large
                            // (Edit old/new strings, MCP payloads)
                            let input = block.get("input").unwrap_or(&Value::Null);
                            let detail = tool_detail(tool, input);
                            if let Some(id) = block.get("id").and_then(Value::as_str) {
                                call_sites.insert(id.to_string(), events.len());
                            }
                            events.push((
                                (session.clone(), seq),
                                Envelope::new(
                                    &session,
                                    ts,
                                    Event::ToolCall {
                                        tool: tool.to_string(),
                                        detail: cap(&detail, DETAIL_CAP),
                                        ok: true,
                                    },
                                ),
                            ));
                            // Edit/Write-family calls also touch a file
                            if let Some(path) = file_touch(tool, input) {
                                events.push((
                                    (session.clone(), seq0 + SEQ_STRIDE - 1),
                                    Envelope::new(&session, ts, Event::FileTouch { path }),
                                ));
                            }
                        }
                        Some("tool_result") => {
                            let err = block.get("is_error").and_then(Value::as_bool) == Some(true)
                                || line
                                    .get("toolUseResult")
                                    .and_then(|r| r.get("is_error"))
                                    .and_then(Value::as_bool)
                                    == Some(true);
                            if err
                                && let Some(id) = block.get("tool_use_id").and_then(Value::as_str)
                                && let Some(&i) = call_sites.get(id)
                                && let Event::ToolCall { ok, .. } = &mut events[i].1.kind
                            {
                                *ok = false;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    if let Some((seq, ts, text)) = final_msg {
        // the closing message is FinalMsg, not Said — drop the duplicate
        events.retain(|(id, e)| !(id.1 == seq && matches!(e.kind, Event::Said { .. })));
        events.push((
            (session.clone(), seq),
            Envelope::new(
                &session,
                ts,
                Event::FinalMsg {
                    text: cap(text, FINAL_MSG_CAP),
                },
            ),
        ));
    }

    Some(Parsed { session, events })
}

fn tool_detail(tool: &str, input: &Value) -> String {
    match tool {
        "Bash" => input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "Read" | "Edit" | "Write" | "NotebookEdit" => input
            .get("file_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => {
            let s = input.to_string();
            if s == "null" { String::new() } else { s }
        }
    }
}

fn file_touch(tool: &str, input: &Value) -> Option<String> {
    matches!(tool, "Edit" | "Write" | "NotebookEdit")
        .then(|| input.get("file_path")?.as_str().map(String::from))
        .flatten()
}

/// Openings that mark what is left of a user-role message as text a
/// harness or another agent put there: inter-agent envelopes (hail, and
/// the tmux-bridge and `[from …]` forms before it), system notices,
/// Claude Code's interrupt marker and instruction-file injection.
const HARNESS_OPENINGS: &[&str] = &[
    "[hail ",
    "[from ",
    "[tmux-bridge ",
    "[SYSTEM",
    "[Request interrupted",
    "Caveat:",
    "# AGENTS.md instructions",
    "Codebase and user instructions are shown below",
];

/// Tags a harness wraps its own text in, as found in real ledgers (Claude
/// Code, Codex, hooks). Any other tag is the person's — they write
/// `<feedback>`, `<ctx>`, `<2a>` and paste `<pasted_content>` themselves.
const HARNESS_TAGS: &[&str] = &[
    "system-reminder",
    "task-notification",
    "subagent_notification",
    "environment_context",
    "codex_internal_context",
    "user_instructions",
    "turn_aborted",
    "skill",
    "user-prompt-submit-hook",
    "pre-compact",
    "ide_opened_file",
    "ide_selection",
];

/// Harness tag families: `local-command-stdout`, `bash-input`, …
const HARNESS_TAG_FAMILIES: &[&str] = &["local-command-", "bash-"];

/// The name of the tag `t` opens with, if it opens with one.
fn leading_tag(t: &str) -> Option<&str> {
    let rest = t.strip_prefix('<')?;
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// Where an inter-agent envelope interrupts the person's typing: the
/// strict header forms only (`[hail kind:K from:…`, `[tmux-bridge from:…
/// pane:%…`), so a message that merely mentions `[hail ...]` keeps it.
fn embedded_envelope(t: &str) -> Option<usize> {
    let hail = t.match_indices("[hail kind:").find(|(i, m)| {
        let rest = &t[i + m.len()..];
        let kind = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(0);
        kind > 0 && rest[kind..].starts_with(" from:")
    });
    let bridge = t.match_indices("[tmux-bridge from:").find(|(i, _)| {
        let head = &t[*i..];
        head.find(']')
            .is_some_and(|end| head[..end].contains(" pane:%"))
    });
    [hail, bridge].into_iter().flatten().map(|(i, _)| i).min()
}

/// The person's own words in a user-role message, or `None` when a
/// harness or another agent put all of it there.
///
/// - Leading harness blocks ([`HARNESS_TAGS`]) are stripped and what
///   follows is kept: a reminder the harness prepends does not unmake the
///   words typed after it. An unclosed harness block is all harness.
/// - A slash command or skill invocation is the harness's text around the
///   person's arguments; those arguments are kept.
/// - An attached image (`<image …></image>`) is the person's: the tag is
///   dropped, the typed text kept. Any tag not on the list is kept as is.
/// - An envelope that landed mid-message (another agent's reply arriving
///   while the person typed) is cut, with everything after it; the
///   person's text before it is kept.
///
/// Pure, and applied at read and index time only: the ledger keeps the
/// raw text, so a better rule here is a view rebuild, never a re-capture.
pub fn persons_words(text: &str) -> Option<String> {
    let mut t = text.trim_start();
    while let Some(name) = leading_tag(t) {
        if name.starts_with("command-") {
            // a slash command: the harness wraps the person's words in tags
            let (_, rest) = t.split_once("<command-args>")?;
            return before_envelope(rest.split("</command-args>").next().unwrap_or(""));
        }
        let close = format!("</{name}>");
        t = if name == "image" {
            // `<image …> </image>`, or a bare opening tag
            match t.find(&close) {
                Some(i) => &t[i + close.len()..],
                None => &t[t.find('>').map_or(t.len(), |i| i + 1)..],
            }
        } else if HARNESS_TAGS.contains(&name)
            || HARNESS_TAG_FAMILIES.iter().any(|f| name.starts_with(f))
        {
            &t[t.find(&close)? + close.len()..]
        } else {
            break;
        }
        .trim_start();
    }
    if t.starts_with("Base directory for this skill") {
        return t
            .rsplit_once("ARGUMENTS:")
            .map(|(_, a)| a.trim().to_string())
            .filter(|a| !a.is_empty());
    }
    if HARNESS_OPENINGS.iter().any(|o| t.starts_with(o)) {
        return None;
    }
    before_envelope(t)
}

/// `t` up to any envelope that interrupted it, trimmed; `None` if empty.
fn before_envelope(t: &str) -> Option<String> {
    let t = embedded_envelope(t).map_or(t, |i| &t[..i]).trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Replace any parsed `FinalMsg` with the hook-provided closing message
/// (`Stop` passes `last_assistant_message`, which is authoritative — the
/// transcript file may lag the final turn).
pub fn override_final_msg(parsed: &mut Parsed, text: &str) {
    parsed
        .events
        .retain(|(_, e)| !matches!(e.kind, Event::FinalMsg { .. }));
    let ts = parsed
        .events
        .iter()
        .map(|(_, e)| e.ts_ms)
        .max()
        .unwrap_or(0);
    parsed.events.push((
        (parsed.session.clone(), crate::event::HOOK_FINAL_SEQ),
        Envelope::new(
            &parsed.session,
            ts,
            Event::FinalMsg {
                text: cap(text, FINAL_MSG_CAP),
            },
        ),
    ));
}

/// "2026-08-10T19:18:11.311Z" -> unix ms. Hand-rolled to keep peat
/// dependency-light; returns None on anything malformed.
pub fn iso_to_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<u64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let ms = if b.get(19) == Some(&b'.') {
        num(20..23).unwrap_or(0)
    } else {
        0
    };
    // days-from-civil (Howard Hinnant), valid for all dates we will ever see
    let (y, mo) = if mo <= 2 { (y - 1, mo + 12) } else { (y, mo) };
    let era = y / 400;
    let yoe = y - era * 400;
    let doy = (153 * (mo - 3) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + sec * 1000 + ms)
}

/// Unix ms -> "YYYY-MM-DD" (UTC). Inverse of [`iso_to_ms`]'s date part;
/// the civil-days constants are stated only in this file.
pub fn date_label(ms: u64) -> String {
    let days = ms / 86_400_000;
    let era = (days + 719_468) / 146_097;
    let doe = days + 719_468 - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Local UTC offset in ms via `date +%z`. Render/capture boundary only —
/// the fold path never sees timezones. Zero (UTC semantics) on failure.
pub fn local_offset_ms() -> i64 {
    let Ok(out) = std::process::Command::new("date").arg("+%z").output() else {
        return 0;
    };
    let s = String::from_utf8_lossy(&out.stdout);
    let s = s.trim();
    if s.len() != 5 {
        return 0;
    }
    let sign = if s.starts_with('-') { -1 } else { 1 };
    match (s[1..3].parse::<i64>(), s[3..5].parse::<i64>()) {
        (Ok(h), Ok(m)) => sign * (h * 60 + m) * 60_000,
        _ => 0,
    }
}

/// A local-day instant for `date` (YYYY-MM-DD) at `time` ("12:00:00.000"
/// or "23:59:59.999"): shared by `obs --at` and `asof`.
pub fn local_day_ms(date: &str, time: &str) -> Option<u64> {
    iso_to_ms(&format!("{date}T{time}Z")).map(|utc| (utc as i64 - local_offset_ms()) as u64)
}

/// Codex CLI rollout adapter (`~/.codex/sessions/**/rollout-*.jsonl`).
///
/// Lines are `{timestamp, type, payload}`. Editorial choices, made here
/// and nowhere else: `reasoning` items and `developer`-role messages are
/// skipped (chain-of-thought and injected instructions, not the work);
/// `function_call` and `custom_tool_call` are both ToolCalls with the
/// command extracted from the arguments JSON when present; `compacted`
/// yields the marker plus the compactor's own summary text.
fn parse_codex(
    lines: &[(usize, Value)],
    fallback_session: Option<&str>,
    meta_in_prefix: bool,
) -> Option<Parsed> {
    let session = lines
        .iter()
        .find(|(_, l)| l.get("type").and_then(Value::as_str) == Some("session_meta"))
        .and_then(|(_, l)| l.get("payload")?.get("session_id")?.as_str())
        .or(fallback_session)?
        .to_string();

    let mut events: Vec<(EventId, Envelope)> = Vec::new();
    let mut last_ts: u64 = 0;
    let mut meta_done = meta_in_prefix;
    let mut final_msg: Option<(u32, u64, &str)> = None;

    for (li, line) in lines.iter().map(|(i, v)| (*i, v)) {
        let seq = (li as u32) * SEQ_STRIDE + 2;
        let ts = line
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(iso_to_ms)
            .unwrap_or(last_ts);
        last_ts = ts;
        let ty = line.get("type").and_then(Value::as_str).unwrap_or("");
        let Some(payload) = line.get("payload") else {
            continue;
        };

        match ty {
            "session_meta" if !meta_done => {
                meta_done = true;
                events.push((
                    (session.clone(), seq),
                    Envelope::new(
                        &session,
                        ts,
                        Event::SessionMeta {
                            cwd: payload
                                .get("cwd")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            branch: None,
                            ese_version: crate::event::ese_version(),
                        },
                    ),
                ));
            }
            "compacted" => {
                events.push((
                    (session.clone(), seq),
                    Envelope::new(&session, ts, Event::Compaction {}),
                ));
                // the summary is payload.message when present. Codex
                // encrypts the real one (the last replacement_history item,
                // type `compaction`, has no readable text), so there is
                // usually none; the first replacement item is the session's
                // opening prompt, not a summary, and must not stand in.
                let text = payload
                    .get("message")
                    .and_then(Value::as_str)
                    .filter(|t| !t.trim().is_empty());
                if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
                    events.push((
                        (session.clone(), seq + 1),
                        Envelope::new(
                            &session,
                            ts,
                            Event::CompactSummary {
                                text: cap(text, FINAL_MSG_CAP),
                            },
                        ),
                    ));
                }
            }
            "response_item" => match payload.get("type").and_then(Value::as_str) {
                Some("message") => {
                    let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
                    let text = payload
                        .get("content")
                        .and_then(Value::as_array)
                        .map(|blocks| {
                            blocks
                                .iter()
                                .filter_map(|b| b.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default();
                    if text.trim().is_empty() {
                        continue;
                    }
                    match role {
                        "user" => events.push((
                            (session.clone(), seq),
                            Envelope::new(
                                &session,
                                ts,
                                Event::UserMsg {
                                    text: cap(&text, USER_MSG_CAP),
                                },
                            ),
                        )),
                        "assistant" => {
                            if text.len() >= SAID_MIN {
                                events.push((
                                    (session.clone(), seq),
                                    Envelope::new(
                                        &session,
                                        ts,
                                        Event::Said {
                                            text: cap(&text, SAID_CAP),
                                        },
                                    ),
                                ));
                            }
                            // borrow for the FinalMsg swap at the end
                            if let Some(t) = payload
                                .get("content")
                                .and_then(Value::as_array)
                                .and_then(|b| b.first())
                                .and_then(|b| b.get("text"))
                                .and_then(Value::as_str)
                            {
                                final_msg = Some((seq, ts, t));
                            }
                        }
                        _ => {} // developer role: injected instructions, skipped
                    }
                }
                Some("function_call") | Some("custom_tool_call") => {
                    let tool = payload
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    // arguments is a JSON string; the command usually lives
                    // under "cmd" or "command"
                    let args = payload.get("arguments").and_then(Value::as_str);
                    let cmd: Option<String> = args
                        .and_then(|a| serde_json::from_str::<Value>(a).ok())
                        .and_then(|v| {
                            v.get("cmd")
                                .or_else(|| v.get("command"))
                                .and_then(Value::as_str)
                                .map(String::from)
                        });
                    let detail = cmd.or_else(|| args.map(String::from)).unwrap_or_default();
                    events.push((
                        (session.clone(), seq),
                        Envelope::new(
                            &session,
                            ts,
                            Event::ToolCall {
                                tool: tool.to_string(),
                                detail: cap(&detail, DETAIL_CAP),
                                ok: true,
                            },
                        ),
                    ));
                }
                _ => {} // reasoning, outputs, agent_message: skipped
            },
            _ => {} // event_msg, turn_context, world_state, ...
        }
    }

    if let Some((seq, ts, text)) = final_msg {
        events.retain(|(id, e)| !(id.1 == seq && matches!(e.kind, Event::Said { .. })));
        events.push((
            (session.clone(), seq),
            Envelope::new(
                &session,
                ts,
                Event::FinalMsg {
                    text: cap(text, FINAL_MSG_CAP),
                },
            ),
        ));
    }

    Some(Parsed { session, events })
}
