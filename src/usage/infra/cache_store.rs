//! Compact binary persistence for `ScanCache` so cold starts reuse parsed log results.

use super::super::domain::{FileParse, RawQuota, UsageEvent};
use super::super::service::{FileCacheEntry, FileStamp};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAGIC: &[u8; 4] = b"IUC1";
const MAX_ENTRIES: usize = 250_000;
const MAX_EVENTS_PER_FILE: usize = 500_000;
const MAX_STR_LEN: usize = 65_536;

pub(crate) fn default_cache_path(home: &Path) -> PathBuf {
    home.join(".cache")
        .join("itsadri-terminal")
        .join("usage-cache.bin")
}

pub(crate) fn load_cache_entries(path: &Path) -> HashMap<PathBuf, FileCacheEntry> {
    try_load_cache_entries(path).unwrap_or_default()
}

pub(crate) fn save_cache_entries(path: &Path, entries: &HashMap<PathBuf, FileCacheEntry>) {
    let _ = try_save_cache_entries(path, entries);
}

fn try_load_cache_entries(path: &Path) -> io::Result<HashMap<PathBuf, FileCacheEntry>> {
    let file = File::open(path)?;
    let mut r = BufReader::with_capacity(64 * 1024, file);

    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad magic"));
    }

    let entry_count = read_u32(&mut r)? as usize;
    if entry_count > MAX_ENTRIES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "too many entries"));
    }

    let mut map = HashMap::with_capacity(entry_count.min(4096));
    for _ in 0..entry_count {
        let path_str = read_string(&mut r)?;
        let mtime_secs = read_u64(&mut r)?;
        let mtime_nanos = read_u32(&mut r)?;
        let len = read_u64(&mut r)?;

        let codex_quota = if read_u8(&mut r)? != 0 {
            Some(RawQuota {
                five_hour_pct: read_opt_f64(&mut r)?,
                five_hour_reset_at: read_opt_u64(&mut r)?,
                weekly_pct: read_opt_f64(&mut r)?,
                weekly_reset_at: read_opt_u64(&mut r)?,
            })
        } else {
            None
        };

        let event_count = read_u32(&mut r)? as usize;
        if event_count > MAX_EVENTS_PER_FILE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "too many events"));
        }

        let mut events = Vec::with_capacity(event_count.min(1024));
        // Reuse Arc<str> for repeated session_key / model values within the same file.
        let mut last_session: Option<Arc<str>> = None;
        let mut last_model: Option<Arc<str>> = None;

        for _ in 0..event_count {
            let (agent_id, agent_name) = decode_agent(read_u8(&mut r)?)?;
            let model_str = read_string(&mut r)?;
            let model = match &last_model {
                Some(prev) if prev.as_ref() == model_str.as_str() => Arc::clone(prev),
                _ => {
                    let arc: Arc<str> = Arc::from(model_str);
                    last_model = Some(Arc::clone(&arc));
                    arc
                }
            };
            let session_str = read_string(&mut r)?;
            let session_key = match &last_session {
                Some(prev) if prev.as_ref() == session_str.as_str() => Arc::clone(prev),
                _ => {
                    let arc: Arc<str> = Arc::from(session_str);
                    last_session = Some(Arc::clone(&arc));
                    arc
                }
            };

            let timestamp = read_u64(&mut r)?;
            let input_tokens = read_u64(&mut r)?;
            let output_tokens = read_u64(&mut r)?;
            let cache_read_tokens = read_u64(&mut r)?;
            let cache_write_tokens = read_u64(&mut r)?;
            let reasoning_tokens = read_u64(&mut r)?;
            let requests = read_u64(&mut r)?;
            let cost_usd = read_f64(&mut r)?;

            events.push(UsageEvent {
                agent_id,
                agent_name,
                model,
                session_key,
                timestamp,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                reasoning_tokens,
                requests,
                cost_usd,
            });
        }

        map.insert(
            PathBuf::from(path_str),
            FileCacheEntry {
                stamp: FileStamp {
                    mtime_secs,
                    mtime_nanos,
                    len,
                },
                parsed: FileParse {
                    events,
                    codex_quota,
                },
            },
        );
    }

    Ok(map)
}

