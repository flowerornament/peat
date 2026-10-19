//! peat — the record of what agents did, as a fold.
//!
//! Hooks capture each session's mechanical exhaust into one append-forever
//! ledger; every readable surface is a materialized fold view over it. The
//! agent-facing surface is bare `peat` (orient) and `peat <thing>` (look
//! closer, dispatched by the argument's shape); `peat obs` records a claim
//! by hand.
//!
//! Wall-clock time is read only at the capture/render boundary (obs
//! timestamps, age labels) — never inside any fold path, so the ledger
//! stays deterministically replayable.
//!
//! Crate layout: `event` (the ledger schema — the API to our past),
//! `pipeline` (the fold graph, a named type), `transcript` (harness
//! transcripts → events, plus civil-date math), `db` (paths, opening, view
//! migration, checkpoint policy), `brief` and `ladder` (orientation),
//! `hook` (the one harness verb), `cli` (flags) and `cmd` (one module per
//! verb). This file parses and dispatches.

pub mod brief;
pub mod cli;
mod cmd;
pub mod db;
pub mod event;
mod hook;
pub mod ladder;
pub mod pipeline;
pub mod transcript;
pub mod ui;
pub mod vcs;

use clap::Parser;

use cli::{Cli, Cmd, HookCmd};
use pipeline::DAY_MS;

fn main() {
    let cli = Cli::parse();

    // hooks run under a harness deadline and mostly need no ledger at
    // all; handle them before the open, and cap the wait when they do
    let cli = match cli.cmd {
        Some(Cmd::Hook {
            cmd:
                Some(HookCmd::Install {
                    codex,
                    local,
                    check,
                    print,
                }),
            ..
        }) => {
            let fabric = match (codex, local) {
                (true, _) => hook::Fabric::Codex,
                (_, true) => hook::Fabric::ClaudeLocal,
                _ => hook::Fabric::Claude,
            };
            hook::install_cli(fabric, check, print);
            return;
        }
        Some(Cmd::Skill) => {
            print!("{}", hook::SKILL);
            return;
        }
        Some(Cmd::Hook { event, cmd: None }) => match hook::run(event) {
            hook::Action::Brief { session } => {
                // a busy ledger skips the brief silently, never the session
                let _ = db::LOCK_WAIT_SECS.set(15);
                let _ = db::LOCK_FAIL_FALLBACK.set(brief::LOOKUP_LINE.to_string());
                let st = db::open_db(db::db_path());
                print!(
                    "{}",
                    brief::agent(&st, &brief::desk(), session.as_deref(), db::now_ms())
                );
                return;
            }
            _ => return,
        },
        _ => cli,
    };

    let mut st = db::open_db(db::db_path());

    // ---- shape dispatch: bare `peat` orients; `peat <thing>` looks
    // closer, inferring window / session / subject / ref / search from the
    // argument's
    // shape. The header of every non-obvious read names its
    // interpretation, and explicit subcommands remain the unambiguous
    // spellings of the same reads.
    let cmd = match cli.cmd {
        Some(c) => c,
        None if cli.query.is_empty() => Cmd::Brief {
            task: vec![],
            json: cli.json,
            budget: cli.budget,
        },
        None => {
            let q = &cli.query;
            let now_bucket = db::now_ms() / DAY_MS;
            // a hex prefix names a session only when one has it; otherwise
            // it may be a commit, tried below as a ref
            let is_sess = |s: &str| {
                s.len() >= 6
                    && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
                    && st.rtx(|(_, _, _, _, sessions, _)| {
                        sessions
                            .iter()
                            .any(|(id, _): (String, pipeline::SessStats)| id.starts_with(s))
                    })
            };
            if q.len() == 1 && ladder::parse_window(&q[0], now_bucket).is_some() {
                Cmd::Zoom {
                    window: q[0].clone(),
                    json: cli.json,
                }
            } else if is_sess(&q[0])
                && (q.len() == 1 || (q.len() == 2 && q[1].parse::<u32>().is_ok()))
            {
                Cmd::Show {
                    session: q[0].clone(),
                    seq: q.get(1).and_then(|s| s.parse().ok()),
                }
            } else if q.len() == 1
                && st.rtx(|(_, _, _, (subjects, _), _, _)| subjects.get(&q[0]).is_some())
            {
                // an exact subject name reads its full evidence trail —
                // the expansion path every clipped belief line points at
                Cmd::Recall {
                    query: vec![],
                    limit: 12,
                    filter: Default::default(),
                    subject: Some(q[0].clone()),
                    json: cli.json,
                }
            } else if q.len() == 1
                && cmd::refs::classify(&q[0], &brief::desk()).is_some_and(|t| {
                    cmd::refs::show(&st, &t, &brief::desk(), db::now_ms(), cli.json)
                })
            {
                // a commit, change, file or token: the sessions behind it
                return;
            } else {
                let words = q.join(" ");
                if !cli.json {
                    println!("{}", ui::h1(&format!("== search: {words:?} ==")));
                }
                Cmd::Recall {
                    query: q.clone(),
                    limit: 12,
                    filter: Default::default(),
                    subject: None,
                    json: cli.json,
                }
            }
        }
    };

    match cmd {
        Cmd::Capture {
            transcript,
            session,
            final_msg,
        } => cmd::capture::capture(&mut st, transcript, session, final_msg),
        Cmd::Obs {
            subject,
            text,
            from,
            session,
            at,
        } => cmd::obs::obs(&mut st, subject, text, from, session, at),
        Cmd::Brief { task, json, budget } => {
            let b = brief::assemble(
                &st,
                &task.join(" "),
                db::now_ms(),
                brief::budget(budget),
                &brief::desk(),
            );
            brief::emit(&b, json);
        }
        Cmd::Recall {
            query,
            limit,
            filter,
            subject,
            json,
        } => cmd::recall::recall(&st, query, limit, filter, subject, json),
        Cmd::Events { filter, json } => cmd::events::events(&st, filter, json),
        Cmd::Subjects { json } => cmd::recall::subjects(&st, json),
        Cmd::Show { session, seq } => cmd::show::show(&st, session, seq),
        Cmd::Zoom { window, json } => cmd::zoom::zoom(&st, window, json),
        Cmd::Ref { target, json } => {
            let desk = brief::desk();
            let resolved = cmd::refs::classify(&target, &desk)
                .is_some_and(|t| cmd::refs::show(&st, &t, &desk, db::now_ms(), json));
            if !resolved {
                ui::error(&format!(
                    "{target:?} names no commit, change, file or mention peat knows"
                ));
                std::process::exit(1);
            }
        }
        Cmd::Compact => cmd::capture::compact(&mut st),
        Cmd::Asof { date, task, json } => cmd::asof::asof(st, date, task, json),
        Cmd::Hook { .. } | Cmd::Skill => unreachable!("handled before the ledger opens"),
    }
}

#[cfg(test)]
mod tests;
