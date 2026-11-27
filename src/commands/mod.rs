//! Bounded context: Managed commands and aliases.

pub mod domain;
pub mod infra;
pub mod ports;
pub mod service;

pub use domain::{
    CommandAlias, CommandError, CommandEvent, CommandStatus, CommandText, MAX_LOG_LINES,
    ManagedCommand, SavedCommand, derive_stop_command, expand_directory_with_home, format_elapsed,
    is_detached_compose_command,
};
pub(crate) use domain::strip_ansi;
pub use infra::{OsProcessRunner, TomlCommandStore, expand_directory};
pub use ports::{CommandRepository, ProcessRunner};
pub use service::CommandManager;
