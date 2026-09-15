//! Cleaning planner + executor.
//!
//! Safety contract (see SAFETY.md):
//! - Nothing happens without `execute: true` (dry-run is the default).
//! - `Danger` findings always need `force_danger`, even with `All`.
//! - Default destination is the OS Recycle Bin / Trash (`to_trash`).
//! - v1 only deletes paths. `RunCommand`/`Manual` findings are reported
//!   with instructions, never executed — running foreign CLIs with
//!   destructive flags deserves its own audited milestone.
//! - Symlinks are never deleted through: a swapped link between plan and
//!   execute turns into an error, not a delete.
//! - Structural guardrails refuse empty paths, filesystem roots and any
//!   path containing `..`, even if a detector ever emits one.

use crate::model::{
    CleanAction, CleanOptions, CleanReceipt, Disposition, Finding, PlannedItem, RemovedItem,
    SkippedItem, SCHEMA_VERSION,
};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

pub struct CleanPlan {
    pub items: Vec<PlannedItem>,
    pub total_bytes: u64,
}

/// Build the plan: filter by safety level, refuse links and protected
/// paths, drop vanished paths, total bytes.
pub fn plan(findings: &[Finding], opts: &CleanOptions) -> CleanPlan {
    let mut items = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut paths = HashSet::new();
    let resolved: Vec<_> = findings
        .iter()
        .map(|finding| action_path(finding).and_then(|path| std::fs::canonicalize(path).ok()))
        .collect();
    // Ancestors that are themselves eligible for removal in this run.
    // A filtered-out (e.g. CAUTION under `--only safe`) or non-path finding
    // must not hide an eligible child: it isn't being deleted, so the child
    // still needs its own plan entry.
    let eligible: Vec<PathBuf> = findings
        .iter()
        .zip(&resolved)
        .filter(|(finding, resolved)| {
            matches!(finding.action, CleanAction::RemovePath { .. })
                && opts.include.allows(finding.safety)
                && (finding.safety != crate::model::Safety::Danger || opts.force_danger)
                && resolved.is_some()
        })
        .filter_map(|(_, resolved)| resolved.clone())
        .collect();
    // Nested RemovePath findings that will NOT be deleted in this run
    // (filtered by level, danger-gated, or invalid). A candidate containing
    // any of these must itself be skipped: deleting it would wipe the
    // excluded child (e.g. a SAFE parent over a CAUTION child under
    // `--only safe`). This preserves the safety gate.
    let withheld: Vec<PathBuf> = findings
        .iter()
        .zip(&resolved)
        .filter_map(|(finding, resolved)| match (&finding.action, resolved) {
            (CleanAction::RemovePath { .. }, Some(path)) if !eligible.contains(path) => {
                Some(path.clone())
            }
            _ => None,
        })
        .collect();
    for finding in findings {
        if !opts.include.allows(finding.safety) {
            items.push(skip(finding, "filtered by --only level"));
            continue;
        }
        if finding.safety == crate::model::Safety::Danger && !opts.force_danger {
            items.push(skip(finding, "danger items need --force"));
            continue;
        }
        match &finding.action {
            CleanAction::RemovePath { path } => {
                if let Some(reason) = refusal_reason(path) {
                    items.push(skip(finding, &reason));
                    continue;
                }
                if let Err(reason) = validate_ancestors(path) {
                    items.push(skip(finding, &reason));
                    continue;
                }
                match std::fs::symlink_metadata(path) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        items.push(skip(finding, "path already gone"));
                        continue;
                    }
                    Err(e) => {
                        items.push(skip(finding, &format!("cannot stat path: {e}")));
                        continue;
                    }
                    Ok(meta) if meta.file_type().is_symlink() => {
                        items.push(skip(finding, "refused: path is a symbolic link"));
                        continue;
                    }
                    Ok(_) => {}
                }
                let path = match std::fs::canonicalize(path) {
                    Ok(path) => path,
                    Err(e) => {
                        items.push(skip(finding, &format!("cannot resolve path: {e}")));
                        continue;
                    }
                };
                // A parent containing an item excluded from this run cannot
                // be removed: it would wipe the excluded child. Skip first.
                if withheld
                    .iter()
                    .any(|kept| kept != &path && kept.starts_with(&path))
                {
                    items.push(skip(
                        finding,
                        "refused: contains an item excluded from this run",
                    ));
                    continue;
                }
                // Nested findings are covered by the broader delete: removing
                // the parent removes the children, so plan the parent once
                // and skip the descendants (bytes counted once, one trash
                // operation instead of hundreds). Only ancestors that are
                // themselves eligible in this run count — see `eligible`.
                if eligible
                    .iter()
                    .any(|other| path != *other && path.starts_with(other))
                {
                    items.push(skip(finding, "refused: covered by a broader finding"));
                    continue;
                }
                if findings.iter().zip(&resolved).any(|(other, resolved)| {
                    resolved.as_ref() == Some(&path) && other.safety > finding.safety
                }) {
                    items.push(skip(
                        finding,
                        "same path has a stricter safety classification",
                    ));
                    continue;
                }
                if !paths.insert(path.clone()) {
                    items.push(skip(finding, "duplicate path"));
                    continue;
                }
                let mut finding = finding.clone();
                finding.action = CleanAction::RemovePath { path };
                total_bytes += finding.bytes;
                items.push(PlannedItem {
                    finding,
                    disposition: Disposition::Remove {
                        via: if opts.to_trash {
                            "trash".to_string()
                        } else {
                            "permanent".to_string()
                        },
                    },
                });
            }
            CleanAction::RunCommand { description, .. } => {
                items.push(skip(
                    finding,
                    &format!("needs external command (v1 reports only): {description}"),
                ));
            }
            CleanAction::Manual { instructions } => {
                items.push(skip(
                    finding,
                    &format!("manual steps required: {instructions}"),
                ));
            }
        }
    }
    CleanPlan { items, total_bytes }
}

