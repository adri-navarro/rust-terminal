//! macOS backend for the system monitor. There is no /proc, so this shells out to
//! ps / vm_stat / sysctl / df and uses libc::getloadavg. Every failure degrades to
//! zeros or empty lists.

use super::super::domain::{DiskInfo, MemoryInfo, ProcessInfo};
use super::super::ports::{RawSystemSample, SystemProbe};
use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct MacosProbe {
    prev_proc_ticks: HashMap<i32, u64>,
    last_cpu_total: f32,
    clk_tck: f64,
    num_cpus: usize,
}

impl MacosProbe {
    pub fn new() -> Self {
        let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(100) as f64;
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        Self {
            prev_proc_ticks: HashMap::new(),
            last_cpu_total: 0.0,
            clk_tck,
            num_cpus,
        }
    }

    fn read_processes(
        &mut self,
        dt_secs: f64,
        total_mem_bytes: u64,
    ) -> (Vec<ProcessInfo>, usize, usize) {
        let Some(text) = run_cmd("/bin/ps", &["-axo", "pid=,stat=,cputime=,rss=,comm="]) else {
            return (Vec::new(), 0, 0);
        };

        let mut procs = Vec::new();
        let mut next_ticks = HashMap::new();
        let mut total_count = 0usize;
        let mut running_count = 0usize;
        let mut cpu_sum = 0.0f32;

        let total_mem_f = total_mem_bytes.max(1) as f64;
        let max_cpu = (self.num_cpus as f32) * 100.0;

        for line in text.lines() {
            let Some(p) = parse_ps_line(line) else {
                continue;
            };

            next_ticks.insert(p.pid, p.cpu_centis);

            let cpu_pct = if let Some(&prev) = self.prev_proc_ticks.get(&p.pid) {
                let delta = p.cpu_centis.saturating_sub(prev) as f64;
                let pct = ((delta / self.clk_tck) / dt_secs * 100.0) as f32;
                pct.clamp(0.0, max_cpu)
            } else {
                0.0
            };
            cpu_sum += cpu_pct;

            let mem_bytes = p.rss_kib.saturating_mul(1024);
            let mem_pct = ((mem_bytes as f64 / total_mem_f) * 100.0) as f32;

            total_count += 1;
            if p.state == 'R' {
                running_count += 1;
            }

            procs.push(ProcessInfo {
                pid: p.pid,
                name: p.name,
                state: p.state,
                cpu_pct,
                mem_bytes,
                mem_pct,
                threads: 1,
            });
        }

        self.prev_proc_ticks = next_ticks;
        self.last_cpu_total = (cpu_sum / self.num_cpus.max(1) as f32).clamp(0.0, 100.0);
        (procs, total_count, running_count)
    }
}

impl SystemProbe for MacosProbe {
    fn sample_raw(&mut self, dt_secs: f64) -> RawSystemSample {
        let cpu_total_pct = self.last_cpu_total;
        let memory = read_memory();
        let disks = read_disks();
        let load_avg = read_loadavg();
        let uptime_secs = read_uptime();
        let (processes, total_processes, running_processes) =
            self.read_processes(dt_secs, memory.total_bytes);

        RawSystemSample {
            cpu_total_pct,
            cpu_cores_pct: Vec::new(),
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

fn run_cmd(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

struct PsLine {
    pid: i32,
    state: char,
    cpu_centis: u64,
    rss_kib: u64,
    name: String,
}

fn next_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    Some((&s[..end], &s[end..]))
}

fn parse_ps_line(line: &str) -> Option<PsLine> {
    let (pid, rest) = next_token(line)?;
    let (stat, rest) = next_token(rest)?;
    let (cputime, rest) = next_token(rest)?;
    let (rss, rest) = next_token(rest)?;
    let comm = rest.trim();
    if comm.is_empty() {
        return None;
    }
    let name = comm.rsplit('/').next().unwrap_or(comm);
    if name.is_empty() {
        return None;
    }
    Some(PsLine {
        pid: pid.parse().ok()?,
        state: stat.chars().next().unwrap_or('?'),
        cpu_centis: parse_cputime(cputime),
        rss_kib: rss.parse().unwrap_or(0),
        name: name.to_string(),
    })
}

fn parse_cputime(s: &str) -> u64 {
    let s = s.trim();
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().unwrap_or(0), r),
        None => (0, s),
    };
    let mut secs = 0.0f64;
    for seg in rest.split(':') {
        secs = secs * 60.0 + seg.parse::<f64>().unwrap_or(0.0);
    }
    ((days as f64 * 86_400.0 + secs).max(0.0) * 100.0).round() as u64
}

#[derive(Debug, PartialEq, Eq)]
struct VmStat {
    free_bytes: u64,
    available_bytes: u64,
    cached_bytes: u64,
}

fn parse_vm_stat(text: &str, fallback_page_size: u64) -> VmStat {
    let mut page = fallback_page_size;
    let (mut free, mut inactive, mut speculative, mut purgeable, mut file_backed) =
        (0u64, 0u64, 0u64, 0u64, 0u64);

    for line in text.lines() {
        if line.starts_with("Mach Virtual Memory Statistics") {
            if let Some(idx) = line.find("page size of ") {
                let n = line[idx + "page size of ".len()..]
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse::<u64>().ok());
                if let Some(n) = n.filter(|&n| n > 0) {
                    page = n;
                }
            }
            continue;
        }
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let val = val.trim().trim_end_matches('.').parse::<u64>().unwrap_or(0);
        match key.trim() {
            "Pages free" => free = val,
            "Pages inactive" => inactive = val,
            "Pages speculative" => speculative = val,
            "Pages purgeable" => purgeable = val,
            "File-backed pages" => file_backed = val,
            _ => {}
        }
    }

