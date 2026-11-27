//! Bounded context: Command history and fuzzy search.

pub mod domain;
pub mod infra;
pub mod ports;
pub mod service;

pub use domain::{FuzzyMatch, HistoryEntry, HistoryError, format_relative_time, fuzzy_score, now_unix};
pub use infra::FileHistoryStore;
pub use ports::HistoryRepository;
pub use service::HistoryStore;
