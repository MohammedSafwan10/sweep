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
                if resolved
                    .iter()
                    .flatten()
                    .any(|child| child != &path && child.starts_with(&path))
                {
                    items.push(skip(finding, "refused: overlaps a more specific finding"));
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
    let mut receipt = CleanReceipt {
        schema_version: SCHEMA_VERSION,
        dry_run: !opts.execute,
        freed_bytes: 0,
        removed: Vec::new(),
        skipped: Vec::new(),
        errors: Vec::new(),
    };
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
        // Re-validate at delete time: refuse links (swapped after planning)
        // and re-stat directories properly (dir metadata len is NOT the
        // subtree size — that undercounted receipts before).
        let (is_dir, bytes_now) = match std::fs::symlink_metadata(&path) {
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
            Ok(meta) => (
                meta.is_dir(),
                if meta.is_dir() {
                    crate::scanner::size_of_dir(&path).0
                } else {
                    meta.len()
                },
            ),
        };
        let result = delete_path(&path, is_dir, via == "trash");
        match result {
            Ok(()) => {
                receipt.freed_bytes += bytes_now;
                receipt.removed.push(RemovedItem {
                    path,
                    bytes: bytes_now,
                    via: via.clone(),
                    safety: item.finding.safety,
                });
            }
            Err(e) => {
                receipt
                    .errors
                    .push(format!("{} ({}): {e}", item.finding.label, path.display()))
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
        // Lie in the finding: receipt must report re-statted bytes (50), not 7.
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
