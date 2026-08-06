//! Host and storage resource collection for `GET /api/v1/system/overview`.
//!
//! Everything here is read from Linux `/proc` and the filesystem with the
//! standard library. Two consequences are deliberate:
//!
//! * **No new dependency.** A registry does not need a general-purpose system
//!   information crate to report the four numbers an operator actually asks
//!   about (CPU, memory, load, disk footprint).
//! * **No fabricated values.** On a non-Linux host the `/proc`-derived fields
//!   are `None`. A zero would be indistinguishable from a genuinely idle host.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uenv_hub_types as dto;

/// One `/proc/stat` reading: total jiffies and the subset spent idle.
#[derive(Debug, Clone, Copy)]
pub struct CpuSample {
    total: u64,
    idle: u64,
    taken_at: Instant,
}

/// Keeps the previous `/proc/stat` reading so CPU usage can be reported as the
/// delta between two overview requests instead of blocking on a sleep every
/// time. Only the very first request (or one after a long idle gap) pays for a
/// short sampling window.
#[derive(Debug, Default)]
pub struct CpuMeter {
    last: Mutex<Option<CpuSample>>,
}

/// Below this gap the two samples are too close for the ratio to mean anything.
const MIN_SAMPLE_GAP: Duration = Duration::from_millis(200);
/// Blocking window used when there is no usable previous sample.
const COLD_SAMPLE_WINDOW: Duration = Duration::from_millis(100);

impl CpuMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whole-host busy percentage in `0.0..=100.0`, or `None` off Linux.
    pub fn usage_percent(&self) -> Option<f64> {
        let now = read_cpu_sample()?;
        let mut guard = self.last.lock().ok()?;

        let baseline = match *guard {
            Some(prev) if now.taken_at.duration_since(prev.taken_at) >= MIN_SAMPLE_GAP => prev,
            _ => {
                std::thread::sleep(COLD_SAMPLE_WINDOW);
                let warm = read_cpu_sample()?;
                let baseline = now;
                *guard = Some(warm);
                return busy_ratio(baseline, warm);
            }
        };

        *guard = Some(now);
        busy_ratio(baseline, now)
    }
}

fn busy_ratio(before: CpuSample, after: CpuSample) -> Option<f64> {
    let total = after.total.checked_sub(before.total)?;
    let idle = after.idle.checked_sub(before.idle)?;
    if total == 0 {
        return None;
    }
    let busy = total.saturating_sub(idle) as f64 / total as f64 * 100.0;
    Some((busy.clamp(0.0, 100.0) * 100.0).round() / 100.0)
}

/// Aggregate line of `/proc/stat`: `cpu user nice system idle iowait irq ...`.
/// Idle time is `idle + iowait`, matching how `top` and AgentENV's host
/// collector account for a CPU that is waiting rather than working.
fn read_cpu_sample() -> Option<CpuSample> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|v| v.parse().ok())
        .collect();
    if fields.len() < 5 {
        return None;
    }
    Some(CpuSample {
        total: fields.iter().sum(),
        idle: fields[3] + fields[4],
        taken_at: Instant::now(),
    })
}

/// `MemTotal` / `MemAvailable` from `/proc/meminfo`, in bytes.
fn read_memory() -> (Option<u64>, Option<u64>) {
    let Ok(text) = std::fs::read_to_string("/proc/meminfo") else {
        return (None, None);
    };
    let field = |key: &str| -> Option<u64> {
        text.lines()
            .find(|l| l.starts_with(key))?
            .split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
            .map(|kb| kb * 1024)
    };
    (field("MemTotal:"), field("MemAvailable:"))
}

fn read_load_average() -> Option<[f64; 3]> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    let mut it = text.split_whitespace();
    let mut next = || it.next()?.parse::<f64>().ok();
    Some([next()?, next()?, next()?])
}

/// Resident set size of this process, from `/proc/self/status`.
fn read_process_rss() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    text.lines()
        .find(|l| l.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()
        .map(|kb| kb * 1024)
}