fn try_save_cache_entries(
    path: &Path,
    entries: &HashMap<PathBuf, FileCacheEntry>,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("bin.tmp");
    {
        let file = File::create(&tmp_path)?;
        let mut w = BufWriter::with_capacity(64 * 1024, file);
        w.write_all(MAGIC)?;
        write_u32(&mut w, entries.len() as u32)?;

        for (p, entry) in entries {
            let p_str = p.to_string_lossy();
            write_str(&mut w, p_str.as_ref())?;
            write_u64(&mut w, entry.stamp.mtime_secs)?;
            write_u32(&mut w, entry.stamp.mtime_nanos)?;
            write_u64(&mut w, entry.stamp.len)?;

            match &entry.parsed.codex_quota {
                Some(q) => {
                    write_u8(&mut w, 1)?;
                    write_opt_f64(&mut w, q.five_hour_pct)?;
                    write_opt_u64(&mut w, q.five_hour_reset_at)?;
                    write_opt_f64(&mut w, q.weekly_pct)?;
                    write_opt_u64(&mut w, q.weekly_reset_at)?;
                }
                None => write_u8(&mut w, 0)?,
            }

            write_u32(&mut w, entry.parsed.events.len() as u32)?;
            for ev in &entry.parsed.events {
                write_u8(&mut w, encode_agent(ev.agent_id, ev.agent_name))?;
                write_str(&mut w, ev.model.as_ref())?;
                write_str(&mut w, ev.session_key.as_ref())?;
                write_u64(&mut w, ev.timestamp)?;
                write_u64(&mut w, ev.input_tokens)?;
                write_u64(&mut w, ev.output_tokens)?;
                write_u64(&mut w, ev.cache_read_tokens)?;
                write_u64(&mut w, ev.cache_write_tokens)?;
                write_u64(&mut w, ev.reasoning_tokens)?;
                write_u64(&mut w, ev.requests)?;
                write_f64(&mut w, ev.cost_usd)?;
            }
        }
        w.flush()?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

fn encode_agent(id: &str, name: &str) -> u8 {
    match (id, name) {
        ("claude", _) => 0,
        ("gemini", _) => 1,
        ("codex", _) => 2,
        ("opencode", _) => 3,
        _ => 0,
    }
}

fn decode_agent(tag: u8) -> io::Result<(&'static str, &'static str)> {
    match tag {
        0 => Ok(("claude", "Claude Code")),
        1 => Ok(("gemini", "Gemini / Antigravity")),
        2 => Ok(("codex", "Codex")),
        3 => Ok(("opencode", "OpenCode")),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "bad agent tag")),
    }
}

fn write_u8(w: &mut impl Write, v: u8) -> io::Result<()> {
    w.write_all(&[v])
}

fn write_u32(w: &mut impl Write, v: u32) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn write_u64(w: &mut impl Write, v: u64) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn write_f64(w: &mut impl Write, v: f64) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn write_opt_u64(w: &mut impl Write, v: Option<u64>) -> io::Result<()> {
    match v {
        Some(n) => {
            write_u8(w, 1)?;
            write_u64(w, n)
        }
        None => write_u8(w, 0),
    }
}

fn write_opt_f64(w: &mut impl Write, v: Option<f64>) -> io::Result<()> {
    match v {
        Some(n) => {
            write_u8(w, 1)?;
            write_f64(w, n)
        }
        None => write_u8(w, 0),
    }
}

fn write_str(w: &mut impl Write, s: &str) -> io::Result<()> {
    let bytes = s.as_bytes();
    write_u32(w, bytes.len() as u32)?;
    w.write_all(bytes)
}

fn read_u8(r: &mut impl Read) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn read_f64(r: &mut impl Read) -> io::Result<f64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(f64::from_le_bytes(b))
}

fn read_opt_u64(r: &mut impl Read) -> io::Result<Option<u64>> {
    if read_u8(r)? != 0 {
        Ok(Some(read_u64(r)?))
    } else {
        Ok(None)
    }
}

fn read_opt_f64(r: &mut impl Read) -> io::Result<Option<f64>> {
    if read_u8(r)? != 0 {
        Ok(Some(read_f64(r)?))
    } else {
        Ok(None)
    }
}

fn read_string(r: &mut impl Read) -> io::Result<String> {
    let len = read_u32(r)? as usize;
    if len > MAX_STR_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "string too long"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid utf8"))
}
