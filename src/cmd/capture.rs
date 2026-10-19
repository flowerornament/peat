//! `peat capture` and `peat compact`: the two verbs that write mechanical
//! exhaust and fold it.

use std::path::PathBuf;

use crate::db::{self, Db};
use crate::ui;

/// Ingest a transcript (Claude Code JSONL or Codex rollout). Idempotent.
pub fn capture(
    st: &mut Db,
    transcript: PathBuf,
    session: Option<String>,
    final_msg: Option<String>,
) {
    let Ok(jsonl) = std::fs::read_to_string(&transcript) else {
        ui::error(&format!("cannot read {}", transcript.display()));
        std::process::exit(1);
    };
    // capture cursor: transcripts are append-only and seq is a pure
    // function of the raw line index, so the already-ingested prefix
    // is skipped without JSON-parsing it — hooks re-capture on every
    // stop, and a long session must not pay O(whole transcript) each
    // time. Backed off by an overlap margin (tool_use/result pairs
    // can straddle the boundary); idempotent upserts absorb overlap.
    // A missing or stale-low cursor degrades to a full parse.
    const CURSOR_MARGIN: usize = 200;
    let cursor_path = transcript.file_stem().map(|stem| {
        db::db_path()
            .parent()
            .unwrap_or(&db::db_path())
            .join("cursor")
            .join(stem)
    });
    let skip = cursor_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0)
        .saturating_sub(CURSOR_MARGIN);
    let total_lines = jsonl.lines().count();
    let Some(mut parsed) = crate::transcript::parse_from(&jsonl, session.as_deref(), skip) else {
        ui::error("no session id found; pass --session");
        std::process::exit(1);
    };
    if let Some(text) = final_msg.filter(|t| !t.trim().is_empty()) {
        crate::transcript::override_final_msg(&mut parsed, &text);
    }
    let n = parsed.events.len();
    let phase = ui::Phase::new(&format!("capturing {n} events"));
    st.wtx(|tx| {
        for (id, env) in &parsed.events {
            tx.upsert(id, env);
        }
    });
    // fsync + fold the journal into the LSM — but only when the work
    // justifies the stall it costs (see `should_checkpoint`); after a
    // bulk backfill this is what keeps later opens from replaying the
    // whole journal.
    if db::should_checkpoint(n, db::journal_bytes(&db::db_path())) {
        st.checkpoint();
    }
    if let Some(p) = &cursor_path {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, total_lines.to_string());
    }
    phase.done();
    ui::note(&format!(
        "captured {n} events from session {}",
        parsed.session
    ));
}

/// Fold the journal into the LSM now.
pub fn compact(st: &mut Db) {
    let before = db::journal_bytes(&db::db_path());
    let phase = ui::Phase::new(&format!(
        "folding {} MB of journal into the LSM",
        before / (1 << 20)
    ));
    st.checkpoint();
    phase.done();
    ui::note(&format!(
        "compacted — journal {} MB → {} MB",
        before / (1 << 20),
        db::journal_bytes(&db::db_path()) / (1 << 20)
    ));
}
