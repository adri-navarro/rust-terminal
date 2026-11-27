//! Linux `/proc` system probe adapter.

use super::super::domain::{DiskInfo, MemoryInfo, ProcessInfo};
use super::super::ports::{RawSystemSample, SystemProbe};
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs;

#[derive(Debug, Clone, Copy, Default)]
struct CpuTicks {
    total: u64,
    idle: u64,
}

pub struct LinuxProcProbe {
    prev_total_cpu: Option<CpuTicks>,
    prev_core_cpu: Vec<CpuTicks>,
    prev_proc_ticks: HashMap<i32, u64>,
    page_size: u64,
    clk_tck: f64,
    num_cpus: usize,
}

impl LinuxProcProbe {
    pub fn new() -> Self {
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(100) as f64;
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        Self {
            prev_total_cpu: None,
            prev_core_cpu: Vec::new(),
            prev_proc_ticks: HashMap::new(),
            page_size,
            clk_tck,
            num_cpus,
        }
    }

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

    fn read_processes(
        &mut self,
        dt_secs: f64,
        total_mem_bytes: u64,
    ) -> (Vec<ProcessInfo>, usize, usize) {
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

impl SystemProbe for LinuxProcProbe {
    fn sample_raw(&mut self, dt_secs: f64) -> RawSystemSample {
        let (cpu_total_pct, cpu_cores_pct) = self.read_cpu();
        let memory = read_memory();
        let disks = read_disks();
        let load_avg = read_loadavg();
        let uptime_secs = read_uptime();
        let (processes, total_processes, running_processes) =
            self.read_processes(dt_secs, memory.total_bytes);

        RawSystemSample {
            cpu_total_pct,
            cpu_cores_pct,
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

fn parse_cpu_ticks(rest: &str) -> CpuTicks {
    let nums: Vec<u64> = rest
        .split_whitespace()
        .filter_map(|s| s.parse::<u64>().ok())
        .collect();
    let total: u64 = nums.iter().sum();
    let idle = nums.get(3).copied().unwrap_or(0) + nums.get(4).copied().unwrap_or(0);
    CpuTicks { total, idle }
}

fn calc_cpu_pct(prev: CpuTicks, cur: CpuTicks) -> f32 {
    let dt = cur.total.saturating_sub(prev.total);
    if dt == 0 {
        return 0.0;
    }
    let di = cur.idle.saturating_sub(prev.idle);
    let busy = dt.saturating_sub(di);
    ((busy as f64 / dt as f64) * 100.0).clamp(0.0, 100.0) as f32
}

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
