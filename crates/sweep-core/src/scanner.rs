//! Parallel directory scanner.
//!
//! Design notes:
//! - Uses `ignore::WalkParallel` (ripgrep's engine): one worker per core,
//!   each aggregating into a thread-local map merged once at the end.
//!   No shared mutable state in the hot loop, no per-file channel traffic.
//! - Symlinks / Windows junctions are never followed and never counted,
//!   so reparse-point loops can't hang or double-count the scan.
//! - Hidden files ARE included (dev caches live in hidden dirs like
//!   `.cargo`, `AppData`); gitignore files are NOT respected — this is a
//!   disk tool, not a source tool.
//! - Sizes are logical (apparent) bytes from file metadata, not allocated
//!   clusters: hardlinked files count once per link, sparse files (like a
//!   Docker vhdx) report their logical size. Receipts therefore say
//!   "would free ~N logical bytes", matching what caches re-download.

use crate::model::{DirEntry, ScanOptions, ScanReport, SCHEMA_VERSION};
use ignore::{WalkBuilder, WalkState};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("scan root is not accessible ({0}): {1}")]
    RootUnreadable(String, String),
    #[error("scan root is not a directory: {0}")]
    NotADirectory(String),
}

/// Live progress snapshot streamed to UIs (TUI progress bar, agent logs).
#[derive(Debug, Clone, Copy)]
pub struct LiveProgress {
    pub files_seen: u64,
    pub bytes_seen: u64,
}

/// Upper bound on stored warning strings; the rest are only counted.
const MAX_WARNINGS: usize = 25;
/// How often (files) a worker reports progress. `try_send` never blocks.
const PROGRESS_EVERY_FILES: u64 = 1024;

/// Per-thread aggregation. Moved into the worker closure and flushed into
/// the shared vec by `Drop` when the thread's walker finishes.
struct Shard {
    /// dir path -> (bytes, files contained)
    dirs: HashMap<PathBuf, (u64, u64)>,
    /// big files at/above `min_file_bytes`: path -> bytes
    big_files: Vec<(PathBuf, u64)>,
    partials: Arc<Mutex<Vec<ShardData>>>,
    files_seen: Arc<AtomicU64>,
    bytes_seen: Arc<AtomicU64>,
    progress: Option<mpsc::SyncSender<LiveProgress>>,
    files: u64,
    /// Unreported deltas since the last progress snapshot.
    since_files: u64,
    since_bytes: u64,
}

impl Shard {
    /// Add deltas to the global counters and emit a snapshot (non-blocking).
    fn flush_progress(&mut self) {
        if self.since_files == 0 {
            return;
        }
        let f = self
            .files_seen
            .fetch_add(self.since_files, Ordering::Relaxed)
            + self.since_files;
        let b = self
            .bytes_seen
            .fetch_add(self.since_bytes, Ordering::Relaxed)
            + self.since_bytes;
        self.since_files = 0;
        self.since_bytes = 0;
        if let Some(tx) = &self.progress {
            let _ = tx.try_send(LiveProgress {
                files_seen: f,
                bytes_seen: b,
            });
        }
    }
}

struct ShardData {
    dirs: HashMap<PathBuf, (u64, u64)>,
    big_files: Vec<(PathBuf, u64)>,
    files: u64,
}

impl Drop for Shard {
    fn drop(&mut self) {
        // Final flush so consumers always see the totals, even for scans
        // too small to ever hit the batch threshold.
        self.flush_progress();
        if let Ok(mut out) = self.partials.lock() {
            out.push(ShardData {
                dirs: std::mem::take(&mut self.dirs),
                big_files: std::mem::take(&mut self.big_files),
                files: self.files,
            });
        }
    }
}

