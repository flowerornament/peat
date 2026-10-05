//! The ledger schema — peat's API to its own past.
//!
//! Every fact peat ever records is an [`Envelope`] stored under an
//! [`EventId`]. Replay-from-genesis is the recovery and time-travel story,
//! so every envelope ever written must parse forever: evolution is
//! additive-only (new variants, new optional fields), never in-place.

use serde::{Deserialize, Serialize};

/// Bumped when the schema changes shape. Written into every envelope.
pub const EVENT_VERSION: u16 = 4;

pub type SessionId = String;

/// `(session, seq)`. Seq is the transcript entry index for captured events;
/// observations use `OBS_SEQ_BASE + n` so the two ranges never collide.
/// Upserting the same id twice is a no-op by construction — re-running
/// `peat capture` on the same transcript is the crash-recovery story.
pub type EventId = (SessionId, u32);

/// High bit set: observation seqs can never collide with line-derived
/// capture seqs (`line_index` * 16 + block) no matter the transcript length.
pub const OBS_SEQ_BASE: u32 = 1 << 31;

/// Reserved seq for a `FinalMsg` delivered by the Stop hook's
/// `last_assistant_message` (authoritative over transcript tail parsing).
pub const HOOK_FINAL_SEQ: u32 = OBS_SEQ_BASE - 1;

/// Distilled events (`Event::Distill`) take seqs from here up — above
/// every observation a session could plausibly accrue, so the three
/// ranges (transcript, obs, distilled) never collide.
pub const DISTILL_SEQ_BASE: u32 = 3 << 30;

/// The synthetic session that owns window digests (day, week, month …):
/// they summarize many sessions, so no real one can hold them.
pub const DISTILL_SESSION: &str = "peat-distill";

#[derive(Clone, Serialize, Deserialize)]
pub struct Envelope {
    /// [`EVENT_VERSION`] at write time.
    pub v: u16,
    /// Milliseconds since epoch, from the transcript or the caller —
    /// never read from the wall clock inside any fold path.
    pub ts_ms: u64,
    pub session: SessionId,
    pub kind: Event,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Event {
    // ---- mechanical exhaust (cannot lie) ----
    /// Once per captured session; pins the embedding provenance.
    SessionMeta {
        cwd: String,
        branch: Option<String>,
        /// ese model + dimensions at capture time: embedding provenance,
        /// recorded so future tooling can detect a model mismatch between
        /// index-time and query-time (not yet enforced).
        ese_version: String,
    },
    /// What the user asked. Truncated to [`USER_MSG_CAP`].
    UserMsg {
        text: String,
    },
    /// One tool invocation. `detail` is the command line / file path,
    /// truncated to [`DETAIL_CAP`].
    ToolCall {
        tool: String,
        detail: String,
        ok: bool,
    },
    /// A file mutated via Edit/Write/NotebookEdit.
    FileTouch {
        path: String,
    },
    Commit {
        hash: String,
        message: String,
    },
    /// The agent's closing message — a free session summary.
    FinalMsg {
        text: String,
    },
    Compaction {},

    /// A substantive assistant message mid-session — where conclusions
    /// live ("the fix is X", "root cause was Y"). Keyword-indexed for
    /// recall; not embedded (volume). Added in v2; additive, so v1
    /// envelopes still parse.
    Said {
        text: String,
    },

    /// The compactor's own distillation of a context window it replaced —
    /// the closest thing to an observation compaction can produce, kept
    /// verbatim and recallable. Added in v2; additive.
    CompactSummary {
        text: String,
    },

    // ---- the one judgment step ----
    /// A small claim the agent chose to record. `derived_from` cites the
    /// seqs of mechanical events it rests on; empty means a bare assertion,
    /// and readers are told so.
    ///
    /// Superseded by [`Event::Obs2`] for new deposits (postcard is
    /// positional, so `Obs` could not grow a field in place); kept so
    /// every pre-v3 envelope parses forever, rendered as unanchored.
    Obs {
        subject: String,
        text: String,
        derived_from: Vec<u32>,
    },

    /// [`Event::Obs`] plus the repo state the claim was deposited against:
    /// a claim carries its basis, not just its age, so a reader can tell a
    /// durable rule from a snapshot that has since rotted. Added in v3;
    /// additive. Stamped by construction — the depositor never opts in.
    Obs2 {
        subject: String,
        text: String,
        derived_from: Vec<u32>,
        /// `None` when the deposit ran outside any git repo.
        basis: Option<Basis>,
    },

    // ---- the distiller's lane ----
    /// Something a model wrote *about* the ledger, deposited by `peat
    /// distill` rather than by the working agent: a summary of a stretch
    /// of history, a standing instruction the user gave, or an open loop.
    /// It is an assertion like any other — an event, never view state —
    /// so it is replayable, citable, and replaced only by a newer event
    /// under the same `(lane, key)`. Added in v4; additive.
    Distill {
        lane: Lane,
        /// What this is about: a segment (`<session>#<n>`) or calendar
        /// window (`2026-10-04`, `2026-w40`, `2026-10`, `2026-q4`, `2026`)
        /// for a digest; a kebab-case name for a ruling or a loop.
        key: String,
        text: String,
        /// Rulings: still in force. Loops: still open. Digests: always true.
        open: bool,
        /// The ledger events this rests on. A ruling always cites the
        /// user message it restates; a deposit that cannot is dropped.
        cites: Vec<EventId>,
        /// Fingerprint of the source text it was computed from — staleness
        /// is "the source hashes differently now", never a clock.
        src: u64,
        /// Who wrote it (the distiller command), shown as provenance.
        by: String,
    },
}

/// The three lifetimes distilled memory has. Digests fade (the ladder
/// shows them coarser with age); rulings hold until superseded; loops
/// hold until closed. Append-only: postcard encodes the variant index.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Lane {
    Digest,
    Ruling,
    Loop,
}

