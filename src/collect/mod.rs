//! Data collectors. Each turns one Claude Code data source into [`AppEvent`]s.
//!
//! [`AppEvent`]: crate::model::AppEvent

pub mod jsonl;
pub mod procs;
pub mod push;
pub mod statscache;
