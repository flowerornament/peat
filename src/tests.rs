//! The oracles peat's claims rest on, plus parser and idempotency checks.
//!
//! Oracle 1 (retraction): revising an indexed text must make the old text
//! unfindable in both the keyword and vector indexes. The `#[ignore]`d twin
//! proves the oracle is red-capable — it asserts the OPPOSITE and must fail
//! when run (`cargo test -p peat -- --ignored` shows exactly one failure).
//!
//! Oracle 2 (replay determinism): folding any prefix of the ledger must
//! yield exactly the views that an independent scan of that prefix
//! predicts. This is what makes time travel (`--asof`-style replay) a fact
//! rather than a hope.

use std::collections::BTreeMap;

use fold::stream::KeyedStream;

use crate::event::{Envelope, Event, EventId, OBS_SEQ_BASE};
use crate::pipeline::{DAY_MS, DayStats, SLOT_MS, SubjStats};

fn tmp() -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("peat-test-{}-{n}.db", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A bare stream over peat's pipeline (no lock wait, no watchdog).
fn open(
    path: impl Into<std::path::PathBuf>,
) -> KeyedStream<EventId, Envelope, crate::pipeline::Pipeline> {
    KeyedStream::new(path.into(), crate::pipeline::pipeline())
}

fn obs(session: &str, n: u32, ts: u64, subject: &str, text: &str) -> (EventId, Envelope) {
    (
        (session.to_string(), OBS_SEQ_BASE + n),
        Envelope::new(
            session,
            ts,
            Event::Obs {
                subject: subject.to_string(),
                text: text.to_string(),
                derived_from: vec![],
            },
        ),
    )
}

fn tool(session: &str, n: u32, ts: u64, ok: bool) -> (EventId, Envelope) {
    (
        (session.to_string(), n),
        Envelope::new(
            session,
            ts,
            Event::ToolCall {
                tool: "Bash".into(),
                detail: format!("cmd-{n}"),
                ok,
            },
        ),
    )
}

// ---------------------------------------------------------------- oracle 1

/// Keyword hits for `probe` (exact posting semantics — text-level).
macro_rules! find_kw {
    ($st:expr, $probe:expr) => {
        $st.rtx(|(_, _, (kw, _, _), _, _, _)| {
            kw.search($probe, 10)
                .into_iter()
                .map(|h| h.val)
                .collect::<Vec<EventId>>()
        })
    };
}

/// Nearest neighbor for `probe`. NOTE: HNSW always returns the nearest
/// vectors regardless of absolute distance, so "old text unfindable" is
/// only well-defined relative to a control document that actually carries
/// the old text — the revised doc must lose to the control.
macro_rules! nearest {
    ($st:expr, $probe:expr) => {
        $st.rtx(|(_, _, (_, vec, _), _, _, _)| {
            vec.search(&ese::encode_single($probe))
                .first()
                .map(|h| h.val.clone())
        })
    };
}

#[test]
fn retraction_makes_old_text_unfindable() {
    let path = tmp();
    let mut st = open(&path);
    let id = ("s1".to_string(), OBS_SEQ_BASE);
    let control = ("s1".to_string(), OBS_SEQ_BASE + 1);

    st.wtx(|tx| {
        tx.upsert(
            &id,
            &obs("s1", 0, 1000, "staging", "runs on the raspberry pi").1,
        );
        // control: keeps carrying the old text so vector-nearest is decidable
        tx.upsert(
            &control,
            &obs(
                "s1",
                1,
                1000,
                "hardware",
                "a raspberry pi lives under the desk",
            )
            .1,
        );
    });
    assert!(
        find_kw!(st, "raspberry").contains(&id),
        "sanity: old text must be keyword-indexed before revision"
    );

    // revise: same event id, new text — one transaction
    st.wtx(|tx| {
        tx.upsert(&id, &obs("s1", 0, 2000, "staging", "moved to a cloud vm").1);
    });

    assert!(
        !find_kw!(st, "raspberry").contains(&id),
        "old text still keyword-findable after revision"
    );
    assert!(find_kw!(st, "cloud vm").contains(&id));
    assert_eq!(
        nearest!(st, "raspberry pi under a desk"),
        Some(control.clone()),
        "revised doc still wins vector search for its OLD text"
    );
    assert_eq!(nearest!(st, "moved to a cloud vm"), Some(id.clone()));

    // and the property must survive a reopen (hnsw rebuild path)
    drop(st);
    let st = open(&path);
    assert!(!find_kw!(st, "raspberry").contains(&id));
    assert_eq!(nearest!(st, "raspberry pi under a desk"), Some(control));
    assert_eq!(nearest!(st, "moved to a cloud vm"), Some(id));
}

/// Red-capability proof for oracle 1: asserts the OPPOSITE of the oracle.
/// `cargo test -p peat -- --ignored` must show this failing — if it ever
/// passes, the retraction path is broken and the oracle above has gone
/// blind. (Kept `#[ignore]`d so the suite is green by default.)
#[test]
#[ignore = "red-capability proof: must FAIL when run explicitly"]
fn retraction_oracle_is_red_capable() {
    let path = tmp();
    let mut st = open(&path);
    let id = ("s1".to_string(), OBS_SEQ_BASE);
    st.wtx(|tx| {
        tx.upsert(
            &id,
            &obs("s1", 0, 1000, "staging", "runs on the raspberry pi").1,
        );
    });
    st.wtx(|tx| {
        tx.upsert(&id, &obs("s1", 0, 2000, "staging", "moved to a cloud vm").1);
    });
    assert!(
        find_kw!(st, "raspberry").contains(&id),
        "correct behavior: this assertion is meant to fail"
    );
}

// ---------------------------------------------------------------- oracle 2

/// Deterministic synthetic ledger: interleaved sessions, tools, failures,
/// obs revisions across days. No randomness, no wall clock.
fn ledger() -> Vec<(EventId, Envelope)> {
    let mut ev = Vec::new();
    for s in 0..3u32 {
        let sid = format!("s{s}");
        for n in 0..20u32 {
            let ts = u64::from(s) * DAY_MS / 2 + u64::from(n) * 3_600_000;
            ev.push(tool(&sid, n, ts, n % 5 != 0));
        }
        for n in 0..4u32 {
            let ts = u64::from(s) * DAY_MS / 2 + u64::from(n) * 7_200_000;
            ev.push(obs(
                &sid,
                n,
                ts,
                if n % 2 == 0 { "staging" } else { "gate" },
                &format!("claim {s}-{n}"),
            ));
        }
    }
    ev
}

/// What the views must contain after folding `prefix`, computed by an
/// independent plain scan (no fold involved).
type DayPrediction = BTreeMap<u64, (i64, i64)>;
type SubjPrediction = BTreeMap<String, (String, i64, (u64, u32))>;

fn predict(prefix: &[(EventId, Envelope)]) -> (DayPrediction, SubjPrediction) {
    let mut days: BTreeMap<u64, (i64, i64)> = BTreeMap::new(); // day -> (tools, fails)
    // subject -> (text, count, (last_ms, last_seq))
    let mut subj: BTreeMap<String, (String, i64, (u64, u32))> = BTreeMap::new();
    for (id, e) in prefix {
        match &e.kind {
            Event::ToolCall { ok, .. } => {
                let d = days.entry(e.ts_ms / SLOT_MS).or_default();
                d.0 += 1;
                d.1 += i64::from(!ok);
            }
            Event::Obs { subject, text, .. } => {
                let s = subj.entry(subject.clone()).or_default();
                s.1 += 1;
                if (e.ts_ms, id.1) >= s.2 {
                    s.0 = text.clone();
                    s.2 = (e.ts_ms, id.1);
                }
            }
            _ => {}
        }
    }
    (days, subj)
}

#[test]
fn replay_prefix_matches_independent_prediction() {
    let ev = ledger();
    for cut in [0, 1, 7, 24, ev.len()] {
        let prefix = &ev[..cut];
        let mut st = open(tmp());
        st.wtx(|tx| {
            for (id, e) in prefix {
                tx.upsert(id, e);
            }
        });
        let (want_days, want_subj) = predict(prefix);

        let (got_days, got_subj) = st.rtx(|(days, _, _, (subjects, _), _, _)| {
            let d: BTreeMap<u64, (i64, i64)> = days
                .iter()
                .map(|(k, v): (u64, DayStats)| (k, (v.tools, v.fails)))
                .collect();
            let s: BTreeMap<String, (String, i64, (u64, u32))> = subjects
                .iter()
                .map(|(k, v): (String, SubjStats)| (k, (v.text, v.count, (v.last_ms, v.last_seq))))
                .collect();
            (d, s)
        });
        assert_eq!(got_days, want_days, "day digest diverged at prefix {cut}");
        assert_eq!(got_subj, want_subj, "subjects diverged at prefix {cut}");
    }
}

/// One-transaction fold equals many-transaction fold: batching must not be
/// observable (the other half of replay determinism).
#[test]
fn batching_is_unobservable() {
    let ev = ledger();

    let mut one = open(tmp());
    one.wtx(|tx| {
        for (id, e) in &ev {
            tx.upsert(id, e);
        }
    });

    let mut many = open(tmp());
    for (id, e) in &ev {
        many.wtx(|tx| {
            tx.upsert(id, e);
        });
    }

    let a = one.rtx(|(days, _, _, (subjects, _), _, _)| {
        (
            days.iter().collect::<BTreeMap<u64, DayStats>>().len(),
            subjects
                .iter()
                .map(|(k, v): (String, SubjStats)| (k, (v.text, v.count)))
                .collect::<BTreeMap<_, _>>(),
        )
    });
    let b = many.rtx(|(days, _, _, (subjects, _), _, _)| {
        (
            days.iter().collect::<BTreeMap<u64, DayStats>>().len(),
            subjects
                .iter()
                .map(|(k, v): (String, SubjStats)| (k, (v.text, v.count)))
                .collect::<BTreeMap<_, _>>(),
        )
    });
    assert_eq!(a, b);
}

/// Two obs on one subject with the SAME timestamp in one transaction:
/// the winner must be the higher seq, not whichever drained last.
#[test]
fn equal_timestamp_obs_resolve_by_seq() {
    for _ in 0..8 {
        // repeated runs guard against hash-order flakiness going unseen
        let mut st = open(tmp());
        st.wtx(|tx| {
            tx.upsert(
                &("s1".to_string(), OBS_SEQ_BASE),
                &obs("s1", 0, 5000, "staging", "first claim").1,
            );
            tx.upsert(
                &("s1".to_string(), OBS_SEQ_BASE + 1),
                &obs("s1", 1, 5000, "staging", "second claim").1,
            );
        });
        let text = st.rtx(|(_, _, _, (subjects, _), _, _)| {
            subjects
                .get(&"staging".to_string())
                .map(|s: SubjStats| s.text)
        });
        assert_eq!(text.as_deref(), Some("second claim"));
    }
}

/// The asof contract at the view level: folding only the events at-or-
/// before a cutoff yields the belief state OF that day — the later
/// revision does not exist there.
#[test]
fn prefix_fold_reconstructs_past_beliefs() {
    let full = open(tmp());
    let mut full = full;
    full.wtx(|tx| {
        tx.upsert(
            &("s1".to_string(), OBS_SEQ_BASE),
            &obs("s1", 0, DAY_MS, "staging", "on the raspberry pi").1,
        );
        tx.upsert(
            &("s1".to_string(), OBS_SEQ_BASE + 1),
            &obs("s1", 1, 3 * DAY_MS, "staging", "moved to a cloud vm").1,
        );
    });
    // read the ledger mirror back, cut at day 2, fold into a scratch db
    let cutoff = 2 * DAY_MS;
    let prefix: Vec<(EventId, Envelope)> = full.rtx(|(_, _, _, _, _, ledger)| {
        ledger
            .iter()
            .filter(|(_, e): &(EventId, Envelope)| e.ts_ms <= cutoff)
            .collect()
    });
    assert_eq!(prefix.len(), 1);
    let mut past = open(tmp());
    past.wtx(|tx| {
        for (id, e) in &prefix {
            tx.upsert(id, e);
        }
    });
    let (then, now) = (
        past.rtx(|(_, _, _, (subjects, _), _, _)| {
            subjects
                .get(&"staging".to_string())
                .map(|s: SubjStats| s.text)
        }),
        full.rtx(|(_, _, _, (subjects, _), _, _)| {
            subjects
                .get(&"staging".to_string())
                .map(|s: SubjStats| s.text)
        }),
    );
    assert_eq!(then.as_deref(), Some("on the raspberry pi"));
    assert_eq!(now.as_deref(), Some("moved to a cloud vm"));
}

// ------------------------------------------------------------ capture path

const FIXTURE: &str = include_str!("../tests/fixtures/transcript-nx-rs-planread.jsonl");

#[test]
fn capture_fixture_parses_and_is_idempotent() {
    let parsed = crate::transcript::parse(FIXTURE, None).expect("fixture has a session id");
    assert!(
        parsed.events.len() >= 10,
        "fixture should yield a real event stream, got {}",
        parsed.events.len()
    );
    // shape: exactly one SessionMeta, at least one ToolCall and one FinalMsg
    let count = |f: fn(&Event) -> bool| parsed.events.iter().filter(|(_, e)| f(&e.kind)).count();
    assert_eq!(count(|e| matches!(e, Event::SessionMeta { .. })), 1);
    assert!(count(|e| matches!(e, Event::ToolCall { .. })) >= 8);
    assert_eq!(count(|e| matches!(e, Event::FinalMsg { .. })), 1);
    // ids are unique (the idempotency key)
    let mut ids: Vec<&EventId> = parsed.events.iter().map(|(id, _)| id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), parsed.events.len(), "duplicate event ids");

    // capturing twice must equal capturing once
    let mut st = open(tmp());
    st.wtx(|tx| {
        for (id, e) in &parsed.events {
            tx.upsert(id, e);
        }
    });
    let once = st.rtx(|(days, _, _, _, sessions, _)| {
        (
            days.iter().collect::<BTreeMap<u64, DayStats>>().len(),
            sessions.iter().count(),
        )
    });
    st.wtx(|tx| {
        for (id, e) in &parsed.events {
            tx.upsert(id, e);
        }
    });
    let twice = st.rtx(|(days, _, _, _, sessions, _)| {
        (
            days.iter().collect::<BTreeMap<u64, DayStats>>().len(),
            sessions.iter().count(),
        )
    });
    assert_eq!(once, twice);
}

