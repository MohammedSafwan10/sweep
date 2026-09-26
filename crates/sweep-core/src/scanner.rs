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

use crate::model::{DirEntry, ScanIssues, ScanOptions, ScanReport, VolumeStats, SCHEMA_VERSION};
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
    scan_dir_with_allocated(root, opts, progress, false)
}

/// As [`scan_dir`], with an optional slower per-file allocation query.
pub fn scan_dir_with_allocated(
    root: &Path,
    opts: &ScanOptions,
    progress: Option<mpsc::SyncSender<LiveProgress>>,
    allocated: bool,
) -> Result<ScanReport, ScanError> {
    let canon = std::fs::canonicalize(root)
        .map_err(|e| ScanError::RootUnreadable(root.display().to_string(), e.to_string()))?;
    if !canon.is_dir() {
        return Err(ScanError::NotADirectory(root.display().to_string()));
    }
    // An unreadable root must fail outright rather than returning a
    // plausible zero total. Unreadable descendants remain report warnings.
    std::fs::read_dir(&canon)
        .map_err(|e| ScanError::RootUnreadable(root.display().to_string(), e.to_string()))?;
    let volume = fs4::statvfs(&canon).ok().map(|stats| VolumeStats {
        total_bytes: stats.total_space(),
        free_bytes: stats.free_space(),
        available_bytes: stats.available_space(),
    });

    let partials: Arc<Mutex<Vec<ShardData>>> = Arc::new(Mutex::new(Vec::new()));
    let warnings: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let suppressed = Arc::new(AtomicUsize::new(0));
    let permission_denied = Arc::new(AtomicU64::new(0));
    let not_found = Arc::new(AtomicU64::new(0));
    let other_issues = Arc::new(AtomicU64::new(0));
    let retry_paths: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
    let files_seen = Arc::new(AtomicU64::new(0));
    let bytes_seen = Arc::new(AtomicU64::new(0));
    let allocated_seen = Arc::new(AtomicU64::new(0));
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
        let permission_denied = Arc::clone(&permission_denied);
        let not_found = Arc::clone(&not_found);
        let other_issues = Arc::clone(&other_issues);
        let retry_paths = Arc::clone(&retry_paths);
        let files_seen = Arc::clone(&files_seen);
        let bytes_seen = Arc::clone(&bytes_seen);
        let allocated_seen = Arc::clone(&allocated_seen);
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
                    record_issue(
                        err.io_error().map(std::io::Error::kind),
                        error_path(&err),
                        &permission_denied,
                        &not_found,
                        &other_issues,
                        &retry_paths,
                    );
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
            if !ft.is_file() {
                return WalkState::Continue;
            }
            let meta = match entry.metadata() {
                Ok(meta) => meta,
                Err(err) => {
                    record_issue(
                        err.io_error().map(std::io::Error::kind),
                        Some(entry.path()),
                        &permission_denied,
                        &not_found,
                        &other_issues,
                        &retry_paths,
                    );
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
            if metadata_is_link_or_reparse(&meta) {
                return WalkState::Continue;
            }
            let len = meta.len();
            if allocated {
                match allocated_size(entry.path(), &meta) {
                    Ok(bytes) => {
                        allocated_seen.fetch_add(bytes, Ordering::Relaxed);
                    }
                    Err(err) => {
                        record_issue(
                            Some(err.kind()),
                            Some(entry.path()),
                            &permission_denied,
                            &not_found,
                            &other_issues,
                            &retry_paths,
                        );
                        if let Ok(mut w) = warnings.lock() {
                            if w.len() < MAX_WARNINGS {
                                w.push(trim_warning(&format!(
                                    "allocated size unavailable for {}: {err}",
                                    entry.path().display()
                                )));
                            } else {
                                suppressed.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }
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
    let complete = warnings.is_empty() && suppressed.load(Ordering::Relaxed) == 0;
    let retry_paths = retry_paths
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    Ok(ScanReport {
        schema_version: SCHEMA_VERSION,
        root: display_path(&canon),
        complete,
        volume,
        total_bytes: root_bytes,
        allocated_bytes: allocated.then(|| allocated_seen.load(Ordering::Relaxed)),
        total_files: root_files,
        total_dirs,
        warnings_suppressed: suppressed.load(Ordering::Relaxed),
        issues: ScanIssues {
            permission_denied: permission_denied.load(Ordering::Relaxed),
            not_found: not_found.load(Ordering::Relaxed),
            other: other_issues.load(Ordering::Relaxed),
        },
        retry_paths,
        warnings,
        entries,
    })
}

/// Byte size of one directory subtree (parallel; for detectors).
/// Symlinks are skipped. Returns bytes + file count.
///
/// Canonicalizes first so Windows `\\?\` long-path resolution applies:
/// raw paths silently fail on >260-char entries (e.g. uv's hash cache),
/// which undercounts by gigabytes. One worker per core with atomic
/// totals — a single Relaxed add per file is ~ns vs ~µs of I/O.
///
/// Inner parallelism is capped at 2 threads: detectors already run in
/// parallel via rayon, so uncapped inner pools oversubscribe (16 detectors
/// × N walkers) and thrash the disk. Best-effort sizing: unreadable
/// entries are skipped silently (conservative undercount, never over).
pub fn size_of_dir(path: &Path) -> (u64, u64) {
    if is_link_or_reparse(path) {
        return (0, 0);
    }
    // Long-path fix: canonicalize for the verbatim prefix. Fall back to
    // the raw path when canonicalization fails (dir may vanish mid-scan).
    let root: PathBuf = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let bytes = Arc::new(AtomicU64::new(0));
    let files = Arc::new(AtomicU64::new(0));
    let mut builder = WalkBuilder::new(&root);
    builder
        .hidden(false)
        .git_ignore(false)
        .ignore(false)
        .git_exclude(false)
        .git_global(false)
        .parents(false)
        .follow_links(false)
        .threads(2)
        .filter_entry(|entry| !is_link_or_reparse(entry.path()));
    builder.build_parallel().run(|| {
        let bytes = Arc::clone(&bytes);
        let files = Arc::clone(&files);
        Box::new(move |entry: Result<ignore::DirEntry, ignore::Error>| {
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };
            if entry.path_is_symlink() {
                return WalkState::Continue;
            }
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                if let Ok(meta) = entry.metadata() {
                    if !metadata_is_link_or_reparse(&meta) {
                        files.fetch_add(1, Ordering::Relaxed);
                        bytes.fetch_add(meta.len(), Ordering::Relaxed);
                    }
                }
            }
            WalkState::Continue
        })
    });
    (bytes.load(Ordering::Relaxed), files.load(Ordering::Relaxed))
}

#[cfg(windows)]
fn allocated_size(path: &Path, _meta: &std::fs::Metadata) -> std::io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
    use windows_sys::Win32::Storage::FileSystem::GetCompressedFileSizeW;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut high = 0u32;
    // SAFETY: `wide` is NUL terminated and lives through the call; `high`
    // is a valid writable output pointer. The walker already rejected links.
    let low = unsafe {
        SetLastError(0);
        GetCompressedFileSizeW(wide.as_ptr(), &mut high)
    };
    // INVALID_FILE_SIZE is also a valid low word, so check last error.
    if low == u32::MAX {
        // SAFETY: GetLastError has no pointer arguments and reads thread state.
        let code = unsafe { GetLastError() };
        if code != 0 {
            return Err(std::io::Error::from_raw_os_error(code as i32));
        }
    }
    Ok((u64::from(high) << 32) | u64::from(low))
}

#[cfg(unix)]
fn allocated_size(_path: &Path, meta: &std::fs::Metadata) -> std::io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(meta.blocks().saturating_mul(512))
}

#[cfg(not(any(windows, unix)))]
fn allocated_size(_path: &Path, _meta: &std::fs::Metadata) -> std::io::Result<u64> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "allocated size is unavailable on this platform",
    ))
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
    std::fs::symlink_metadata(path)
        .map(|meta| metadata_is_link_or_reparse(&meta))
        .unwrap_or(true)
}

