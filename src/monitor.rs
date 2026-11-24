use std::collections::{HashMap, VecDeque};
#[cfg(not(target_os = "macos"))]
use std::collections::HashSet;
#[cfg(not(target_os = "macos"))]
use std::ffi::CString;
#[cfg(not(target_os = "macos"))]
use std::fs;
use std::time::Instant;

#[cfg(target_os = "macos")]
#[path = "monitor_macos.rs"]
mod macos;
#[cfg(target_os = "macos")]
use macos::{read_disks, read_loadavg, read_memory, read_uptime};

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

#[derive(Debug, Clone)]
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
    pub ram_history: Vec<f32>,
    pub load_avg: (f32, f32, f32),
    pub uptime_secs: u64,
    pub memory: MemoryInfo,
    pub disks: Vec<DiskInfo>,
    pub processes: Vec<ProcessInfo>,
    pub total_processes: usize,
    pub running_processes: usize,
}

#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone, Copy, Default)]
struct CpuTicks {
    total: u64,
    idle: u64,
}

pub struct SystemMonitor {
    #[cfg(not(target_os = "macos"))]
    prev_total_cpu: Option<CpuTicks>,
    #[cfg(not(target_os = "macos"))]
    prev_core_cpu: Vec<CpuTicks>,
    prev_proc_ticks: HashMap<i32, u64>,
    prev_sample_time: Option<Instant>,
    cpu_history: VecDeque<f32>,
    ram_history: VecDeque<f32>,
    #[cfg(not(target_os = "macos"))]
    page_size: u64,
    /// Total CPU % computed by the previous `read_processes` (macOS has no cheap system-wide counter).
    #[cfg(target_os = "macos")]
    last_cpu_total: f32,
    clk_tck: f64,
    num_cpus: usize,
}

impl SystemMonitor {
    pub fn new() -> Self {
        #[cfg(not(target_os = "macos"))]
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(100) as f64;
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        Self {
            #[cfg(not(target_os = "macos"))]
            prev_total_cpu: None,
            #[cfg(not(target_os = "macos"))]
            prev_core_cpu: Vec::new(),
            prev_proc_ticks: HashMap::new(),
            prev_sample_time: None,
            cpu_history: VecDeque::with_capacity(48),
            ram_history: VecDeque::with_capacity(48),
            #[cfg(not(target_os = "macos"))]
            page_size,
            #[cfg(target_os = "macos")]
            last_cpu_total: 0.0,
            clk_tck,
            num_cpus,
        }
    }