/// Structural refusal independent of any detector: roots, empties, `..`.
fn refusal_reason(path: &Path) -> Option<String> {
    if path.as_os_str().is_empty() {
        return Some("refused: empty path".to_string());
    }
    let mut normal_components = 0;
    for component in path.components() {
        match component {
            Component::ParentDir => return Some("refused: `..` in path".to_string()),
            Component::RootDir | Component::Prefix(_) => {}
            Component::Normal(_) => normal_components += 1,
            Component::CurDir => {}
        }
    }
    if normal_components == 0 {
        return Some("refused: filesystem root".to_string());
    }
    None
}

/// Check every existing component before resolving it; canonicalizing first
/// would hide a junction or symlink in a parent directory.
fn validate_ancestors(path: &Path) -> Result<(), String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    for ancestor in absolute.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) if crate::scanner::metadata_is_link_or_reparse(&meta) => {
                return Err(format!(
                    "refused: symbolic link or reparse point: {}",
                    ancestor.display()
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot inspect {}: {e}", ancestor.display())),
        }
    }
    Ok(())
}

fn skip(finding: &Finding, reason: &str) -> PlannedItem {
    PlannedItem {
        finding: finding.clone(),
        disposition: Disposition::Skipped {
            reason: reason.to_string(),
        },
    }
}

/// Execute a plan. Dry-run (`execute: false`) returns the receipt without
/// touching anything. Real runs continue past single-item errors and
/// report them in `receipt.errors`.
///
/// The delete method comes from the plan's recorded `via`, never from a
/// second options struct, so a plan can't silently change meaning.
pub fn execute(plan: &CleanPlan, opts: &CleanOptions) -> CleanReceipt {
    execute_with_progress(plan, opts, None)
}

/// Progress event for one processed plan item (the human CLI prints these
/// live; agents ignore them and read the final receipt).
#[derive(Debug, Clone)]
pub struct CleanProgress {
    pub done: usize,
    pub total: usize,
    pub bytes: u64,
    pub label: String,
    pub ok: bool,
}

/// A validated deletion collected in phase 1, ready for phase 2.
struct Job {
    path: PathBuf,
    is_dir: bool,
    via: String,
    bytes: u64,
    label: String,
    safety: crate::model::Safety,
}