/// Scan `root` and return a ranked size report.
///
/// `progress` is an optional bounded channel sender; workers `try_send`
/// snapshots and never block on a slow consumer. Pass `None` for CLI use.
pub fn scan_dir(
    root: &Path,
    opts: &ScanOptions,
    progress: Option<mpsc::SyncSender<LiveProgress>>,
) -> Result<ScanReport, ScanError> {
    let canon = std::fs::canonicalize(root)
        .map_err(|e| ScanError::RootUnreadable(root.display().to_string(), e.to_string()))?;
    if !canon.is_dir() {
        return Err(ScanError::NotADirectory(root.display().to_string()));
    }

    let partials: Arc<Mutex<Vec<ShardData>>> = Arc::new(Mutex::new(Vec::new()));
    let warnings: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let suppressed = Arc::new(AtomicUsize::new(0));
    let files_seen = Arc::new(AtomicU64::new(0));
    let bytes_seen = Arc::new(AtomicU64::new(0));
    let min_file_bytes = opts.min_file_bytes;

    let mut builder = WalkBuilder::new(&canon);
    builder
        .hidden(false)
        .git_ignore(false)
        .ignore(false)
        .git_exclude(false)
        .git_global(false)
        .parents(false)
        .follow_links(false)
        .same_file_system(opts.same_filesystem);

    builder.build_parallel().run(|| {
        let partials = Arc::clone(&partials);
        let warnings = Arc::clone(&warnings);
        let suppressed = Arc::clone(&suppressed);
        let files_seen = Arc::clone(&files_seen);
        let bytes_seen = Arc::clone(&bytes_seen);
        let progress = progress.clone();
        let root = canon.clone();
        let mut shard = Shard {
            dirs: HashMap::new(),
            big_files: Vec::new(),
            partials,
            files_seen,
            bytes_seen,
            progress,
            files: 0,
            since_files: 0,
            since_bytes: 0,
        };
        Box::new(move |entry: Result<ignore::DirEntry, ignore::Error>| {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    if let Ok(mut w) = warnings.lock() {
                        if w.len() < MAX_WARNINGS {
                            w.push(trim_warning(&err.to_string()));
                        } else {
                            suppressed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    return WalkState::Continue;
                }
            };
            // Never follow or count links: with follow_links(false) these are
            // yielded, not descended into — skip them entirely. Unknown
            // file types are skipped too rather than miscounted.
            // Junctions need the reparse check: they pose as plain dirs.
            let Some(ft) = entry.file_type() else {
                return WalkState::Continue;
            };
            if ft.is_symlink() {
                return WalkState::Continue;
            }
            if ft.is_dir() {
                if is_link_or_reparse(entry.path()) {
                    return WalkState::Skip;
                }
                // Register empty dirs too so they show with 0 bytes.
                shard
                    .dirs
                    .entry(entry.path().to_path_buf())
                    .or_insert((0, 0));
                return WalkState::Continue;
            }
            if !ft.is_file() || is_link_or_reparse(entry.path()) {
                return WalkState::Continue;
            }
            let len = match entry.metadata() {
                Ok(meta) => meta.len(),
                Err(err) => {
                    if let Ok(mut w) = warnings.lock() {
                        if w.len() < MAX_WARNINGS {
                            w.push(trim_warning(&err.to_string()));
                        } else {
                            suppressed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    return WalkState::Continue;
                }
            };
            shard.files += 1;
            shard.since_files += 1;
            shard.since_bytes += len;
            let path = entry.path();
            for anc in path.ancestors().skip(1) {
                if !anc.starts_with(&root) {
                    break;
                }
                let slot = shard.dirs.entry(anc.to_path_buf()).or_insert((0, 0));
                slot.0 += len;
                slot.1 += 1;
                if anc == root {
                    break;
                }
            }
            if len >= min_file_bytes {
                shard.big_files.push((path.to_path_buf(), len));
            }
            if shard.since_files >= PROGRESS_EVERY_FILES {
                shard.flush_progress();
            }
            WalkState::Continue
        })
        // `shard` drops here per worker thread, flushing into `partials`.
    });

    // Merge shards. A poisoned mutex means a worker panicked; recover its
    // partial data instead of crashing the whole scan.
    let mut merged: HashMap<PathBuf, (u64, u64)> = HashMap::new();
    let mut big_files: Vec<(PathBuf, u64)> = Vec::new();
    let mut worker_files: u64 = 0;
    let partials = partials
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for shard in partials.iter() {
        worker_files += shard.files;
        for (path, (bytes, files)) in &shard.dirs {
            let slot = merged.entry(path.clone()).or_insert((0, 0));
            slot.0 += bytes;
            slot.1 += files;
        }
        big_files.extend(shard.big_files.iter().cloned());
    }
    drop(partials);

    let (root_bytes, root_files) = merged.get(&canon).copied().unwrap_or((0, 0));
    let total_dirs = merged.len() as u64;

    // Ranked entries: dirs + individually tracked big files.
    let mut entries: Vec<DirEntry> = Vec::with_capacity(merged.len() + big_files.len());
    for (path, (bytes, files)) in &merged {
        if *bytes < opts.min_bytes {
            continue;
        }
        let depth = path
            .strip_prefix(&canon)
            .map(|p| p.components().count())
            .unwrap_or(0);
        entries.push(DirEntry {
            path: display_path(path),
            bytes: *bytes,
            files: *files,
            depth,
            is_dir: true,
        });
    }
    for (path, bytes) in &big_files {
        if *bytes < opts.min_bytes {
            continue;
        }
        let depth = path
            .strip_prefix(&canon)
            .map(|p| p.components().count())
            .unwrap_or(0);
        entries.push(DirEntry {
            path: display_path(path),
            bytes: *bytes,
            files: 1,
            depth,
            is_dir: false,
        });
    }
    entries.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path)));
    entries.truncate(opts.top);

    // Cross-check the two independent counts: every counted file attributes
    // itself up to the root, so the worker sum and the root aggregate must
    // agree. A mismatch is reported, never hidden.
    let mut warnings = warnings
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if worker_files != root_files {
        warnings.push(format!(
            "internal count mismatch: workers saw {worker_files} files, root aggregate has {root_files}"
        ));
    }
    // A full or unconsumed channel must not prevent the scan returning.
    if let Some(tx) = progress {
        let _ = tx.try_send(LiveProgress {
            files_seen: root_files,
            bytes_seen: root_bytes,
        });
    }
    Ok(ScanReport {
        schema_version: SCHEMA_VERSION,
        root: display_path(&canon),
        total_bytes: root_bytes,
        total_files: root_files,
        total_dirs,
        warnings_suppressed: suppressed.load(Ordering::Relaxed),
        warnings,
        entries,
    })
}