/// Collect the host resource snapshot.
pub fn host_stats(meter: &CpuMeter) -> dto::HostStats {
    let (memory_total_bytes, memory_available_bytes) = read_memory();
    dto::HostStats {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_cores: std::thread::available_parallelism()
            .ok()
            .map(|n| n.get() as u64),
        cpu_usage_percent: meter.usage_percent(),
        load_average: read_load_average(),
        memory_total_bytes,
        memory_available_bytes,
        process_resident_bytes: read_process_rss(),
    }
}

/// File count and total bytes under a directory tree.
///
/// Walks iteratively rather than recursively so a pathological artifact store
/// cannot overflow the stack, and ignores unreadable entries: an overview that
/// 500s because one file lost its permissions is worse than one that
/// under-reports by that file.
fn dir_usage(root: &Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                files += 1;
                bytes += meta.len();
            }
        }
    }
    (files, bytes)
}

/// Size of the SQLite database file behind a `sqlite:` connection URL.
///
/// In-memory databases (`sqlite::memory:`) and non-file URLs report `None`
/// rather than 0, because "not backed by a file" and "an empty file" are
/// different states.
fn database_bytes(url: &str) -> Option<u64> {
    let path = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))?;
    let path = path.split('?').next().unwrap_or(path);
    if path.is_empty() || path.contains(":memory:") {
        return None;
    }
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// Collect the on-disk footprint of the artifact store and the database.
pub fn storage_stats(artifact_dir: &str, database_url: &str) -> dto::StorageStats {
    let dir = Path::new(artifact_dir);
    let exists = dir.is_dir();
    let (artifact_files, artifact_bytes) = if exists { dir_usage(dir) } else { (0, 0) };
    dto::StorageStats {
        artifact_dir: artifact_dir.to_string(),
        artifact_dir_exists: exists,
        artifact_files,
        artifact_bytes,
        database_url: database_url.to_string(),
        database_bytes: database_bytes(database_url),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_artifact_dir_is_reported_as_absent_not_empty() {
        let stats = storage_stats("/nonexistent/uenv-hub/artifacts", "sqlite://x.db");
        assert!(!stats.artifact_dir_exists);
        assert_eq!(stats.artifact_files, 0);
    }

    #[test]
    fn artifact_usage_counts_nested_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("sha256/ab")).unwrap();
        std::fs::write(tmp.path().join("sha256/ab/one"), b"0123456789").unwrap();
        std::fs::write(tmp.path().join("top"), b"abc").unwrap();

        let stats = storage_stats(&tmp.path().display().to_string(), "sqlite::memory:");
        assert!(stats.artifact_dir_exists);
        assert_eq!(stats.artifact_files, 2);
        assert_eq!(stats.artifact_bytes, 13);
        // An in-memory DB has no file to size.
        assert_eq!(stats.database_bytes, None);
    }

    #[test]
    fn database_size_follows_the_sqlite_url() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("hub.db");
        std::fs::write(&db, vec![0u8; 4096]).unwrap();
        let url = format!("sqlite://{}", db.display());
        assert_eq!(database_bytes(&url), Some(4096));
        // Query parameters must not be mistaken for part of the path.
        assert_eq!(database_bytes(&format!("{url}?mode=rwc")), Some(4096));
    }

    /// Only meaningful on Linux; elsewhere the contract is that we return
    /// `None` instead of inventing a number.
    #[test]
    fn host_stats_are_internally_consistent() {
        let stats = host_stats(&CpuMeter::new());
        assert!(!stats.os.is_empty());
        if let Some(pct) = stats.cpu_usage_percent {
            assert!((0.0..=100.0).contains(&pct));
        }
        if let (Some(total), Some(avail)) = (stats.memory_total_bytes, stats.memory_available_bytes)
        {
            assert!(avail <= total);
        }
        if cfg!(not(target_os = "linux")) {
            assert!(stats.cpu_usage_percent.is_none());
            assert!(stats.memory_total_bytes.is_none());
        }
    }
}
