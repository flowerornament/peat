//! One module per verb. Each takes the open ledger ([`crate::db::Db`]);
//! `main` parses, opens, and dispatches.

pub mod asof;
pub mod capture;
pub mod events;
pub mod obs;
pub mod recall;
pub mod refs;
pub mod show;
pub mod zoom;