    pub fn sample(&mut self, sort: ProcessSort) -> SystemSnapshot {
        let now = Instant::now();
        let dt_secs = self
            .prev_sample_time
            .map(|t| now.duration_since(t).as_secs_f64().max(0.05))
            .unwrap_or(1.0);
        self.prev_sample_time = Some(now);

        let (cpu_total_pct, cpu_cores_pct) = self.read_cpu();
        let memory = read_memory();
        let disks = read_disks();
        let load_avg = read_loadavg();
        let uptime_secs = read_uptime();

        if self.cpu_history.len() >= 48 {
            self.cpu_history.pop_front();
        }
        self.cpu_history.push_back(cpu_total_pct);

        if self.ram_history.len() >= 48 {
            self.ram_history.pop_front();
        }
        self.ram_history.push_back(memory.used_pct);

        let (mut processes, total_processes, running_processes) =
            self.read_processes(dt_secs, memory.total_bytes);

        sort_processes(&mut processes, sort);

        SystemSnapshot {
            cpu_total_pct,
            cpu_cores_pct,
            cpu_history: self.cpu_history.iter().copied().collect(),
            ram_history: self.ram_history.iter().copied().collect(),
            load_avg,
            uptime_secs,
            memory,
            disks,
            processes,
            total_processes,
            running_processes,
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl SystemMonitor {
    fn read_cpu(&mut self) -> (f32, Vec<f32>) {
        let Ok(content) = fs::read_to_string("/proc/stat") else {
            return (0.0, Vec::new());
        };

        let mut total_pct = 0.0f32;
        let mut new_cores = Vec::new();
        let mut cores_pct = Vec::new();

        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("cpu ") {
                let cur = parse_cpu_ticks(rest);
                if let Some(prev) = self.prev_total_cpu {
                    total_pct = calc_cpu_pct(prev, cur);
                }
                self.prev_total_cpu = Some(cur);
            } else if line.starts_with("cpu") {
                let mut parts = line.split_whitespace();
                let label = parts.next().unwrap_or("");
                if label.len() > 3 && label[3..].chars().all(|c| c.is_ascii_digit()) {
                    let rest = &line[label.len()..];
                    let cur = parse_cpu_ticks(rest);
                    let idx = new_cores.len();
                    let pct = self
                        .prev_core_cpu
                        .get(idx)
                        .map(|&prev| calc_cpu_pct(prev, cur))
                        .unwrap_or(0.0);
                    cores_pct.push(pct);
                    new_cores.push(cur);
                }
            } else {
                break;
            }
        }

        self.prev_core_cpu = new_cores;
        (total_pct, cores_pct)
    }

    fn read_processes(&mut self, dt_secs: f64, total_mem_bytes: u64) -> (Vec<ProcessInfo>, usize, usize) {
        let mut procs = Vec::new();
        let mut next_ticks = HashMap::new();
        let mut total_count = 0usize;
        let mut running_count = 0usize;

        let Ok(entries) = fs::read_dir("/proc") else {
            return (procs, 0, 0);
        };

        let total_mem_f = total_mem_bytes.max(1) as f64;
        let max_cpu = (self.num_cpus as f32) * 100.0;

        for entry in entries.flatten() {
            let fname = entry.file_name();
            let Some(pid_str) = fname.to_str() else {
                continue;
            };
            let Ok(pid) = pid_str.parse::<i32>() else {
                continue;
            };

            let stat_path = entry.path().join("stat");
            let Ok(stat_text) = fs::read_to_string(&stat_path) else {
                continue;
            };

            // Format: pid (comm with possible spaces/parens) state ppid ... utime(14) stime(15) ... num_threads(20) ... rss(24)
            let Some(open_p) = stat_text.find('(') else {
                continue;
            };
            let Some(close_p) = stat_text.rfind(')') else {
                continue;
            };
            if close_p <= open_p || close_p + 2 >= stat_text.len() {
                continue;
            }

            let name = stat_text[open_p + 1..close_p].to_string();
            let fields: Vec<&str> = stat_text[close_p + 2..].split_whitespace().collect();
            if fields.len() < 22 {
                continue;
            }

            let state = fields[0].chars().next().unwrap_or('?');
            let utime = fields[11].parse::<u64>().unwrap_or(0);
            let stime = fields[12].parse::<u64>().unwrap_or(0);
            let threads = fields[17].parse::<u32>().unwrap_or(1);
            let rss_pages = fields[21].parse::<i64>().unwrap_or(0).max(0) as u64;

            let proc_ticks = utime.saturating_add(stime);
            next_ticks.insert(pid, proc_ticks);

            let cpu_pct = if let Some(&prev) = self.prev_proc_ticks.get(&pid) {
                let delta = proc_ticks.saturating_sub(prev) as f64;
                let pct = ((delta / self.clk_tck) / dt_secs * 100.0) as f32;
                pct.clamp(0.0, max_cpu)
            } else {
                0.0
            };

            let mem_bytes = rss_pages.saturating_mul(self.page_size);
            let mem_pct = ((mem_bytes as f64 / total_mem_f) * 100.0) as f32;

            total_count += 1;
            if state == 'R' {
                running_count += 1;
            }

            procs.push(ProcessInfo {
                pid,
                name,
                state,
                cpu_pct,
                mem_bytes,
                mem_pct,
                threads,
            });
        }

        self.prev_proc_ticks = next_ticks;
        (procs, total_count, running_count)
    }
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

pub fn send_process_signal(pid: i32, sig: i32) -> Result<(), String> {
    if pid <= 1 {
        return Err("Refusing to signal system PID <= 1".to_string());
    }
    let res = unsafe { libc::kill(pid, sig) };
    if res == 0 {
        Ok(())
    } else {
        Err(format!("kill({pid}) failed: {}", std::io::Error::last_os_error()))
    }
}

#[cfg(not(target_os = "macos"))]
fn parse_cpu_ticks(rest: &str) -> CpuTicks {
    let nums: Vec<u64> = rest
        .split_whitespace()
        .filter_map(|s| s.parse::<u64>().ok())
        .collect();
    let total: u64 = nums.iter().sum();
    let idle = nums.get(3).copied().unwrap_or(0) + nums.get(4).copied().unwrap_or(0);
    CpuTicks { total, idle }
}

#[cfg(not(target_os = "macos"))]
fn calc_cpu_pct(prev: CpuTicks, cur: CpuTicks) -> f32 {
    let dt = cur.total.saturating_sub(prev.total);
    if dt == 0 {
        return 0.0;
    }
    let di = cur.idle.saturating_sub(prev.idle);
    let busy = dt.saturating_sub(di);
    ((busy as f64 / dt as f64) * 100.0).clamp(0.0, 100.0) as f32
}

#[cfg(not(target_os = "macos"))]
fn read_memory() -> MemoryInfo {
    let mut mem_total = 0u64;
    let mut mem_free = 0u64;
    let mut mem_avail = 0u64;
    let mut buffers = 0u64;
    let mut cached = 0u64;
    let mut sreclaim = 0u64;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;

    if let Ok(text) = fs::read_to_string("/proc/meminfo") {
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let key = parts.next().unwrap_or("");
            let val = parts.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) * 1024;
            match key {
                "MemTotal:" => mem_total = val,
                "MemFree:" => mem_free = val,
                "MemAvailable:" => mem_avail = val,
                "Buffers:" => buffers = val,
                "Cached:" => cached = val,
                "SReclaimable:" => sreclaim = val,
                "SwapTotal:" => swap_total = val,
                "SwapFree:" => swap_free = val,
                _ => {}
            }
        }
    }

    let available = if mem_avail > 0 { mem_avail } else { mem_free };
    let used = mem_total.saturating_sub(available);
    let cached_total = buffers.saturating_add(cached).saturating_add(sreclaim);
    let swap_used = swap_total.saturating_sub(swap_free);
    let used_pct = if mem_total > 0 {
        ((used as f64 / mem_total as f64) * 100.0).clamp(0.0, 100.0) as f32
    } else {
        0.0
    };

    MemoryInfo {
        total_bytes: mem_total,
        used_bytes: used,
        free_bytes: mem_free,
        available_bytes: available,
        cached_bytes: cached_total,
        swap_total_bytes: swap_total,
        swap_used_bytes: swap_used,
        used_pct,
    }
}

#[cfg(not(target_os = "macos"))]
fn read_disks() -> Vec<DiskInfo> {
    let mut disks = Vec::new();
    let mut seen_devices = HashSet::new();

    let Ok(mounts) = fs::read_to_string("/proc/mounts") else {
        return disks;
    };

    let skip_fs = [
        "proc",
        "sysfs",
        "devtmpfs",
        "devpts",
        "tmpfs",
        "cgroup",
        "cgroup2",
        "pstore",
        "securityfs",
        "debugfs",
        "tracefs",
        "configfs",
        "fusectl",
        "mqueue",
        "hugetlbfs",
        "autofs",
        "squashfs",
        "overlay",
        "nsfs",
        "ramfs",
        "rpc_pipefs",
        "efivarfs",
        "bpf",
    ];

    for line in mounts.lines() {
        let mut parts = line.split_whitespace();
        let Some(dev) = parts.next() else {
            continue;
        };
        let Some(mount_raw) = parts.next() else {
            continue;
        };
        let Some(fs_type) = parts.next() else {
            continue;
        };

        if skip_fs.contains(&fs_type) || !dev.starts_with('/') {
            continue;
        }
        let mount = mount_raw.replace("\\040", " ");
        if mount.starts_with("/snap/") || mount.starts_with("/var/lib/docker") {
            continue;
        }
        if !seen_devices.insert(dev.to_string()) {
            continue;
        }

        let Ok(c_mount) = CString::new(mount.as_str()) else {
            continue;
        };
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c_mount.as_ptr(), &mut stat) } != 0 {
            continue;
        }