#[test]
fn unknown_lines_never_fail() {
    let weird = r#"{"type":"mode","mode":"normal","sessionId":"sX"}
not even json
{"type":"totally-new-thing","payload":{"deep":[1,2,3]}}
{"type":"user","sessionId":"sX","timestamp":"2026-08-16T20:00:00.000Z","message":{"content":"hello"}}
"#;
    let parsed = crate::transcript::parse(weird, None).unwrap();
    assert_eq!(parsed.session, "sX");
    assert!(
        parsed
            .events
            .iter()
            .any(|(_, e)| matches!(&e.kind, Event::UserMsg { text } if text == "hello"))
    );
}

#[test]
fn iso_timestamps_round_trip() {
    let ms = crate::transcript::iso_to_ms("2026-08-10T19:18:11.311Z").unwrap();
    assert_eq!(ms, 1786389491311);
    assert_eq!(crate::transcript::iso_to_ms("garbage"), None);
    // date_label is the inverse's date part
    assert_eq!(crate::transcript::date_label(ms), "2026-08-10");
}

/// Codex rollout adapter: positive-signature detection, message/tool/
/// compaction mapping, developer-role and reasoning skipped, unknown
/// formats rejected rather than guessed.
#[test]
fn codex_rollout_parses_and_unknown_rejected() {
    let rollout = r#"{"timestamp":"2026-08-16T20:00:00.000Z","type":"session_meta","payload":{"session_id":"cdx-1","cwd":"/tmp/w"}}
{"timestamp":"2026-08-16T20:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the flaky test"}]}}
{"timestamp":"2026-08-16T20:00:02.000Z","type":"response_item","payload":{"type":"reasoning","content":"hidden"}}
{"timestamp":"2026-08-16T20:00:03.000Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"injected instructions"}]}}
{"timestamp":"2026-08-16T20:00:04.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"jj describe -m \\\"fold: fix\\\"\"}"}}
{"timestamp":"2026-08-16T20:00:05.000Z","type":"compacted","payload":{"message":"summary of what the window held"}}
{"timestamp":"2026-08-16T20:00:06.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"The flaky test is fixed; root cause was an unseeded rng in the harness."}]}}
"#;
    let parsed = crate::transcript::parse(rollout, None).expect("codex rollout parses");
    assert_eq!(parsed.session, "cdx-1");
    let count = |f: fn(&Event) -> bool| parsed.events.iter().filter(|(_, e)| f(&e.kind)).count();
    assert_eq!(count(|e| matches!(e, Event::SessionMeta { .. })), 1);
    assert_eq!(
        count(|e| matches!(e, Event::UserMsg { .. })),
        1,
        "developer role must be skipped"
    );
    assert_eq!(count(|e| matches!(e, Event::ToolCall { .. })), 1);
    assert_eq!(
        count(|e| matches!(e, Event::Commit { .. })),
        0,
        "commits are not guessed from command text (0.5.0)"
    );
    assert_eq!(count(|e| matches!(e, Event::Compaction {})), 1);
    assert_eq!(count(|e| matches!(e, Event::CompactSummary { .. })), 1);
    assert_eq!(count(|e| matches!(e, Event::FinalMsg { .. })), 1);
    assert_eq!(
        count(|e| matches!(e, Event::Said { .. })),
        0,
        "sole assistant msg became FinalMsg"
    );

    // unknown format: reject, never guess
    let unknown = r#"{"kind":"mystery","data":1}
{"kind":"mystery","data":2}
"#;
    assert!(crate::transcript::parse(unknown, Some("s")).is_none());
}

