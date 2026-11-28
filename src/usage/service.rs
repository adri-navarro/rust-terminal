//! Application service (`ScanCache` + report orchestration) for `usage`.

use super::domain::{
    FileParse, LiveAgentQuotas, ScanCollector, UsageReport, build_all_snapshots,
};
use super::infra::{
    cache_store::{default_cache_path, load_cache_entries, save_cache_entries},
    claude, default_providers, fs_helpers::unix_now, gemini,
};
use super::ports::UsageProvider;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileStamp {
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
    pub len: u64,
}

impl FileStamp {
    fn from_path(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        let dur = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            mtime_secs: dur.as_secs(),
            mtime_nanos: dur.subsec_nanos(),
            len: meta.len(),
        })
    }
}

pub(crate) struct FileCacheEntry {
    pub stamp: FileStamp,
    pub parsed: FileParse,
}

/// Per-file parse results keyed by path, reused while a file's (mtime, len) is unchanged.
#[derive(Default)]
pub struct ScanCache {
    entries: HashMap<PathBuf, FileCacheEntry>,
    seen: HashSet<PathBuf>,
    dirty: bool,
}

impl ScanCache {
    pub fn load_from_disk(path: &Path) -> Self {
        Self {
            entries: load_cache_entries(path),
            seen: HashSet::new(),
            dirty: false,
        }
    }

    pub fn save_to_disk(&mut self, path: &Path) {
        if self.dirty {
            save_cache_entries(path, &self.entries);
            self.dirty = false;
        }
    }

    #[allow(dead_code)]
    pub fn scan_file(
        &mut self,
        path: &Path,
        out: &mut ScanCollector,
        now: u64,
        parse: impl FnOnce() -> Option<FileParse>,
    ) {
        self.seen.insert(path.to_path_buf());
        let stamp = FileStamp::from_path(path);
        if let (Some(st), Some(entry)) = (stamp, self.entries.get(path)) {
            if entry.stamp == st {
                apply_file_parse(&entry.parsed, out, now);
                return;
            }
        }
        let Some(parsed) = parse() else {
            if self.entries.remove(path).is_some() {
                self.dirty = true;
            }
            return;
        };
        apply_file_parse(&parsed, out, now);
        match stamp {
            Some(st) => {
                self.entries
                    .insert(path.to_path_buf(), FileCacheEntry { stamp: st, parsed });
                self.dirty = true;
            }
            None => {
                if self.entries.remove(path).is_some() {
                    self.dirty = true;
                }
            }
        }
    }

    /// Scans a batch of files: cache hits are applied immediately, cache misses are parsed
    /// in parallel across worker threads using work-stealing.
    pub fn scan_files_parallel<F>(
        &mut self,
        paths: &[PathBuf],
        out: &mut ScanCollector,
        now: u64,
        parse_fn: F,
    ) where
        F: Fn(&Path, u64) -> Option<FileParse> + Sync,
    {
        if paths.is_empty() {
            return;
        }

        let mut misses: Vec<(&PathBuf, Option<FileStamp>)> = Vec::new();
        for path in paths {
            out.files_scanned += 1;
            self.seen.insert(path.clone());
            let stamp = FileStamp::from_path(path);
            if let (Some(st), Some(entry)) = (stamp, self.entries.get(path)) {
                if entry.stamp == st {
                    apply_file_parse(&entry.parsed, out, now);
                    continue;
                }
            }
            misses.push((path, stamp));
        }

        if misses.is_empty() {
            return;
        }

        let parsed_results: Vec<Option<FileParse>> = if misses.len() <= 2 {
            misses.iter().map(|(p, _)| parse_fn(p, now)).collect()
        } else {
            let workers = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .clamp(2, 8)
                .min(misses.len());
            let next_idx = AtomicUsize::new(0);
            let slots: Vec<Mutex<Option<FileParse>>> =
                (0..misses.len()).map(|_| Mutex::new(None)).collect();

            std::thread::scope(|s| {
                for _ in 0..workers {
                    s.spawn(|| loop {
                        let idx = next_idx.fetch_add(1, Ordering::Relaxed);
                        if idx >= misses.len() {
                            break;
                        }
                        let res = parse_fn(misses[idx].0, now);
                        *slots[idx].lock().unwrap_or_else(|e| e.into_inner()) = res;
                    });
                }
            });

            slots
                .into_iter()
                .map(|m| m.into_inner().unwrap_or_else(|e| e.into_inner()))
                .collect()
        };

        for ((path, stamp), parsed_opt) in misses.into_iter().zip(parsed_results.into_iter()) {
            let Some(parsed) = parsed_opt else {
                if self.entries.remove(path).is_some() {
                    self.dirty = true;
                }
                continue;
            };
            apply_file_parse(&parsed, out, now);
            match stamp {
                Some(st) => {
                    self.entries
                        .insert(path.clone(), FileCacheEntry { stamp: st, parsed });
                    self.dirty = true;
                }
                None => {
                    if self.entries.remove(path).is_some() {
                        self.dirty = true;
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub fn cached_files(&self) -> usize {
        self.entries.len()
    }

    fn finish_scan(&mut self) {
        let seen = std::mem::take(&mut self.seen);
        let before = self.entries.len();
        self.entries.retain(|path, _| seen.contains(path));
        if self.entries.len() != before {
            self.dirty = true;
        }
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
static LIVE_QUOTA_REFRESHING: AtomicBool = AtomicBool::new(false);

/// Fast local-only usage scan backed by a persistent disk cache + parallel parsing.
/// Live quotas (Claude OAuth / Antigravity CLI) are read from their on-disk caches immediately
/// without blocking on network or external CLI subprocesses; call `refresh_live_quotas_async`
/// to refresh stale live quotas in the background.
pub fn scan_local_usage() -> UsageReport {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let cache_path = default_cache_path(&home);
    let mut cache = SCAN_CACHE
        .get_or_init(|| Mutex::new(ScanCache::load_from_disk(&cache_path)))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let providers = default_providers();
    let report = scan_with_providers(&providers, &home, false, &mut cache);
    cache.save_to_disk(&cache_path);
    report
}

/// Refreshes stale Claude OAuth and Antigravity CLI live quota caches in parallel on background
/// threads (with strict timeouts). Invokes `on_updated` only if fresh quota data was written.
pub fn refresh_live_quotas_async<F>(on_updated: F)
where
    F: FnOnce() + Send + 'static,
{
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let now = unix_now();
    let need_claude = claude::claude_quota_needs_refresh(&home, now);
    let need_agy = gemini::agy_quota_needs_refresh(&home, now);
    if !need_claude && !need_agy {
        return;
    }
    if LIVE_QUOTA_REFRESHING.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        let mut updated = false;
        std::thread::scope(|s| {
            let h_claude = need_claude.then(|| {
                let home = &home;
                s.spawn(move || claude::refresh_claude_oauth_cache(home, now))
            });
            let h_agy = need_agy.then(|| {
                let home = &home;
                s.spawn(move || gemini::refresh_agy_usage_cache(home, now))
            });
            if let Some(h) = h_claude {
                updated |= h.join().unwrap_or(false);
            }
            if let Some(h) = h_agy {
                updated |= h.join().unwrap_or(false);
            }
        });
        LIVE_QUOTA_REFRESHING.store(false, Ordering::Release);
        if updated {
            on_updated();
        }
    });
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

    let (five_hours, today, week, month, all) = build_all_snapshots(&collector, now);

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
