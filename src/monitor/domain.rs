//! Pure domain model for the system & process monitor.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MonitorError {
    #[error("Refusing to signal system PID <= 1 (pid={0})")]
    ProtectedPid(i32),
    #[error("kill({pid}) failed: {reason}")]
    SignalFailed { pid: i32, reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSort {
    Cpu,
    Memory,
    Pid,
    Name,
}

impl ProcessSort {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "MEM",
            Self::Pid => "PID",
            Self::Name => "NAME",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub available_bytes: u64,
    pub cached_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub used_pct: f32,
}

#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub mount: String,
    pub fs_type: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub used_pct: f32,
}

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: i32,
    pub name: String,
    pub state: char,
    pub cpu_pct: f32,
    pub mem_bytes: u64,
    pub mem_pct: f32,
    pub threads: u32,
}

#[derive(Debug, Clone)]
pub struct SystemSnapshot {
    pub cpu_total_pct: f32,
    pub cpu_cores_pct: Vec<f32>,
    pub cpu_history: Vec<f32>,
    pub load_avg: (f32, f32, f32),
    pub uptime_secs: u64,
    pub memory: MemoryInfo,
    pub disks: Vec<DiskInfo>,
    pub processes: Vec<ProcessInfo>,
    pub total_processes: usize,
    pub running_processes: usize,
}

/// Indices into `procs` (in their current order) matching the filter query:
/// case-insensitive name substring, or a digit-only query contained in the pid.
pub fn filter_process_indices(procs: &[ProcessInfo], query: &str) -> Vec<usize> {
    let q = query.trim();
    if q.is_empty() {
        return (0..procs.len()).collect();
    }
    let q_lower = q.to_lowercase();
    let digits = q.chars().all(|c| c.is_ascii_digit());
    procs
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            p.name.to_lowercase().contains(&q_lower) || (digits && p.pid.to_string().contains(q))
        })
        .map(|(i, _)| i)
        .collect()
}

pub fn sort_processes(procs: &mut [ProcessInfo], sort: ProcessSort) {
    match sort {
        ProcessSort::Cpu => procs.sort_by(|a, b| {
            b.cpu_pct
                .partial_cmp(&a.cpu_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.mem_bytes.cmp(&a.mem_bytes))
                .then_with(|| a.pid.cmp(&b.pid))
        }),
        ProcessSort::Memory => procs.sort_by(|a, b| {
            b.mem_bytes
                .cmp(&a.mem_bytes)
                .then_with(|| {
                    b.cpu_pct
                        .partial_cmp(&a.cpu_pct)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.pid.cmp(&b.pid))
        }),
        ProcessSort::Pid => procs.sort_by_key(|p| p.pid),
        ProcessSort::Name => procs.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
                .then_with(|| a.pid.cmp(&b.pid))
        }),
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;
    let b = bytes as f64;
    if b >= TIB {
        format!("{:.2} TiB", b / TIB)
    } else if b >= GIB {
        format!("{:.1} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.0} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}
