//! Bounded context: VT100/ANSI terminal emulation and PTY sessions.

pub mod grid;
pub mod infra;
pub mod ports;

pub use infra::TerminalTab;
pub use ports::{PtySession, TerminalError};