/// Execute with optional per-item progress notifications.
///
/// Phase 1 validates every plan item cheaply (safety gates, ancestor
/// links, vanished paths, detector rechecks) and collects deletion jobs.
/// Phase 2 performs the destructive calls. Permanent plans run jobs in
/// parallel via rayon and each directory job uses a parallel one-pass
/// deleter ([`delete_tree`]) that counts bytes while unlinking — raw
/// `std::fs` primitives, no shell file-operation APIs, no separate sizing
/// walk. Trash plans stay sequential because the platform trash is a
/// shell service (COM `IFileOperation` on Windows). Receipts keep plan
/// order.
pub fn execute_with_progress(
    plan: &CleanPlan,
    opts: &CleanOptions,
    progress: Option<std::sync::mpsc::SyncSender<CleanProgress>>,
) -> CleanReceipt {
    let mut receipt = CleanReceipt {
        schema_version: SCHEMA_VERSION,
        dry_run: !opts.execute,
        freed_bytes: 0,
        removed: Vec::new(),
        skipped: Vec::new(),
        errors: Vec::new(),
    };
    let mut jobs: Vec<Job> = Vec::new();
    for item in &plan.items {
        let Disposition::Remove { via } = &item.disposition else {
            if let Disposition::Skipped { reason } = &item.disposition {
                receipt.skipped.push(SkippedItem {
                    label: item.finding.label.clone(),
                    safety: item.finding.safety,
                    reason: reason.clone(),
                });
            }
            continue;
        };
        let Some(path) = action_path(&item.finding) else {
            receipt.errors.push(format!(
                "{}: plan/action mismatch (no path to remove)",
                item.finding.label
            ));
            continue;
        };
        if !matches!(via.as_str(), "trash" | "permanent") {
            receipt.errors.push(format!("unknown delete method: {via}"));
            continue;
        }
        if !opts.include.allows(item.finding.safety)
            || (item.finding.safety == crate::model::Safety::Danger && !opts.force_danger)
        {
            receipt.errors.push(format!(
                "{}: execution options exclude this safety level",
                item.finding.label
            ));
            continue;
        }
        if let Some(reason) = refusal_reason(&path) {
            receipt.errors.push(reason);
            continue;
        }
        if let Err(reason) = validate_ancestors(&path) {
            receipt.errors.push(reason);
            continue;
        }
        if !opts.execute {
            receipt.freed_bytes += item.finding.bytes;
            receipt.removed.push(RemovedItem {
                path,
                bytes: item.finding.bytes,
                via: via.clone(),
                safety: item.finding.safety,
            });
            continue;
        }
        if item.finding.detector_id == "pycache"
            && path.exists()
            && crate::detectors::pycache::backed_bytes(&path).is_none()
        {
            receipt.errors.push(format!(
                "{}: bytecode contents or sources changed after planning",
                path.display()
            ));
            continue;
        }
        if item.finding.detector_id == "temp"
            && path.exists()
            && !crate::detectors::temp::is_stale(&path)
        {
            receipt.errors.push(format!(
                "{}: temp contents changed or could not be verified",
                path.display()
            ));
            continue;
        }
        if item.finding.detector_id == "agent-artifacts"
            && path.exists()
            && !crate::detectors::agent::is_quiet(&path)
        {
            receipt.errors.push(format!(
                "{}: agent artifact changed recently; a task may be running",
                path.display()
            ));
            continue;
        }
        // Re-validate type at delete time (a swapped symlink must never be
        // deleted through). Sizing is deliberately NOT done here: it runs
        // inside the phase-2 job so it overlaps with other jobs' deletes
        // instead of adding a sequential walk per item.
        let is_dir = match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                receipt.skipped.push(SkippedItem {
                    label: item.finding.label.clone(),
                    safety: item.finding.safety,
                    reason: "vanished between plan and execute".to_string(),
                });
                continue;
            }
            Err(e) => {
                receipt.errors.push(format!(
                    "{} ({}): re-stat failed: {e}",
                    item.finding.label,
                    path.display()
                ));
                continue;
            }
            Ok(meta) if meta.file_type().is_symlink() => {
                receipt.errors.push(format!(
                    "{} ({}): refused: became a symbolic link after planning",
                    item.finding.label,
                    path.display()
                ));
                continue;
            }
            Ok(meta) => meta.is_dir(),
        };
        jobs.push(Job {
            path,
            is_dir,
            via: via.clone(),
            bytes: item.finding.bytes,
            label: item.finding.label.clone(),
            safety: item.finding.safety,
        });
    }
    // Phase 2: destructive calls. Each job re-stats its tree right before
    // deleting (drift guard + honest receipt) and permanent plans run jobs
    // in parallel: independent tree roots delete concurrently, so the
    // re-stat walk overlaps with other jobs' deletes instead of adding a
    // full sequential pre-pass (the v0.1.0 behaviour that made multi-GB
    // cleanups crawl: walk everything, then delete everything, one item at
    // a time). Trash plans stay sequential — the platform trash is a shell
    // service (COM IFileOperation on Windows).
    if opts.execute && !jobs.is_empty() {
        let total = jobs.len();
        let done = std::sync::atomic::AtomicUsize::new(0);
        let run = |job: &Job| -> (Result<RemovedItem, String>, Vec<String>) {
            let mut warnings: Vec<String> = Vec::new();
            let outcome: Result<u64, String> = if job.via == "trash" {
                // Trash keeps a pre-walk: the OS moves the tree (no per-file
                // callback), so the walk is the only way to size the receipt.
                let bytes_now = if job.is_dir {
                    crate::scanner::size_of_dir(&job.path).0
                } else {
                    std::fs::symlink_metadata(&job.path)
                        .map(|m| m.len())
                        .unwrap_or(job.bytes)
                };
                delete_path(&job.path, job.is_dir, true).map(|()| bytes_now)
            } else if job.is_dir {
                // Parallel one-pass delete: bytes counted while unlinking.
                let (bytes, mut errs) = delete_tree(&job.path);
                if job.path.exists() {
                    let detail = if errs.is_empty() {
                        "tree still exists after deletion".to_string()
                    } else {
                        errs.join("; ")
                    };
                    Err(detail)
                } else {
                    // Locked children are reported, but the item is done.
                    warnings.append(&mut errs);
                    Ok(bytes)
                }
            } else {
                let len = std::fs::symlink_metadata(&job.path)
                    .map(|m| m.len())
                    .unwrap_or(job.bytes);
                delete_path(&job.path, false, false).map(|()| len)
            };
            let ok = outcome.is_ok();
            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if let Some(tx) = &progress {
                let _ = tx.try_send(CleanProgress {
                    done: n,
                    total,
                    bytes: outcome.as_ref().copied().unwrap_or(0),
                    label: job.label.clone(),
                    ok,
                });
            }
            match outcome {
                Ok(bytes_now) => (
                    Ok(RemovedItem {
                        path: job.path.clone(),
                        bytes: bytes_now,
                        via: job.via.clone(),
                        safety: job.safety,
                    }),
                    warnings,
                ),
                Err(e) => (
                    Err(format!("{} ({}): {e}", job.label, job.path.display())),
                    warnings,
                ),
            }
        };
        if jobs.iter().any(|j| j.via == "trash") {
            for job in &jobs {
                let (result, mut warnings) = run(job);
                receipt.errors.append(&mut warnings);
                match result {
                    Ok(removed) => {
                        receipt.freed_bytes += removed.bytes;
                        receipt.removed.push(removed);
                    }
                    Err(e) => receipt.errors.push(e),
                }
            }
        } else {
            use rayon::prelude::*;
            let results: Vec<(Result<RemovedItem, String>, Vec<String>)> =
                jobs.par_iter().map(run).collect();
            for (result, mut warnings) in results {
                receipt.errors.append(&mut warnings);
                match result {
                    Ok(removed) => {
                        receipt.freed_bytes += removed.bytes;
                        receipt.removed.push(removed);
                    }
                    Err(e) => receipt.errors.push(e),
                }
            }
        }
    }
    receipt
}
fn delete_path(path: &Path, is_dir: bool, to_trash: bool) -> Result<(), String> {
    if to_trash {
        return trash::delete(path).map_err(|e| e.to_string());
    }
    if is_dir {
        // Handles Windows long paths and read-only trees.
        return remove_dir_all::remove_dir_all(path).map_err(|e| e.to_string());
    }
    // remove_file fails on read-only files (git objects, caches);
    // clear the flag first — the trash path above doesn't need this.
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        let mut perms = meta.permissions();
        make_writable(&mut perms);
        let _ = std::fs::set_permissions(path, perms);
    }
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

