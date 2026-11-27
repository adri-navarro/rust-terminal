pub mod commands;
pub mod config;
pub mod docker;
pub mod history;
pub mod monitor;
pub mod shell;
pub mod telemetry;
pub mod terminal;
pub mod ui;
pub mod usage;

pub use shell::completer;

#[cfg(test)]
mod tests;