/// The capture-cursor fast path must be invisible in the ledger: parsing
/// from a skip point yields exactly the suffix of a full parse — same ids,
/// same envelopes — so cursor + overlap + idempotent upserts converge to
/// the cold-capture state.
#[test]
fn parse_from_matches_full_parse_suffix() {
    let jsonl = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/transcript-nx-rs-planread.jsonl"
    ))
    .unwrap();
    let full = crate::transcript::parse(&jsonl, None).unwrap();
    for skip in [1usize, 10, 30] {
        let tail = crate::transcript::parse_from(&jsonl, None, skip).unwrap();
        assert_eq!(tail.session, full.session);
        let cut = (skip as u32) * 16;
        let expect: Vec<_> = full
            .events
            .iter()
            .filter(|((_, seq), _)| *seq >= cut && *seq < crate::event::HOOK_FINAL_SEQ)
            .map(|(id, e)| (id.clone(), serde_json::to_string(&e.kind).unwrap()))
            .collect();
        let got: Vec<_> = tail
            .events
            .iter()
            .filter(|((_, seq), _)| *seq >= cut && *seq < crate::event::HOOK_FINAL_SEQ)
            .map(|(id, e)| (id.clone(), serde_json::to_string(&e.kind).unwrap()))
            .collect();
        assert_eq!(got, expect, "skip={skip} diverged from full-parse suffix");
    }
}

// ------------------------------------------------------------- anchoring
//
// A claim carries its basis, not just its age: `Obs2` stamps the repo
// state at deposit, legacy `Obs` parses forever and renders unanchored,
// and the drift figure is a read-time join over the day table.

fn obs2(
    session: &str,
    n: u32,
    ts: u64,
    subject: &str,
    text: &str,
    basis: Option<crate::event::Basis>,
) -> (EventId, Envelope) {
    (
        (session.to_string(), OBS_SEQ_BASE + n),
        Envelope::new(
            session,
            ts,
            Event::Obs2 {
                subject: subject.to_string(),
                text: text.to_string(),
                derived_from: vec![],
                basis,
            },
        ),
    )
}

#[test]
fn anchored_and_legacy_obs_coexist() {
    let dir = tmp();
    let mut st = open(&dir);
    let basis = crate::event::Basis {
        commit: "abcdef0123456789".into(),
        dirty: true,
    };
    st.wtx(|tx| {
        for (id, e) in [
            obs("s", 0, DAY_MS, "subj", "legacy claim"),
            obs2(
                "s",
                1,
                2 * DAY_MS,
                "subj",
                "anchored claim",
                Some(basis.clone()),
            ),
        ] {
            tx.upsert(&id, &e);
        }
    });
    let (head, rows) = st.rtx(|(_, _, _, (subjects, evidence), _, _)| {
        (
            subjects.get(&"subj".to_string()) as Option<SubjStats>,
            evidence.get(&"subj".to_string()) as Vec<crate::pipeline::ObsRow>,
        )
    });
    let head = head.unwrap();
    assert_eq!(head.text, "anchored claim");
    let b = head.basis.expect("winning obs is anchored");
    assert_eq!(b.commit, "abcdef0123456789");
    assert!(b.dirty);
    assert_eq!(b.label(), "@abcdef01+");
    let legacy = rows.iter().find(|r| r.seq == OBS_SEQ_BASE).unwrap();
    assert!(legacy.basis.is_none(), "legacy Obs renders unanchored");
    let anchored = rows.iter().find(|r| r.seq == OBS_SEQ_BASE + 1).unwrap();
    assert!(anchored.basis.is_some());
}