/// Delete one directory tree in a single parallel pass, returning the
/// bytes of files actually removed plus non-fatal errors (locked files
/// leave their siblings deleted).
///
/// Why this shape (see docs/SAFETY.md):
/// - Raw `std::fs` primitives only. Benchmarks of mass deletion on
///   Windows rank plain unlink loops (what `std::filesystem`/`del /f/s/q`
///   do) well ahead of shell APIs: `IFileOperation` ~2x slower and
///   `SHFileOperation` >7x slower for many small files.
/// - Subdirectories (and wide file batches) are processed in parallel via
///   rayon — the same trick that makes `robocopy /MT` the fastest mass
///   file tool on Windows. With NVMe the bottleneck is per-unlink
///   latency, not bandwidth, so concurrent subtrees scale.
/// - One pass total: bytes are accumulated while deleting, so there is no
///   separate sizing walk before the delete.
/// - Links and reparse points are never followed: the link itself is
///   removed, its target is untouched.
/// - Read-only attributes are cleared before unlinking (same behaviour
///   the old remove_dir_all path provided).
fn delete_tree(root: &Path) -> (u64, Vec<String>) {
    use rayon::prelude::*;
    let mut bytes = 0u64;
    let mut errors = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => return (0, vec![format!("cannot list {}: {e}", root.display())]),
    };
    let mut files: Vec<(PathBuf, u64)> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                errors.push(format!("{}: {e}", root.display()));
                continue;
            }
        };
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) => {
                errors.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        if meta.file_type().is_symlink() || crate::scanner::metadata_is_link_or_reparse(&meta) {
            // Remove the link itself; never descend into the target.
            let result = if meta.is_dir() {
                std::fs::remove_dir(&path)
            } else {
                std::fs::remove_file(&path)
            };
            if let Err(e) = result {
                errors.push(format!("{}: {e}", path.display()));
            }
        } else if meta.is_dir() {
            subdirs.push(path);
        } else if meta.is_file() {
            files.push((path, meta.len()));
        } else if let Err(e) = std::fs::remove_file(&path) {
            // Unknown types (sockets, devices): best-effort unlink.
            errors.push(format!("{}: {e}", path.display()));
        }
    }
    // Wide leaf dirs unlink in parallel; narrow ones stay serial to avoid
    // task overhead. Threshold chosen from cache-dir shapes (hundreds of
    // files per leaf on average).
    let file_results: Vec<Result<u64, String>> = if files.len() >= 32 {
        files
            .par_iter()
            .map(|(p, len)| unlink_file(p, *len))
            .collect()
    } else {
        files.iter().map(|(p, len)| unlink_file(p, *len)).collect()
    };
    for result in file_results {
        match result {
            Ok(len) => bytes += len,
            Err(e) => errors.push(e),
        }
    }
    let sub_results: Vec<(u64, Vec<String>)> =
        subdirs.par_iter().map(|dir| delete_tree(dir)).collect();
    for (sub_bytes, mut sub_errors) in sub_results {
        bytes += sub_bytes;
        errors.append(&mut sub_errors);
    }
    if let Ok(meta) = std::fs::symlink_metadata(root) {
        let mut perms = meta.permissions();
        if perms.readonly() {
            make_writable(&mut perms);
            let _ = std::fs::set_permissions(root, perms);
        }
    }
    if let Err(e) = std::fs::remove_dir(root) {
        errors.push(format!("cannot remove {}: {e}", root.display()));
    }
    (bytes, errors)
}

