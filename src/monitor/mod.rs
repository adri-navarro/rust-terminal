//! Bounded context: System and process monitor.

pub mod domain;
pub mod infra;
pub mod ports;
pub mod service;

pub use domain::{
    DiskInfo, MemoryInfo, MonitorError, ProcessInfo, ProcessSort, SystemSnapshot,
    filter_process_indices, format_bytes, format_uptime, sort_processes,
};
pub use infra::{PosixSignaler, send_process_signal};
pub use ports::{ProcessSignaler, RawSystemSample, SystemProbe};
pub use service::SystemMonitor;