    let pages_available = free
        .saturating_add(inactive)
        .saturating_add(speculative)
        .saturating_add(purgeable);
    VmStat {
        free_bytes: free.saturating_mul(page),
        available_bytes: pages_available.saturating_mul(page),
        cached_bytes: file_backed.saturating_mul(page),
    }
}

fn parse_swapusage(text: &str) -> (u64, u64) {
    fn field(text: &str, key: &str) -> u64 {
        let needle = format!("{key} = ");
        let Some(idx) = text.find(&needle) else {
            return 0;
        };
        let Some(token) = text[idx + needle.len()..].split_whitespace().next() else {
            return 0;
        };
        let (num, mult) = match token.chars().last() {
            Some('K') => (&token[..token.len() - 1], 1024.0),
            Some('M') => (&token[..token.len() - 1], 1024.0 * 1024.0),
            Some('G') => (&token[..token.len() - 1], 1024.0 * 1024.0 * 1024.0),
            _ => (token, 1.0),
        };
        (num.parse::<f64>().unwrap_or(0.0).max(0.0) * mult) as u64
    }
    (field(text, "total"), field(text, "used"))
}

fn read_memory() -> MemoryInfo {
    let total = run_cmd("/usr/sbin/sysctl", &["-n", "hw.memsize"])
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);

    let fallback_page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
    let vm = run_cmd("/usr/bin/vm_stat", &[])
        .map(|t| parse_vm_stat(&t, fallback_page))
        .unwrap_or(VmStat {
            free_bytes: 0,
            available_bytes: 0,
            cached_bytes: 0,
        });

    let (swap_total, swap_used) = run_cmd("/usr/sbin/sysctl", &["-n", "vm.swapusage"])
        .map(|t| parse_swapusage(&t))
        .unwrap_or((0, 0));

    let used = total.saturating_sub(vm.available_bytes);
    let used_pct = if total > 0 {
        ((used as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as f32
    } else {
        0.0
    };

    MemoryInfo {
        total_bytes: total,
        used_bytes: used,
        free_bytes: vm.free_bytes,
        available_bytes: vm.available_bytes,
        cached_bytes: vm.cached_bytes,
        swap_total_bytes: swap_total,
        swap_used_bytes: swap_used.min(swap_total),
        used_pct,
    }
}

fn parse_df(text: &str) -> Vec<DiskInfo> {
    const DATA: &str = "/System/Volumes/Data";
    let mut disks = Vec::new();
    let mut seen_devices = HashSet::new();

    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 || !fields[0].starts_with("/dev/") {
            continue;
        }
        let (Ok(_blocks), Ok(used_kib), Ok(avail_kib)) = (
            fields[1].parse::<u64>(),
            fields[2].parse::<u64>(),
            fields[3].parse::<u64>(),
        ) else {
            continue;
        };
        let mount = fields[5..].join(" ");
        if mount.starts_with("/System/Volumes/") && mount != DATA {
            continue;
        }
        if mount.starts_with("/private/var/vm") {
            continue;
        }
        if !seen_devices.insert(fields[0].to_string()) {
            continue;
        }

        let used = used_kib.saturating_mul(1024);
        let free = avail_kib.saturating_mul(1024);
        let total = used.saturating_add(free);
        if total == 0 {
            continue;
        }
        let used_pct = ((used as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as f32;

        disks.push(DiskInfo {
            mount,
            fs_type: "apfs".to_string(),
            total_bytes: total,
            used_bytes: used,
            free_bytes: free,
            used_pct,
        });
    }

    if disks.iter().any(|d| d.mount == DATA) {
        disks.retain(|d| d.mount != "/");
    }

    disks.sort_by(|a, b| {
        let rank = |m: &str| match m {
            "/" | DATA => 0,
            "/home" => 1,
            _ => 2,
        };
        rank(&a.mount)
            .cmp(&rank(&b.mount))
            .then_with(|| b.total_bytes.cmp(&a.total_bytes))
    });
    disks
}

fn read_disks() -> Vec<DiskInfo> {
    run_cmd("/bin/df", &["-kP"])
        .map(|t| parse_df(&t))
        .unwrap_or_default()
}

fn read_loadavg() -> (f32, f32, f32) {
    let mut avg = [0.0f64; 3];
    let n = unsafe { libc::getloadavg(avg.as_mut_ptr(), 3) };
    if n < 3 {
        return (0.0, 0.0, 0.0);
    }
    (avg[0] as f32, avg[1] as f32, avg[2] as f32)
}

fn parse_boottime(text: &str) -> Option<u64> {
    let idx = text.find("sec = ")?;
    let digits: String = text[idx + "sec = ".len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn read_uptime() -> u64 {
    let Some(boot) = run_cmd("/usr/sbin/sysctl", &["-n", "kern.boottime"])
        .and_then(|t| parse_boottime(&t))
    else {
        return 0;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_sub(boot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cputime_formats() {
        assert_eq!(parse_cputime("0:00.00"), 0);
        assert_eq!(parse_cputime("0:01.23"), 123);
        assert_eq!(parse_cputime("12:34.56"), (12 * 60 + 34) * 100 + 56);
        assert_eq!(parse_cputime("1:02:03.50"), (3600 + 120 + 3) * 100 + 50);
        assert_eq!(parse_cputime("2-03:04:05"), (2 * 86_400 + 3 * 3600 + 4 * 60 + 5) * 100);
        assert_eq!(parse_cputime("junk"), 0);
    }

    #[test]
    fn ps_line_with_spaces_in_comm() {
        let p = parse_ps_line(
            "  412 Ss     0:01.50  20480 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        )
        .unwrap();
        assert_eq!(p.pid, 412);
        assert_eq!(p.state, 'S');
        assert_eq!(p.cpu_centis, 150);
        assert_eq!(p.rss_kib, 20480);
        assert_eq!(p.name, "Google Chrome");

        let r = parse_ps_line("    1 R+   10:00.00   1024 launchd").unwrap();
        assert_eq!(r.state, 'R');
        assert_eq!(r.name, "launchd");

        assert!(parse_ps_line("").is_none());
        assert!(parse_ps_line("abc S 0:00.00 1 foo").is_none());
        assert!(parse_ps_line("1 S 0:00.00 1").is_none());
    }

    #[test]
    fn swapusage() {
        let (t, u) = parse_swapusage(
            "total = 2048.00M  used = 1024.50M  free = 1023.50M  (encrypted)",
        );
        assert_eq!(t, 2048 * 1024 * 1024);
        assert_eq!(u, (1024.5 * 1024.0 * 1024.0) as u64);
        let (t, u) = parse_swapusage("total = 1.50G  used = 0.00M  free = 1.50G  (encrypted)");
        assert_eq!(t, (1.5 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(u, 0);
        assert_eq!(parse_swapusage("nonsense"), (0, 0));
    }

    #[test]
    fn vm_stat_text() {
        let text = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
Pages free:                               1000.\n\
Pages active:                             5000.\n\
Pages inactive:                           2000.\n\
Pages speculative:                         300.\n\
Pages wired down:                         4000.\n\
Pages purgeable:                           100.\n\
File-backed pages:                        1500.\n\
Pages occupied by compressor:              700.\n";
        let vm = parse_vm_stat(text, 4096);
        assert_eq!(vm.free_bytes, 1000 * 16384);
        assert_eq!(vm.available_bytes, (1000 + 2000 + 300 + 100) * 16384);
        assert_eq!(vm.cached_bytes, 1500 * 16384);

        let vm = parse_vm_stat("Pages free: 10.\n", 4096);
        assert_eq!(vm.free_bytes, 10 * 4096);
    }

    #[test]
    fn df_text() {
        let text = "Filesystem     1024-blocks      Used Available Capacity  Mounted on\n\
/dev/disk3s1s1   488245288  15374908 307330772     5%    /\n\
devfs                  215       215         0   100%    /dev\n\
/dev/disk3s6     488245288   6291456 307330772     2%    /System/Volumes/VM\n\
/dev/disk3s5     488245288 165000000 307330772    35%    /System/Volumes/Data\n\
/dev/disk5s1       1000000    500000    500000    50%    /Volumes/My Disk\n\
map auto_home            0         0         0   100%    /System/Volumes/Data/home\n";
        let disks = parse_df(text);
        let mounts: Vec<&str> = disks.iter().map(|d| d.mount.as_str()).collect();
        assert_eq!(mounts, vec!["/System/Volumes/Data", "/Volumes/My Disk"]);
        assert_eq!(disks[0].fs_type, "apfs");
        assert_eq!(disks[0].used_bytes, 165_000_000 * 1024);
        assert_eq!(disks[0].free_bytes, 307_330_772 * 1024);
        assert!((disks[1].used_pct - 50.0).abs() < 0.01);

        let only_root = parse_df(
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
/dev/disk1s1 1000 250 750 25% /\n",
        );
        assert_eq!(only_root.len(), 1);
        assert_eq!(only_root[0].mount, "/");
    }

    #[test]
    fn boottime() {
        assert_eq!(
            parse_boottime("{ sec = 1700000000, usec = 123456 } Tue Nov 14 22:13:20 2023"),
            Some(1_700_000_000)
        );
        assert_eq!(parse_boottime("garbage"), None);
    }
}