/// Byte size of one directory subtree (single-threaded; for detectors).
/// Symlinks are skipped. Returns bytes + file count.
pub fn size_of_dir(path: &Path) -> (u64, u64) {
    if is_link_or_reparse(path) {
        return (0, 0);
    }
    let mut bytes: u64 = 0;
    let mut files: u64 = 0;
    let walker = WalkBuilder::new(path)
        .hidden(false)
        .git_ignore(false)
        .ignore(false)
        .git_exclude(false)
        .git_global(false)
        .parents(false)
        .follow_links(false)
        .filter_entry(|entry| !is_link_or_reparse(entry.path()))
        .build();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        if entry.path_is_symlink() {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir && is_link_or_reparse(entry.path()) {
            continue;
        }
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            files += 1;
            bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    (bytes, files)
}

/// Parse human sizes: `512`, `10KB`, `1.5 MB`, `2GiB` (case-insensitive).
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty size".to_string());
    }
    let split = s.find(|c: char| c.is_alphabetic()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let num: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("invalid size number: {s}"))?;
    if !num.is_finite() || num < 0.0 {
        return Err(format!("invalid size (not a finite positive number): {s}"));
    }
    let mult: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "tib" => 1024.0_f64.powi(4),
        other => return Err(format!("unknown size unit: {other}")),
    };
    let bytes = num * mult;
    if !bytes.is_finite() || bytes >= u64::MAX as f64 {
        return Err(format!("size exceeds the supported range: {s}"));
    }
    Ok(bytes as u64)
}

/// Format bytes as `1.4 GiB` (1024-based, one decimal).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// True for symlinks and (on Windows) junctions / other reparse points.
///
/// `DirEntry::file_type().is_symlink()` misses junctions: they report as
/// plain directories, so without this check scans descend into them —
/// looping or double-counting trees like `WindowsApps` or OneDrive roots.
pub(crate) fn is_link_or_reparse(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        std::fs::symlink_metadata(path)
            .map(|m| {
                m.file_type().is_symlink()
                    || m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            })
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        std::fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }
}