/// Unlink one file, clearing read-only first. Returns bytes on success.
fn unlink_file(path: &Path, len: u64) -> Result<u64, String> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        let mut perms = meta.permissions();
        if perms.readonly() {
            make_writable(&mut perms);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    std::fs::remove_file(path)
        .map(|()| len)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Add owner-write without clobbering other bits. (`set_readonly(false)`
/// trips `clippy::permission_set_readonly_false` and is Windows-only in
/// spirit; this is portable and lint-clean on MSRV 1.74.)
#[cfg(windows)]
fn make_writable(perms: &mut std::fs::Permissions) {
    perms.set_readonly(false);
}

#[cfg(not(windows))]
fn make_writable(perms: &mut std::fs::Permissions) {
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(perms.mode() | 0o200);
}

fn action_path(finding: &Finding) -> Option<PathBuf> {
    match &finding.action {
        CleanAction::RemovePath { path } => Some(path.clone()),
        _ => None,
    }
}

/// Convenience: plan + execute in one call.
pub fn clean(findings: &[Finding], opts: &CleanOptions) -> CleanReceipt {
    let planned = plan(findings, opts);
    execute(&planned, opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{IncludeLevel, Safety};
    use std::fs;

    fn finding(label: &str, path: &Path, bytes: u64, safety: Safety) -> Finding {
        Finding {
            detector_id: "test".to_string(),
            label: label.to_string(),
            bytes,
            safety,
            detail: String::new(),
            action: CleanAction::RemovePath {
                path: path.to_path_buf(),
            },
        }
    }

    fn opts() -> CleanOptions {
        CleanOptions {
            execute: true,
            to_trash: false,
            include: IncludeLevel::All,
            force_danger: true,
        }
    }

    #[test]
    fn dry_run_touches_nothing_but_totals() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("junk");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("f"), vec![0u8; 50]).unwrap();
        let findings = vec![finding("junk", &target, 50, Safety::Safe)];
        let dry = CleanOptions {
            execute: false,
            ..opts()
        };
        let receipt = clean(&findings, &dry);
        assert!(receipt.dry_run);
        assert_eq!(receipt.freed_bytes, 50);
        assert!(target.exists(), "dry run must not delete");
    }

    #[test]
    fn execute_permanent_deletes_and_counts_true_dir_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("junk");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("f"), vec![0u8; 50]).unwrap();
        // Lie in the finding: receipt must report real bytes (50), not 7.
        let findings = vec![finding("junk", &target, 7, Safety::Safe)];
        let receipt = clean(&findings, &opts());
        assert!(!receipt.dry_run);
        assert!(!target.exists());
        assert_eq!(receipt.removed.len(), 1);
        assert_eq!(receipt.removed[0].via, "permanent");
        assert_eq!(receipt.freed_bytes, 50);
        assert!(receipt.errors.is_empty());
    }

    #[test]
    fn parallel_delete_removes_nested_tree_and_counts_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("tree");
        fs::create_dir_all(root.join("a").join("b")).unwrap();
        fs::create_dir_all(root.join("c")).unwrap();
        fs::write(root.join("f1"), vec![0u8; 10]).unwrap();
        fs::write(root.join("a").join("f2"), vec![0u8; 20]).unwrap();
        fs::write(root.join("a").join("b").join("f3"), vec![0u8; 30]).unwrap();
        fs::write(root.join("c").join("f4"), vec![0u8; 40]).unwrap();
        let findings = vec![finding("tree", &root, 0, Safety::Safe)];
        let receipt = clean(&findings, &opts());
        assert!(!root.exists());
        assert!(receipt.errors.is_empty(), "{:?}", receipt.errors);
        assert_eq!(receipt.freed_bytes, 100);
        assert_eq!(receipt.removed.len(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn parallel_delete_continues_past_locked_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("tree");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("locked.bin"), vec![0u8; 8]).unwrap();
        fs::write(root.join("sub").join("free.bin"), vec![0u8; 16]).unwrap();
        let handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(root.join("locked.bin"))
            .unwrap();
        let findings = vec![finding("tree", &root, 0, Safety::Safe)];
        let receipt = clean(&findings, &opts());
        // Free sibling deleted; the locked file blocks only its own entry.
        assert!(!root.join("sub").exists());
        assert!(root.join("locked.bin").exists());
        assert!(!receipt.errors.is_empty());
        assert!(receipt.removed.is_empty());
        drop(handle);
    }

    #[test]
    fn execute_deletes_readonly_files_permanently() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("ro.bin");
        fs::write(&target, vec![0u8; 32]).unwrap();
        let mut perms = fs::metadata(&target).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&target, perms).unwrap();
        let findings = vec![finding("ro", &target, 32, Safety::Safe)];
        let receipt = clean(&findings, &opts());
        assert!(!target.exists());
        assert!(receipt.errors.is_empty());
    }

    #[test]
    fn danger_needs_force_and_level_all() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("db");
        fs::create_dir_all(&target).unwrap();
        let findings = vec![finding("db", &target, 10, Safety::Danger)];

        let no_force = CleanOptions {
            include: IncludeLevel::All,
            force_danger: false,
            ..opts()
        };
        let r = clean(&findings, &no_force);
        assert!(target.exists());
        assert!(r.removed.is_empty());

        let safe_only = CleanOptions {
            include: IncludeLevel::Safe,
            force_danger: true,
            ..opts()
        };
        let r = clean(&findings, &safe_only);
        assert!(target.exists());
        assert!(r.removed.is_empty());

        let r = clean(&findings, &opts());
        assert!(!target.exists());
        assert_eq!(r.removed.len(), 1);
    }

    #[test]
    fn caution_excluded_by_default_level() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("mods");
        fs::create_dir_all(&target).unwrap();
        let findings = vec![finding("mods", &target, 10, Safety::Caution)];
        let safe_only = CleanOptions {
            include: IncludeLevel::Safe,
            ..opts()
        };
        let r = clean(&findings, &safe_only);
        assert!(target.exists());
        assert_eq!(r.skipped.len(), 1);
    }

    #[test]
    fn guardrails_refuse_roots_empties_and_dotdot() {
        let roots: Vec<PathBuf> = if cfg!(windows) {
            vec![PathBuf::from(r"C:\")]
        } else {
            vec![PathBuf::from("/")]
        };
        for root in roots
            .into_iter()
            .chain([PathBuf::new(), PathBuf::from("/tmp/../x")])
        {
            let findings = vec![finding("evil", &root, 999, Safety::Safe)];
            let r = clean(&findings, &opts());
            assert!(r.removed.is_empty(), "must not plan {}", root.display());
            assert_eq!(r.skipped.len(), 1);
        }
    }

    #[test]
    fn symlinks_are_never_deleted_through() {
        let tmp = tempfile::tempdir().unwrap();
        let victim = tmp.path().join("victim");
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep"), vec![0u8; 16]).unwrap();
        let link = tmp.path().join("link");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&victim, &link).is_ok();
        #[cfg(not(windows))]
        let made = std::os::unix::fs::symlink(&victim, &link).is_ok();
        if !made {
            // No privilege to create links (stock Windows): nothing to test.
            return;
        }
        let findings = vec![finding("link", &link, 16, Safety::Safe)];
        let r = clean(&findings, &opts());
        assert!(r.removed.is_empty());
        assert!(victim.join("keep").exists(), "victim must survive");
    }

    #[test]
    fn manual_and_command_findings_are_reported_never_run() {
        let findings = vec![
            Finding {
                detector_id: "docker".to_string(),
                label: "disk".to_string(),
                bytes: 100,
                safety: Safety::Caution,
                detail: String::new(),
                action: CleanAction::Manual {
                    instructions: "do it".to_string(),
                },
            },
            Finding {
                detector_id: "x".to_string(),
                label: "cmd".to_string(),
                bytes: 100,
                safety: Safety::Safe,
                detail: String::new(),
                action: CleanAction::RunCommand {
                    program: "rm".to_string(),
                    args: vec!["-rf".to_string(), "/".to_string()],
                    description: "evil".to_string(),
                },
            },
        ];
        let r = clean(&findings, &opts());
        assert!(r.removed.is_empty());
        assert_eq!(r.skipped.len(), 2);
    }
}