pub(crate) fn metadata_is_link_or_reparse(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_type().is_symlink() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

/// Display path without the Windows `\\?\` verbatim prefix from canonicalize.
/// Human display form of a path: strips the Windows `\\?\` verbatim prefix
/// from canonicalized paths. Display-only and lossy (non-UTF8 names may
/// collapse) — never use for deletion or identity comparisons.
pub fn display_path(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.display().to_string();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    p.to_path_buf()
}

fn error_path(err: &ignore::Error) -> Option<&Path> {
    match err {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            error_path(err)
        }
        ignore::Error::Partial(errors) => errors.iter().find_map(error_path),
        _ => None,
    }
}

fn record_issue(
    kind: Option<std::io::ErrorKind>,
    path: Option<&Path>,
    permission_denied: &AtomicU64,
    not_found: &AtomicU64,
    other: &AtomicU64,
    retry_paths: &Mutex<Vec<PathBuf>>,
) {
    match kind {
        Some(std::io::ErrorKind::PermissionDenied) => {
            permission_denied.fetch_add(1, Ordering::Relaxed);
        }
        Some(std::io::ErrorKind::NotFound) => {
            not_found.fetch_add(1, Ordering::Relaxed);
        }
        _ => {
            other.fetch_add(1, Ordering::Relaxed);
        }
    }
    if let Some(path) = path {
        if let Ok(mut paths) = retry_paths.lock() {
            if paths.len() < MAX_WARNINGS {
                paths.push(display_path(path));
            }
        }
    }
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
        assert!(report.complete);
        assert!(report.volume.as_ref().is_some_and(|v| v.total_bytes > 0));
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
    fn issues_keep_counts_and_bounded_retry_paths() {
        let denied = AtomicU64::new(0);
        let missing = AtomicU64::new(0);
        let other = AtomicU64::new(0);
        let paths = Mutex::new(Vec::new());
        for index in 0..(MAX_WARNINGS + 5) {
            let path = PathBuf::from(format!("missing-{index}"));
            record_issue(
                Some(std::io::ErrorKind::PermissionDenied),
                Some(&path),
                &denied,
                &missing,
                &other,
                &paths,
            );
        }
        record_issue(
            Some(std::io::ErrorKind::NotFound),
            None,
            &denied,
            &missing,
            &other,
            &paths,
        );
        assert_eq!(denied.load(Ordering::Relaxed), (MAX_WARNINGS + 5) as u64);
        assert_eq!(missing.load(Ordering::Relaxed), 1);
        assert_eq!(paths.lock().unwrap().len(), MAX_WARNINGS);
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