/// A view-version mismatch must rebuild every view from the ledger mirror
/// alone and keep the previous database as a backup — the migration path
/// every existing db takes on first contact with a new view schema.
#[test]
fn view_rebuild_replays_the_ledger() {
    let dir = tmp();
    let events = [
        tool("s", 16, DAY_MS, true),
        obs("s", 0, DAY_MS, "subj", "legacy claim"),
        obs2(
            "s",
            1,
            2 * DAY_MS,
            "subj",
            "anchored claim",
            Some(crate::event::Basis {
                commit: "abcdef0123456789".into(),
                dirty: false,
            }),
        ),
    ];
    {
        let mut st = open(&dir);
        st.wtx(|tx| {
            for (id, e) in &events {
                tx.upsert(id, e);
            }
        });
        st.checkpoint();
    }
    // simulate a db written under the previous view schema
    let marker = crate::db::beside(&dir, ".view-version");
    std::fs::write(&marker, "1\n").unwrap();
    crate::db::migrate_views(&dir);
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap().trim(),
        crate::db::VIEW_VERSION.to_string()
    );
    assert!(crate::db::beside(&dir, ".old").exists(), "previous db kept");
    let st = open(&dir);
    let (head, ledger_len) = st.rtx(|(_, _, _, (subjects, _), _, ledger)| {
        (
            subjects.get(&"subj".to_string()) as Option<SubjStats>,
            ledger.iter().count(),
        )
    });
    assert_eq!(ledger_len, events.len(), "ledger mirror survives rebuild");
    let head = head.unwrap();
    assert_eq!(head.text, "anchored claim");
    assert_eq!(head.basis.unwrap().commit, "abcdef0123456789");
}

/// A database at `dir` holding `n` tool events (its ledger length tells
/// one database from another).
fn db_of(dir: &std::path::Path, n: u32) {
    let mut st = open(dir);
    st.wtx(|tx| {
        for i in 0..n {
            let (id, e) = tool("s", 16 + i, DAY_MS, true);
            tx.upsert(&id, &e);
        }
    });
}

fn ledger_len(dir: &std::path::Path) -> usize {
    open(dir).rtx(|(_, _, _, _, _, ledger)| ledger.iter().count())
}