/// Display path without the Windows `\\?\` verbatim prefix from canonicalize.
fn display_path(p: &Path) -> PathBuf {
    let s = p.display().to_string();
    #[cfg(windows)]
    {
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    PathBuf::from(s)
}

fn trim_warning(s: &str) -> String {
    const MAX: usize = 300;
    if s.len() > MAX {
        // Byte-slicing could split a multi-byte char and panic; walk back
        // to a boundary instead (`floor_char_boundary` needs Rust 1.91,
        // MSRV here is 1.74).
        let mut end = MAX.min(s.len());
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc::sync_channel;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("a").join("deep")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(root.join("a").join("f1.bin"), vec![0u8; 100]).unwrap();
        fs::write(root.join("a").join("deep").join("f2.bin"), vec![0u8; 300]).unwrap();
        fs::write(root.join("b").join("f3.bin"), vec![0u8; 50]).unwrap();
        fs::write(root.join("top.bin"), vec![0u8; 200]).unwrap();
        dir
    }

    #[test]
    fn totals_and_ranking_are_exact() {
        let dir = fixture();
        let opts = ScanOptions {
            top: 10,
            min_bytes: 0,
            min_file_bytes: u64::MAX,
            same_filesystem: false,
        };
        let report = scan_dir(dir.path(), &opts, None).unwrap();
        assert_eq!(report.total_bytes, 650);
        assert_eq!(report.total_files, 4);
        // root first, then a (400), then b (50)
        assert_eq!(report.entries[0].bytes, 650);
        assert!(report.entries[0].is_dir);
        let a = report
            .entries
            .iter()
            .find(|e| e.path.ends_with("a"))
            .unwrap();
        assert_eq!((a.bytes, a.files, a.depth), (400, 2, 1));
    }

    #[test]
    fn min_bytes_filters_small_entries() {
        let dir = fixture();
        let opts = ScanOptions {
            top: 10,
            min_bytes: 100,
            min_file_bytes: u64::MAX,
            same_filesystem: false,
        };
        let report = scan_dir(dir.path(), &opts, None).unwrap();
        assert!(report.entries.iter().all(|e| e.bytes >= 100));
        assert!(!report.entries.iter().any(|e| e.path.ends_with("b")));
    }

    #[test]
    fn big_files_are_listed_individually() {
        let dir = fixture();
        let opts = ScanOptions {
            top: 10,
            min_bytes: 0,
            min_file_bytes: 150,
            same_filesystem: false,
        };
        let report = scan_dir(dir.path(), &opts, None).unwrap();
        let files: Vec<_> = report.entries.iter().filter(|e| !e.is_dir).collect();
        // f2 (300) and top.bin (200) qualify; f1 (100) and f3 (50) do not.
        assert_eq!(files.len(), 2);
        assert!(files
            .iter()
            .any(|e| e.path.ends_with("f2.bin") && e.bytes == 300));
    }

    #[test]
    fn missing_root_is_an_error_not_a_panic() {
        let opts = ScanOptions::default();
        let err = scan_dir(Path::new("/definitely/not/here/sweep-test"), &opts, None);
        assert!(matches!(err, Err(ScanError::RootUnreadable(..))));
    }

    #[test]
    fn progress_channel_receives_snapshots() {
        // The final per-worker flush guarantees snapshots even for scans
        // far below the batch threshold.
        let dir = fixture();
        let (tx, rx) = sync_channel(64);
        let opts = ScanOptions {
            top: 10,
            min_bytes: 0,
            min_file_bytes: u64::MAX,
            same_filesystem: false,
        };
        let report = scan_dir(dir.path(), &opts, Some(tx)).unwrap();
        assert_eq!(report.total_files, 4);
        let snaps: Vec<_> = rx.try_iter().collect();
        assert!(!snaps.is_empty(), "expected progress snapshots");
        assert!(snaps.iter().all(|s| s.files_seen > 0));
        assert!(snaps.iter().map(|s| s.files_seen).max().unwrap() <= 4);
    }

    #[test]
    fn parse_size_table() {
        assert_eq!(parse_size("512").unwrap(), 512);
        assert_eq!(parse_size("10KB").unwrap(), 10 * 1024);
        assert_eq!(parse_size("1.5 MB").unwrap(), 1_572_864);
        assert_eq!(parse_size("2gib").unwrap(), 2 * 1024 * 1024 * 1024);
        assert!(parse_size("10XB").is_err());
        assert!(parse_size("").is_err());
        assert!(parse_size("-5MB").is_err());
        assert!(parse_size("nan").is_err());
        assert!(parse_size("1e308GB").is_err(), "overflow must not saturate");
    }

    #[test]
    fn trim_warning_never_splits_utf8() {
        let s = "é".repeat(500); // 1000 bytes of 2-byte chars
        let trimmed = trim_warning(&s);
        assert!(trimmed.len() <= 300 + "…".len());
        assert!(trimmed.ends_with('…'));
    }

    #[test]
    fn format_bytes_table() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GiB");
    }

    #[test]
    fn size_of_dir_matches() {
        let dir = fixture();
        let (bytes, files) = size_of_dir(dir.path());
        assert_eq!((bytes, files), (650, 4));
    }
}
