//! Hexagonal ports for OS system metrics probing and process signaling.

use super::domain::{DiskInfo, MemoryInfo, MonitorError, ProcessInfo};

pub struct RawSystemSample {
    pub cpu_total_pct: f32,
    pub cpu_cores_pct: Vec<f32>,
    pub load_avg: (f32, f32, f32),
    pub uptime_secs: u64,
    pub memory: MemoryInfo,
    pub disks: Vec<DiskInfo>,
    pub processes: Vec<ProcessInfo>,
    pub total_processes: usize,
    pub running_processes: usize,
}

pub trait SystemProbe: Send {
    fn sample_raw(&mut self, dt_secs: f64) -> RawSystemSample;
}

pub trait ProcessSignaler: Send + Sync {
    fn signal(&self, pid: i32, sig: i32) -> Result<(), MonitorError>;
}
