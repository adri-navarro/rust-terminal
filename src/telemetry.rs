//! Structured telemetry setup (*Zero To Production* pattern).
//!
//! Logs are written to `stderr` and disabled by default so `--shell` mode and
//! normal GUI sessions produce zero noise. Set `RUST_LOG=itsadri_terminal=debug`
//! (or `info`/`trace`) to enable structured diagnostics.

use tracing_subscriber::EnvFilter;

/// Initializes the global `tracing` subscriber once per process.
pub fn init_subscriber() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("off"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(true)
        .try_init();
}