impl Lane {
    pub fn tag(self) -> &'static str {
        match self {
            Lane::Digest => "digest",
            Lane::Ruling => "ruling",
            Lane::Loop => "loop",
        }
    }
}

/// Repo state at deposit time, stamped mechanically into [`Event::Obs2`].
/// Frozen shape (postcard is positional): a future stamp that needs more
/// fields is a new envelope variant, not a field here.
#[derive(Clone, Serialize, Deserialize)]
pub struct Basis {
    /// Full commit hash of `HEAD` in the depositor's cwd (in a colocated
    /// jj repo this is `@-`: the last named commit, not the working copy).
    pub commit: String,
    /// Whether tracked files had uncommitted changes — the claim may rest
    /// on state no commit records.
    pub dirty: bool,
}

pub const USER_MSG_CAP: usize = 2048;
pub const FINAL_MSG_CAP: usize = 8192;
pub const DETAIL_CAP: usize = 500;
pub const SAID_CAP: usize = 1200;
/// Assistant messages shorter than this are chatter, not conclusions.
pub const SAID_MIN: usize = 80;

/// Truncate on a char boundary at `cap` bytes.
pub fn cap(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Embedding provenance stamp written into `SessionMeta`.
pub fn ese_version() -> String {
    format!("ese-static-retrieval-mrl-en-v1 dim={}", ese::DIMENSIONS)
}

impl Event {
    /// Short kind tag, used by `events --kind` and the raw ledger view.
    pub fn tag(&self) -> &'static str {
        match self {
            Event::SessionMeta { .. } => "meta",
            Event::UserMsg { .. } => "user",
            Event::ToolCall { .. } => "tool",
            Event::FileTouch { .. } => "file",
            Event::Commit { .. } => "commit",
            Event::FinalMsg { .. } => "final",
            Event::Compaction {} => "compacted",
            Event::Said { .. } => "said",
            Event::CompactSummary { .. } => "compact",
            Event::Obs { .. } | Event::Obs2 { .. } => "obs",
            Event::Distill { lane, .. } => lane.tag(),
        }
    }

    /// One display line for the raw ledger view. Adding a variant is a
    /// one-file change: this, [`Event::tag`], and the enum live together.
    pub fn summary(&self) -> String {
        let one = |s: &str| crate::ui::clip(s, 150);
        match self {
            Event::SessionMeta { cwd, branch, .. } => {
                format!("{} {}", one(cwd), branch.as_deref().unwrap_or(""))
            }
            Event::UserMsg { text }
            | Event::FinalMsg { text }
            | Event::Said { text }
            | Event::CompactSummary { text } => one(text),
            Event::ToolCall { tool, detail, ok } => {
                format!(
                    "{}{} {}",
                    tool,
                    if *ok { "" } else { " FAILED" },
                    one(detail)
                )
            }
            Event::FileTouch { path } => one(path),
            Event::Commit { hash, message } => {
                format!("{} {}", &hash[..hash.len().min(8)], one(message))
            }
            Event::Compaction {} => "— context window compacted —".into(),
            Event::Obs {
                subject,
                text,
                derived_from,
            } => format!(
                "{}: {}{}",
                subject,
                one(text),
                if derived_from.is_empty() {
                    String::new()
                } else {
                    format!(" [cites {} events]", derived_from.len())
                }
            ),
            Event::Obs2 {
                subject,
                text,
                derived_from,
                basis,
            } => format!(
                "{}: {}{}{}",
                subject,
                one(text),
                if derived_from.is_empty() {
                    String::new()
                } else {
                    format!(" [cites {} events]", derived_from.len())
                },
                basis
                    .as_ref()
                    .map(|b| format!(" {}", b.label()))
                    .unwrap_or_default()
            ),
            Event::Distill {
                key,
                text,
                open,
                cites,
                ..
            } => format!(
                "{}: {}{}{}",
                key,
                one(text),
                if *open { "" } else { " [closed]" },
                if cites.is_empty() {
                    String::new()
                } else {
                    format!(" [cites {} events]", cites.len())
                }
            ),
        }
    }
}

impl Basis {
    /// The inline anchor tag every render uses: `@abc1234`, `+` if the
    /// tree was dirty at deposit.
    pub fn label(&self) -> String {
        format!(
            "@{}{}",
            &self.commit[..self.commit.len().min(8)],
            if self.dirty { "+" } else { "" }
        )
    }

    /// The full disposition: anchor tag plus how far the repo has moved
    /// since (from [`crate::pipeline::commits_since`]).
    pub fn label_with(&self, commits_since: i64) -> String {
        if commits_since > 0 {
            format!("{} · ~{commits_since} commits since", self.label())
        } else {
            self.label()
        }
    }
}

impl Envelope {
    pub fn new(session: &str, ts_ms: u64, kind: Event) -> Self {
        Envelope {
            v: EVENT_VERSION,
            ts_ms,
            session: session.to_string(),
            kind,
        }
    }
}