        let frsize = stat.f_frsize as u64;
        let total = (stat.f_blocks as u64).saturating_mul(frsize);
        if total == 0 {
            continue;
        }
        let free = (stat.f_bavail as u64).saturating_mul(frsize);
        let used = total.saturating_sub((stat.f_bfree as u64).saturating_mul(frsize));
        let used_pct = ((used as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as f32;

        disks.push(DiskInfo {
            mount,
            fs_type: fs_type.to_string(),
            total_bytes: total,
            used_bytes: used,
            free_bytes: free,
            used_pct,
        });
    }

    // Put "/" first, then "/home", then others sorted by total size desc
    disks.sort_by(|a, b| {
        let rank = |m: &str| match m {
            "/" => 0,
            "/home" => 1,
            _ => 2,
        };
        rank(&a.mount)
            .cmp(&rank(&b.mount))
            .then_with(|| b.total_bytes.cmp(&a.total_bytes))
    });
    disks
}

#[cfg(not(target_os = "macos"))]
fn read_loadavg() -> (f32, f32, f32) {
    if let Ok(text) = fs::read_to_string("/proc/loadavg") {
        let mut parts = text.split_whitespace();
        let a = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let b = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let c = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        return (a, b, c);
    }
    (0.0, 0.0, 0.0)
}

#[cfg(not(target_os = "macos"))]
fn read_uptime() -> u64 {
    if let Ok(text) = fs::read_to_string("/proc/uptime") {
        if let Some(first) = text.split_whitespace().next() {
            if let Ok(secs) = first.parse::<f64>() {
                return secs as u64;
            }
        }
    }
    0
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