fn marker_of(dir: &std::path::Path) -> String {
    std::fs::read_to_string(crate::db::beside(dir, ".view-version"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// A rebuild killed anywhere in its swap is finished from what is on disk,
/// never read as current before it is, and never loses the ledger. The
/// rebuild proper is told apart from the original by its ledger length
/// (3 vs 2 events); `db.swap` is written only once a rebuild is complete.
#[test]
fn a_rebuild_killed_mid_swap_is_finished_not_read_half_done() {
    use crate::db::{Views, beside, migrate_views, settle, views};
    let current = crate::db::VIEW_VERSION.to_string();
    let stage = |db_present: bool, rebuild_present: bool| {
        let dir = tmp();
        if db_present {
            db_of(&dir, 2);
        } else {
            db_of(&beside(&dir, ".old"), 2);
        }
        if rebuild_present {
            db_of(&beside(&dir, ".rebuild"), 3);
        }
        std::fs::write(beside(&dir, ".view-version"), "1\n").unwrap();
        std::fs::write(beside(&dir, ".swap"), "").unwrap();
        dir
    };
    let finished = |dir: &std::path::Path| {
        assert_eq!(marker_of(dir), current);
        assert!(!beside(dir, ".swap").exists(), "intent cleared");
        assert!(!beside(dir, ".rebuild").exists());
        assert_eq!(ledger_len(dir), 3, "the rebuild is in place");
        assert_eq!(ledger_len(&beside(dir, ".old")), 2, "original kept");
    };

    // killed before either rename: a full open finishes the swap
    let dir = stage(true, true);
    assert_eq!(views(&dir), Views::Stale, "half a swap is not current");
    migrate_views(&dir);
    finished(&dir);

    // killed between the renames (`db` absent): `settle`, which every
    // existence check runs first, finishes it
    let dir = stage(false, true);
    assert!(!dir.exists());
    settle(&dir);
    finished(&dir);

    // killed after both renames, before the marker: only the marker is
    // written — no second rebuild (one would leave db.old at 3 events)
    let dir = stage(false, true);
    std::fs::rename(beside(&dir, ".rebuild"), &dir).unwrap();
    migrate_views(&dir);
    finished(&dir);

    // killed while building (no intent): the partial rebuild is discarded
    // and the original's ledger rebuilt
    let dir = tmp();
    db_of(&dir, 2);
    db_of(&beside(&dir, ".rebuild"), 3);
    std::fs::write(beside(&dir, ".view-version"), "1\n").unwrap();
    migrate_views(&dir);
    assert_eq!(marker_of(&dir), current);
    assert_eq!(ledger_len(&dir), 2, "rebuilt from the original ledger");
    assert!(!beside(&dir, ".rebuild").exists());
}

/// The rebuild leaves only the active journal (see `db::retire_journal`).
/// A guard, not an oracle: at this size (one sealed journal, under fjall's
/// 512 MiB straggler threshold) maintenance already runs before the
/// rebuild closes, so this passes with the wait knocked out. The race the
/// wait closes needs a journal over 512 MiB — murail's ledger, where the
/// knockout leaves the 557 MB journal behind — too slow for the gate.
#[test]
fn a_rebuild_leaves_only_the_active_journal() {
    use crate::db::{beside, journal_files, migrate_views};
    let dir = tmp();
    {
        let mut st = open(&dir);
        st.wtx(|tx| {
            for i in 0..1_200u32 {
                let e = Envelope::new(
                    "s",
                    DAY_MS + u64::from(i),
                    Event::ToolCall {
                        tool: "Bash".into(),
                        detail: format!("{i}:{}", "x".repeat(64 << 10)),
                        ok: true,
                    },
                );
                tx.upsert(&("s".to_string(), 16 + i), &e);
            }
        });
    }
    std::fs::write(beside(&dir, ".view-version"), "1\n").unwrap();
    migrate_views(&dir);
    assert_eq!(marker_of(&dir), crate::db::VIEW_VERSION.to_string());
    assert_eq!(
        journal_files(&dir),
        1,
        "the rebuild's journal is still on disk"
    );
    assert_eq!(ledger_len(&dir), 1_200);
}

/// A process that passed the marker check and then waited for the store
/// while another rebuilt and swapped the views must not use the db it
/// then gets: [`crate::db::open_db`] re-checks the marker under the lock.
#[test]
fn open_db_rechecks_the_marker_once_it_holds_the_store() {
    use crate::db::{VIEW_VERSION, beside, open_db};
    let dir = tmp();
    db_of(&dir, 2);
    std::fs::write(beside(&dir, ".view-version"), format!("{VIEW_VERSION}\n")).unwrap();
    // another process holds the store; while it does, the views go stale
    // under the waiter (as a newer peat's swap moves the marker)
    let holder = open(&dir);
    let waiter = {
        let dir = dir.clone();
        std::thread::spawn(move || drop(open_db(dir)))
    };
    std::thread::sleep(std::time::Duration::from_millis(600));
    std::fs::write(beside(&dir, ".view-version"), "1\n").unwrap();
    drop(holder);
    waiter.join().unwrap();
    assert_eq!(marker_of(&dir), VIEW_VERSION.to_string());
    assert!(
        beside(&dir, ".old").exists(),
        "the waiter used views it never checked"
    );
}

/// A marker newer than this binary is `Newer`, never `Stale`: an old
/// binary must not rebuild over a new one's views.
#[test]
fn newer_views_are_not_stale() {
    use crate::db::{VIEW_VERSION, Views, beside, views};
    let dir = tmp();
    let set = |v: &str| std::fs::write(beside(&dir, ".view-version"), v).unwrap();
    assert_eq!(views(&dir), Views::Stale, "no marker: v1");
    set(&format!("{}\n", VIEW_VERSION + 1));
    assert_eq!(views(&dir), Views::Newer(VIEW_VERSION + 1));
    set(&format!("{VIEW_VERSION}\n"));
    assert_eq!(views(&dir), Views::Current);
    set(&format!("{}\n", VIEW_VERSION - 1));
    assert_eq!(views(&dir), Views::Stale);
    let _ = std::fs::remove_file(beside(&dir, ".view-version"));
}

// ---------------------------------------------------- checkpoint policy
//
// Checkpointing costs a corpus-sized flush, so a capture pays it only when
// the work justifies it. The failure this encodes: a Stop hook capturing a
// 15-line delta sat >7 minutes in fjall's `rotate_memtable_and_wait`,
// visibly freezing the session it was supposed to be invisible to.

#[test]
fn small_captures_do_not_checkpoint() {
    // the per-turn case: a handful of events, a fresh journal
    assert!(!crate::db::should_checkpoint(3, 0));
    assert!(!crate::db::should_checkpoint(12, 5 << 20));
    assert!(!crate::db::should_checkpoint(499, 47 << 20));
}

#[test]
fn bulk_or_grown_journal_checkpoints() {
    // bulk backfill: worth folding immediately
    assert!(crate::db::should_checkpoint(500, 0));
    assert!(crate::db::should_checkpoint(50_000, 0));
    // small delta, but the journal would dominate the next open
    assert!(crate::db::should_checkpoint(1, 48 << 20));
    assert!(crate::db::should_checkpoint(0, 200 << 20));
}

#[test]
fn journal_bytes_sums_only_jnl_files() {
    let dir = tmp();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("3.jnl"), vec![0u8; 2048]).unwrap();
    std::fs::write(dir.join("4.jnl"), vec![0u8; 1024]).unwrap();
    std::fs::write(dir.join("lock"), vec![0u8; 4096]).unwrap();
    std::fs::create_dir_all(dir.join("keyspaces")).unwrap();
    assert_eq!(crate::db::journal_bytes(&dir), 3072);
    // a missing directory is not an error — capture must never be fatal
    assert_eq!(crate::db::journal_bytes(&dir.join("nope")), 0);
}

// ------------------------------------------------------- vector lane (Flat)
//
// The vector lane is an exact scan over persisted rows (spec 2026-09-02).
// What that buys, asserted: identical rankings across two opens of the same
// ledger — there is no in-memory graph whose build order could differ.

#[test]
fn vector_lane_is_deterministic_across_opens() {
    let dir = tmp();
    let first = {
        let mut st = open(&dir);
        st.wtx(|tx| {
            for (n, text) in [
                "the retraction oracle must be red-capable",
                "checkpoint only when the work justifies the flush",
                "codex skips untrusted hooks silently",
                "hook commands must invoke bare peat from PATH",
                "views rebuild themselves from the ledger",
            ]
            .into_iter()
            .enumerate()
            {
                let (id, e) = obs("s", n as u32, DAY_MS * (n as u64 + 1), "subj", text);
                tx.upsert(&id, &e);
            }
        });
        st.checkpoint();
        st.rtx(|(_, _, (_, vec, _), _, _, _)| {
            vec.search(&ese::encode_single("hooks that fail silently"))
                .into_iter()
                .map(|h| h.val)
                .collect::<Vec<EventId>>()
        })
    };
    let second = {
        let st = open(&dir);
        st.rtx(|(_, _, (_, vec, _), _, _, _)| {
            vec.search(&ese::encode_single("hooks that fail silently"))
                .into_iter()
                .map(|h| h.val)
                .collect::<Vec<EventId>>()
        })
    };
    assert_eq!(first, second);
    assert_eq!(first.len(), 5);
}

#[test]
fn last_session_prefers_this_desk_on_a_shared_ledger() {
    use crate::pipeline::SessStats;
    let s = |cwd: &str, end_ms: u64, msg: &str| SessStats {
        end_ms,
        cwd: cwd.into(),
        final_msg: msg.into(),
        ..Default::default()
    };
    // newest-first, as the brief sorts them; herald-2a closed most recently
    let sess = vec![
        ("a".to_string(), s("/code/herald-1a", 40, "")),
        (
            "b".to_string(),
            s("/code/herald-2a/lib", 30, "2a's summary"),
        ),
        (
            "c".to_string(),
            s("/code/herald-1a/test", 20, "1a's summary"),
        ),
        ("d".to_string(), s("/code/herald", 10, "main's summary")),
    ];
    let pick = |desk: &str| {
        crate::brief::last_session(&sess, std::path::Path::new(desk))
            .map(|(s, here)| (s.final_msg.as_str(), here))
    };
    assert_eq!(pick("/code/herald-1a"), Some(("1a's summary", true)));
    // a sibling desk sharing a name prefix is not under this desk
    assert_eq!(pick("/code/herald"), Some(("main's summary", true)));
    // a desk with no closed session of its own falls back, flagged
    assert_eq!(pick("/code/herald-0b"), Some(("2a's summary", false)));
    assert_eq!(
        crate::brief::last_session(&[], std::path::Path::new("/x")).map(|_| ()),
        None
    );
}

/// The agent brief hands on this desk's previous closing message — never an
/// older session's in place of the current one's. After a compaction the
/// current session is the desk's newest closed session; it already holds
/// its context, so only the lookup line is printed.
#[test]
fn agent_brief_never_passes_off_an_older_session_as_the_last() {
    let path = tmp();
    let h = 3_600_000u64;
    let now = 100 * DAY_MS;
    let sess = |id: &str, end: u64, msg: &str| {
        vec![
            (
                (id.to_string(), 0),
                Envelope::new(
                    id,
                    end - h,
                    Event::SessionMeta {
                        cwd: "/desk".into(),
                        branch: None,
                        ese_version: String::new(),
                    },
                ),
            ),
            (
                (id.to_string(), crate::event::HOOK_FINAL_SEQ),
                Envelope::new(id, end, Event::FinalMsg { text: msg.into() }),
            ),
        ]
    };
    {
        let mut st = crate::db::open(path.clone(), crate::pipeline::pipeline);
        st.wtx(|tx| {
            for (id, e) in sess("old", now - 5 * h, "older work")
                .into_iter()
                .chain(sess("cur", now - 2 * h, "current work"))
            {
                tx.upsert(&id, &e);
            }
        });
    }
    let st = crate::db::open(path, crate::pipeline::pipeline);
    let desk = std::path::Path::new("/desk");
    let compacted = crate::brief::agent(&st, desk, Some("cur"), now);
    assert_eq!(
        compacted,
        format!("{}\n", crate::brief::LOOKUP_LINE),
        "{compacted}"
    );
    let fresh = crate::brief::agent(&st, desk, Some("new"), now);
    assert!(
        fresh.contains("current work") && !fresh.contains("older work"),
        "{fresh}"
    );
    let stale = crate::brief::agent(&st, desk, Some("new"), now + 4 * DAY_MS);
    assert_eq!(
        stale,
        format!("{}\n", crate::brief::LOOKUP_LINE),
        "older than three days"
    );
}

// ---------------------------------------------------------------- local days

/// Days are the reader's local days on every surface: the brief's "today",
/// its day digest, and a zoomed window all count the same events, under
/// the UTC offset passed in at the render boundary (never read from the
/// environment here). Late-evening work west of UTC lands on its local
/// day, not on the UTC day after.
#[test]
fn days_are_local_on_every_reading_surface() {
    let at = |iso: &str| crate::transcript::iso_to_ms(iso).unwrap();
    let pdt = -7 * 3_600_000i64; // UTC-07:00
    let path = tmp();
    {
        let mut st = crate::db::open(path.clone(), crate::pipeline::pipeline);
        st.wtx(|tx| {
            // local 2026-10-18 05:00, then 18:10..18:30 (UTC 10-19 01:xx)
            let (id, e) = tool("s", 0, at("2026-10-18T12:00:00.000Z"), true);
            tx.upsert(&id, &e);
            for n in 1..4u32 {
                let ts = at("2026-10-19T01:00:00.000Z") + u64::from(n) * 600_000;
                let (id, e) = tool("s", n, ts, n != 2);
                tx.upsert(&id, &e);
            }
            let (id, e) = obs("s", 0, at("2026-10-19T02:00:00.000Z"), "subj", "evening");
            tx.upsert(&id, &e);
        });
    }
    let st = crate::db::open(path, crate::pipeline::pipeline);
    let now = at("2026-10-19T04:00:00.000Z"); // local 2026-10-18 21:00
    let desk = std::path::Path::new("/desk");
    let brief = |offset: i64| {
        serde_json::to_value(crate::brief::assemble(&st, "", now, offset, 8, desk, None)).unwrap()
    };
    let day = |b: &serde_json::Value, i: usize| {
        (
            b["days"][i]["day"].as_str().unwrap().to_string(),
            b["days"][i]["tools"].as_i64().unwrap(),
        )
    };

    let local = brief(pdt);
    assert_eq!(local["today"], "2026-10-18");
    assert_eq!(local["days"].as_array().unwrap().len(), 1);
    assert_eq!(day(&local, 0), ("today".into(), 4));

    let utc = brief(0);
    assert_eq!(utc["today"], "2026-10-19");
    assert_eq!(day(&utc, 0), ("today".into(), 3));
    assert_eq!(day(&utc, 1), ("yesterday".into(), 1));

    // zoom's reads: `peat 2026-10-18` and `peat w42` under the same offset
    let (slots, obs_ts): (Vec<(u64, DayStats)>, Vec<u64>) =
        st.rtx(|(days, _, _, (_, evidence), _, _)| {
            let ts = evidence
                .get(&"subj".to_string())
                .iter()
                .map(|r: &crate::pipeline::ObsRow| r.ts_ms)
                .collect();
            (days.iter().collect(), ts)
        });
    assert_eq!(obs_ts.len(), 1);
    let now_bucket = crate::ladder::local_day(now, pdt);
    let day_rows = crate::ladder::local_days(slots.clone(), pdt);
    for window in ["2026-10-18", "w42"] {
        let (s, e, _) = crate::ladder::parse_window(window, now_bucket).unwrap();
        let none = BTreeMap::new();
        let d = crate::ladder::digest(
            &day_rows,
            &none,
            s,
            e,
            window.into(),
            String::new(),
            String::new(),
        );
        assert_eq!((d.tools, d.fails), (4, 1), "{window}");
        let (lo, hi) = crate::ladder::window_ms(s, e, pdt);
        assert!(obs_ts.iter().all(|t| (lo..hi).contains(t)), "{window}");
    }
    // the UTC reading of the same day sees only the morning call
    let (s, e, _) = crate::ladder::parse_window("2026-10-18", now_bucket).unwrap();
    let d = crate::ladder::digest(
        &crate::ladder::local_days(slots, 0),
        &BTreeMap::new(),
        s,
        e,
        String::new(),
        String::new(),
        String::new(),
    );
    assert_eq!(d.tools, 1);
}

/// Quarter-hour slots keep every real offset exact: at UTC+05:45 the slot
/// starting 18:15 UTC is the first of the next local day.
#[test]
fn quarter_hour_offsets_split_days_exactly() {
    let at = |iso: &str| crate::transcript::iso_to_ms(iso).unwrap();
    let npt = (5 * 60 + 45) * 60_000i64;
    let slot = |iso: &str| {
        (
            at(iso) / SLOT_MS,
            DayStats {
                tools: 1,
                ..Default::default()
            },
        )
    };
    let rows = crate::ladder::local_days(
        [
            slot("2026-10-18T18:14:59.999Z"),
            slot("2026-10-18T18:15:00.000Z"),
        ],
        npt,
    );
    let d18 = crate::ladder::local_day(at("2026-10-18T12:00:00.000Z"), npt);
    assert_eq!(
        rows.iter()
            .map(|(d, s)| (*d - d18, s.tools))
            .collect::<Vec<_>>(),
        vec![(0, 1), (1, 1)]
    );
}

// ------------------------------------------------- recall: the person's words

fn user(session: &str, seq: u32, ts: u64, text: &str) -> (EventId, Envelope) {
    (
        (session.to_string(), seq),
        Envelope::new(session, ts, Event::UserMsg { text: text.into() }),
    )
}

#[test]
fn only_the_persons_words_count() {
    use crate::transcript::persons_words;
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
        persons_words(
            "<command-name>/template</command-name>\n<command-args>move peat onto jj</command-args>"
        )
        .as_deref(),
        Some("move peat onto jj")
    );
    // samples from real ledgers: what the person typed survives the
    // harness around it; their own tags are theirs
    for (raw, words) in [
        (
            "\n<pasted_content id=\"a1\">\nmy notes",
            "<pasted_content id=\"a1\">\nmy notes",
        ),
        (
            "<image name=[Image #1] path=\"/s.png\"> </image> [Image #1] poor UI choices",
            "[Image #1] poor UI choices",
        ),
        (
            "<feedback>\nThe five defects\n</feedback>",
            "<feedback>\nThe five defects\n</feedback>",
        ),
        (
            "<ctx>murail</ctx> what next?",
            "<ctx>murail</ctx> what next?",
        ),
        (
            "<system-reminder>\nThe user started task_51d.\n</system-reminder>\n\nDouble check anything",
            "Double check anything",
        ),
        (
            "<pre-compact>\nchecked.md\n</pre-compact>\n\nYour original findings",
            "Your original findings",
        ),
        // another agent's reply landed while the person typed: cut from
        // the envelope on, keep what came before
        (
            "Does this con[tmux-bridge from:murail-codex pane:%8 at:0:2.2 — load the smux skill to reply] smcs.2 landed",
            "Does this con",
        ),
        (
            "Yes, and the zoom [hail kind:fyi from:murail-2a/%6 reply:%6 id:0906T065614-08e2] Both landed",
            "Yes, and the zoom",
        ),
        (
            "<command-name>/cd</command-name> <command-args>~/code/murail-1b[tmux-bridge from:murail-1a pane:%5 at:0:2.1 — load the smux skill to reply] Ruling</command-args>",
            "~/code/murail-1b",
        ),
        // a mention of an envelope is not one
        (
            "a `[hail ...]` (or `[tmux-bridge from:...]`) envelope appears",
            "a `[hail ...]` (or `[tmux-bridge from:...]`) envelope appears",
        ),
    ] {
        assert_eq!(persons_words(raw).as_deref(), Some(words), "{raw}");
    }
    for h in [
        "Base directory for this skill: /x\nno args",
        "<task-notification>x",
        "<system-reminder>x</system-reminder>",
        "<system-reminder>unclosed, so all harness",
        "<system-reminder>x</system-reminder>\n\nCodebase and user instructions are shown below.",
        "<bash-input>pwd</bash-input>",
        "<local-command-stdout>ok</local-command-stdout>",
        "<turn_aborted>\nThe user interrupted\n</turn_aborted>",
        "<codex_internal_context source=\"goal\">\n<objective>x</objective>\n</codex_internal_context>",
        "<command-name>/compact</command-name>",
        "<image name=[Image #1] path=\"/s.png\"> </image>",
        "[hail kind:fyi] x",
        "[from dev-claude reviewer] T1 VERDICT",
        "[tmux-bridge from:dev-claude pane:%7] hi",
        "[Request interrupted by user]",
        "# AGENTS.md instructions for /x",
        "   ",
    ] {
        assert_eq!(persons_words(h), None, "{h}");
    }
}

/// Harness text arrives in the user role but is not what anyone said:
/// indexing it makes recall return the harness. Only the person's words
/// are searchable; the ledger keeps the raw message.
#[test]
fn injected_text_is_not_recallable() {
    let mut st = open(tmp());
    let hail = user(
        "s",
        16,
        10,
        "[hail kind:ask from:2b] zanzibar ruling please",
    );
    let reminder = user(
        "s",
        32,
        20,
        "<system-reminder>quokka rules</system-reminder>",
    );
    let slash = user(
        "s",
        48,
        30,
        "<command-name>/fix</command-name>\n<command-args>marmalade gate</command-args>",
    );
    st.wtx(|tx| {
        for (id, e) in [&hail, &reminder, &slash] {
            tx.upsert(id, e);
        }
    });
    assert!(find_kw!(st, "zanzibar").is_empty(), "hail envelope indexed");
    assert!(find_kw!(st, "quokka").is_empty(), "system reminder indexed");
    assert_eq!(find_kw!(st, "marmalade"), vec![slash.0.clone()]);
    // the row holds the arguments, not the wrapper; the ledger, everything
    let (row, raw) = st.rtx(|(_, _, (_, _, texts), _, _, ledger)| {
        (
            texts
                .get(&slash.0)
                .map(|t: crate::pipeline::TextRow| t.text),
            ledger.get(&hail.0).map(|e: Envelope| e.kind.summary()),
        )
    });
    assert_eq!(row.as_deref(), Some("marmalade gate"));
    assert!(raw.is_some_and(|s| s.contains("zanzibar")));
}

// --------------------------------------------- recall: the calling session

/// A ledger where the calling session ("me") and two others all match.
fn calling_fixture() -> crate::db::Db {
    let path = tmp();
    {
        let mut st = crate::db::open(path.clone(), crate::pipeline::pipeline);
        st.wtx(|tx| {
            for (id, e) in [
                user("me", 16, 10, "the ferrous wombat ferrous wombat"),
                user("me", 32, 11, "ferrous wombat again"),
                user("other", 16, 20, "the ferrous wombat lives in fold"),
                user("third", 16, 30, "a ferrous wombat note"),
            ] {
                tx.upsert(&id, &e);
            }
        });
    }
    crate::db::open(path, crate::pipeline::pipeline)
}

/// The calling session's rows are earlier turns the caller may not hold
/// (a sub-agent shares its coordinator's id): ranked after every other
/// session's, flagged, never dropped — and never at another hit's cost.
#[test]
fn recall_ranks_the_calling_session_last() {
    use crate::cmd::recall::search;
    let st = calling_fixture();
    let all = crate::cli::Filter::default();
    let order = |limit: usize, calling: Option<&str>| {
        let (hits, _) = search(&st, "ferrous wombat", limit, &all, calling, 100);
        hits.into_iter()
            .map(|h| (h.session, h.calling_session))
            .collect::<Vec<_>>()
    };
    let unflagged = order(12, None);
    assert_eq!(unflagged.len(), 4);
    assert!(unflagged.iter().all(|(_, c)| !c));
    let demoted = order(12, Some("me"));
    assert_eq!(demoted.len(), 4, "demotion dropped a hit");
    let tail: Vec<_> = demoted[2..].to_vec();
    assert_eq!(tail, [("me".into(), true), ("me".into(), true)]);
    assert!(demoted[..2].iter().all(|(s, c)| s != "me" && !c));
    // a limit the other sessions can fill is filled by them
    let two = order(2, Some("me"));
    assert!(two.iter().all(|(s, _)| s != "me"), "{two:?}");
}

/// When the calling session fills the first keyword page, the fetch
/// widens: another session's hit is found, not crowded out. Its row is a
/// long one (keyword lane only, too long to embed), so only the keyword
/// fetch can reach it.
#[test]
fn recall_widens_past_the_calling_session() {
    use crate::cmd::recall::search;
    let path = tmp();
    {
        let mut st = crate::db::open(path.clone(), crate::pipeline::pipeline);
        let long = format!(
            "ferrous wombat {}",
            "filler words about nothing ".repeat(20)
        );
        assert!(long.len() > crate::pipeline::EMBED_USER_MAX);
        st.wtx(|tx| {
            for n in 0..4u32 {
                let (id, e) = user("me", 16 * (n + 1), 10, "ferrous wombat");
                tx.upsert(&id, &e);
            }
            let (id, e) = user("other", 16, 20, &long);
            tx.upsert(&id, &e);
        });
    }
    let st = crate::db::open(path, crate::pipeline::pipeline);
    let (hits, _) = search(
        &st,
        "ferrous wombat",
        1,
        &crate::cli::Filter::default(),
        Some("me"),
        100,
    );
    let got: Vec<_> = hits.iter().map(|h| h.session.as_str()).collect();
    assert_eq!(got, ["other"]);
}

/// The brief's `relevant` section follows the same rule.
#[test]
fn brief_ranks_the_calling_session_last() {
    let st = calling_fixture();
    let rel = |calling: Option<&str>| {
        let b = serde_json::to_value(crate::brief::assemble(
            &st,
            "ferrous wombat",
            100,
            0,
            8,
            std::path::Path::new("/desk"),
            calling.map(String::from),
        ))
        .unwrap();
        b["relevant"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["calling_session"].as_bool().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(rel(None), [false; 4]);
    assert_eq!(rel(Some("me")), [false, false, true, true]);
}

#[test]
fn calling_session_trusts_only_exact_env_evidence() {
    let env = |pairs: &[(&str, &str)]| {
        crate::db::calling_session(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())))
    };
    assert_eq!(env(&[]), None);
    assert_eq!(
        env(&[("CLAUDE_CODE_SESSION_ID", "c1")]).as_deref(),
        Some("c1")
    );
    assert_eq!(env(&[("CLAUDE_CODE_SESSION_ID", " ")]), None);
    // a codex started from a Claude pane inherits Claude's id: codex wins
    assert_eq!(
        env(&[("CLAUDE_CODE_SESSION_ID", "c1"), ("CODEX_THREAD_ID", "x1")]).as_deref(),
        Some("x1")
    );
    // a variable Codex sets per process means codex: the inherited Claude
    // id is never evidence there
    for codex in [
        "CODEX_SANDBOX",
        "CODEX_TOOL_CALL_ID",
        "CODEX_SANDBOX_NETWORK_DISABLED",
    ] {
        assert_eq!(
            env(&[("CLAUDE_CODE_SESSION_ID", "c1"), (codex, "1")]),
            None,
            "{codex}"
        );
    }
    // a person's own exported CODEX_HOME does not make a Claude session codex
    assert_eq!(
        env(&[("CLAUDE_CODE_SESSION_ID", "c1"), ("CODEX_HOME", "/h")]).as_deref(),
        Some("c1")
    );
}

/// Commits are joined to a session by its parsed `Session:` trailer in a
/// real jj repo — never by a body line that merely mentions the id.
#[test]
fn session_commits_read_the_trailer_not_the_text() {
    if std::process::Command::new("jj")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("jj absent: skipped");
        return;
    }
    let repo = tmp();
    std::fs::create_dir_all(&repo).unwrap();
    // jj reports resolved paths, as a session's recorded cwd is (getcwd);
    // the temp dir is a symlink on macOS
    let repo = repo.canonicalize().unwrap();
    let jj = |args: &[&str]| {
        let out = std::process::Command::new("jj")
            .current_dir(&repo)
            .env("JJ_CONFIG", "/dev/null")
            .env("JJ_USER", "t")
            .env("JJ_EMAIL", "t@t")
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    jj(&["git", "init"]);
    let id = "e4acd12f-b1bd-4524-b96f-c21d5522a932";
    for msg in [
        format!("full\n\nSession: {id}"),
        "prefix\n\nSession: e4acd12f-b1bd".to_string(),
        format!("mention\n\nthe body names Session: {id} in passing\n\nAgent: codex"),
        format!("longer\n\nSession: {id}-extra"),
        "other\n\nSession: 01a153a5-a16f".to_string(),
    ] {
        jj(&["new", "-m", &msg]);
    }
    let (found, cut) = crate::vcs::session_commits(&repo, id).unwrap();
    let mut subjects: Vec<String> = found
        .iter()
        .map(|c| c.description.lines().next().unwrap().to_string())
        .collect();
    subjects.sort();
    assert_eq!(subjects, ["full", "prefix"]);
    assert!(!cut);

    // a retired desk of this repo: its directory is gone, jj still knows it
    let desk = repo.with_extension("ws2");
    jj(&["workspace", "add", desk.to_str().unwrap()]);
    std::fs::remove_dir_all(&desk).unwrap();
    let inside = desk.join("src");
    assert_eq!(
        crate::cmd::show::session_repo(inside.to_str().unwrap(), &repo),
        Some(repo.clone())
    );
    // gone, and never one of this repo's workspaces: no repo to ask
    let elsewhere = repo.with_extension("other").join("src");
    assert_eq!(
        crate::cmd::show::session_repo(elsewhere.to_str().unwrap(), &repo),
        None
    );
}

/// A session whose desk is gone reads its commits from this repo only when
/// that desk was one of this repo's workspaces; otherwise none, not zero.
#[test]
fn a_gone_desk_reads_commits_only_from_its_own_repo() {
    use crate::cmd::show::session_repo;
    let here = std::env::temp_dir();
    assert_eq!(
        session_repo(here.to_str().unwrap(), std::path::Path::new("/nowhere")),
        Some(here.clone())
    );
    // a path that never existed, against a directory jj knows nothing of
    assert_eq!(session_repo("/gone/desk-1a", &here), None);
}

// ------------------------------------------------------- the session digest

fn sess_events() -> Vec<(EventId, Envelope)> {
    let e = |seq: u32, ts: u64, kind: Event| (("s".to_string(), seq), Envelope::new("s", ts, kind));
    let call = |seq: u32, ts: u64, tool: &str, ok: bool| {
        e(
            seq,
            ts,
            Event::ToolCall {
                tool: tool.into(),
                detail: format!("{tool}-{seq}"),
                ok,
            },
        )
    };
    let file = |seq: u32, ts: u64, p: &str| e(seq, ts, Event::FileTouch { path: p.into() });
    vec![
        e(
            0,
            5,
            Event::SessionMeta {
                cwd: "/desk".into(),
                branch: None,
                ese_version: String::new(),
            },
        ),
        // harness text first: it is not the intent
        user("s", 16, 10, "<system-reminder>be nice</system-reminder>"),
        user("s", 17, 20, "build the session digest"),
        user("s", 64, 90, "and then land it"),
        call(32, 30, "Bash", true),
        call(33, 31, "Bash", false),
        call(34, 32, "Read", true),
        call(48, 40, "exec_command", false),
        call(49, 41, "Bash", false),
        call(50, 42, "Bash", false),
        file(47, 30, "/desk/src/a.rs"),
        file(63, 40, "/desk/src/a.rs"),
        file(79, 50, "/desk/src/b.rs"),
    ]
}

fn sess_row(st: &KeyedStream<EventId, Envelope, crate::pipeline::Pipeline>) -> serde_json::Value {
    st.rtx(|(_, _, _, _, sessions, _)| {
        serde_json::to_value(sessions.get(&"s".to_string()) as Option<crate::pipeline::SessStats>)
            .unwrap()
    })
}

/// The digest is read off mechanical events, and folds to the same row
/// whatever the order or batching of its inputs.
#[test]
fn session_digest_is_mechanical_and_order_independent() {
    let evs = sess_events();
    let mut one = open(tmp());
    one.wtx(|tx| {
        for (id, e) in &evs {
            tx.upsert(id, e);
        }
    });
    let mut each = open(tmp());
    for (id, e) in evs.iter().rev() {
        each.wtx(|tx| tx.upsert(id, e));
    }
    let row = sess_row(&one);
    assert_eq!(row, sess_row(&each), "fold order leaked into the digest");
    assert_eq!(row["intent"], "build the session digest");
    assert_eq!(row["intent_at"], serde_json::json!([20, 17]));
    assert_eq!(
        (&row["tools"], &row["commands"], &row["fails"]),
        (&6.into(), &5.into(), &4.into())
    );
    let fails: Vec<&str> = row["first_fails"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f[2].as_str().unwrap())
        .collect();
    assert_eq!(fails, ["Bash-33", "exec_command-48", "Bash-49"]);
    assert_eq!(
        row["files"],
        serde_json::json!({"/desk/src/a.rs": 2, "/desk/src/b.rs": 1})
    );

    // a re-capture that learns a call failed revises it in place: counted
    // once, listed once
    let mut st = open(tmp());
    let ok = tool("s", 7, 1, true);
    st.wtx(|tx| tx.upsert(&ok.0, &ok.1));
    let failed = tool("s", 7, 1, false);
    st.wtx(|tx| tx.upsert(&failed.0, &failed.1));
    st.wtx(|tx| tx.upsert(&failed.0, &failed.1));
    let row = sess_row(&st);
    assert_eq!((&row["tools"], &row["fails"]), (&1.into(), &1.into()));
    assert_eq!(row["first_fails"].as_array().unwrap().len(), 1);
    // a failed call revised with a new detail replaces its listing
    let retried = (
        failed.0.clone(),
        Envelope::new(
            "s",
            1,
            Event::ToolCall {
                tool: "Bash".into(),
                detail: "retried".into(),
                ok: false,
            },
        ),
    );
    st.wtx(|tx| tx.upsert(&retried.0, &retried.1));
    let row = sess_row(&st);
    assert_eq!(row["fails"], 1);
    assert_eq!(row["first_fails"], serde_json::json!([[1, 7, "retried"]]));
    // a touch revised to another path leaves no zero-count ghost
    let touch = |p: &str| {
        (
            ("s".to_string(), 15),
            Envelope::new("s", 2, Event::FileTouch { path: p.into() }),
        )
    };
    let (id, e) = touch("/desk/a.rs");
    st.wtx(|tx| tx.upsert(&id, &e));
    let (id, e) = touch("/desk/b.rs");
    st.wtx(|tx| tx.upsert(&id, &e));
    assert_eq!(sess_row(&st)["files"], serde_json::json!({"/desk/b.rs": 1}));
}
