//! Application service (`ScanCache` + report orchestration) for `usage`.

use super::domain::{
    FileParse, LiveAgentQuotas, ScanCollector, TimeRange, UsageReport, build_snapshot,
};
use super::infra::{default_providers, fs_helpers::unix_now};
use super::ports::UsageProvider;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime};

struct FileCacheEntry {
    mtime: SystemTime,
    len: u64,
    parsed: FileParse,
}

/// Per-file parse results keyed by path, reused while a file's (mtime, len) is unchanged.
#[derive(Default)]
pub struct ScanCache {
    entries: HashMap<PathBuf, FileCacheEntry>,
    seen: HashSet<PathBuf>,
}

impl ScanCache {
    pub fn scan_file(
        &mut self,
        path: &Path,
        out: &mut ScanCollector,
        now: u64,
        parse: impl FnOnce() -> Option<FileParse>,
    ) {
        self.seen.insert(path.to_path_buf());
        let stamp = fs::metadata(path)
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())));
        if let (Some((mtime, len)), Some(entry)) = (stamp, self.entries.get(path)) {
            if entry.mtime == mtime && entry.len == len {
                apply_file_parse(&entry.parsed, out, now);
                return;
            }
        }
        let Some(parsed) = parse() else {
            self.entries.remove(path);
            return;
        };
        apply_file_parse(&parsed, out, now);
        match stamp {
            Some((mtime, len)) => {
                self.entries
                    .insert(path.to_path_buf(), FileCacheEntry { mtime, len, parsed });
            }
            None => {
                self.entries.remove(path);
            }
        }
    }

    #[cfg(test)]
    pub fn cached_files(&self) -> usize {
        self.entries.len()
    }

    fn finish_scan(&mut self) {
        let seen = std::mem::take(&mut self.seen);
        self.entries.retain(|path, _| seen.contains(path));
    }
}

fn apply_file_parse(parsed: &FileParse, out: &mut ScanCollector, now: u64) {
    for ev in &parsed.events {
        out.push(ev.clone());
    }
    if let Some(q) = &parsed.codex_quota {
        out.live_quotas.insert(
            "codex",
            LiveAgentQuotas {
                five_hour_pct: q.five_hour_pct,
                five_hour_reset_secs: q.five_hour_reset_at.map(|t| t.saturating_sub(now)),
                weekly_pct: q.weekly_pct,
                weekly_reset_secs: q.weekly_reset_at.map(|t| t.saturating_sub(now)),
            },
        );
    }
}

static SCAN_CACHE: OnceLock<Mutex<ScanCache>> = OnceLock::new();

pub fn scan_local_usage() -> UsageReport {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let mut cache = SCAN_CACHE
        .get_or_init(|| Mutex::new(ScanCache::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let providers = default_providers();
    scan_with_providers(&providers, &home, true, &mut cache)
}

#[cfg(test)]
pub fn scan_usage_from_home(home: &Path) -> UsageReport {
    let providers = default_providers();
    scan_with_providers(&providers, home, false, &mut ScanCache::default())
}

#[cfg(test)]
pub fn scan_usage_from_home_cached(home: &Path, cache: &mut ScanCache) -> UsageReport {
    let providers = default_providers();
    scan_with_providers(&providers, home, false, cache)
}

pub fn scan_with_providers(
    providers: &[Box<dyn UsageProvider>],
    home: &Path,
    allow_network: bool,
    cache: &mut ScanCache,
) -> UsageReport {
    let t0 = Instant::now();
    let now = unix_now();
    let mut collector = ScanCollector::new();

    cache.seen.clear();
    for provider in providers {
        provider.scan(home, allow_network, &mut collector, cache, now);
    }
    cache.finish_scan();

    let five_hours = build_snapshot(&collector, TimeRange::FiveHours, now);
    let today = build_snapshot(&collector, TimeRange::Today, now);
    let week = build_snapshot(&collector, TimeRange::Week, now);
    let month = build_snapshot(&collector, TimeRange::Month, now);
    let all = build_snapshot(&collector, TimeRange::All, now);

    UsageReport {
        generated_at: now,
        scan_duration_ms: t0.elapsed().as_millis() as u64,
        files_scanned: collector.files_scanned,
        five_hours,
        today,
        week,
        month,
        all,
    }
}
