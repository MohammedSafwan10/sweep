//! Cleaning planner + executor.
//!
//! Safety contract (see SAFETY.md):
//! - Nothing happens without `execute: true` (dry-run is the default).
//! - `Danger` findings always need `force_danger`, even with `All`.
//! - Default destination is the OS Recycle Bin / Trash (`to_trash`).
//! - v1 only deletes paths. `RunCommand`/`Manual` findings are reported
//!   with instructions, never executed — running foreign CLIs with
//!   destructive flags deserves its own audited milestone.

use crate::model::{
    CleanAction, CleanOptions, CleanReceipt, Disposition, Finding, PlannedItem, RemovedItem,
    SkippedItem, SCHEMA_VERSION,
};

pub struct CleanPlan {
    pub items: Vec<PlannedItem>,
    pub total_bytes: u64,
}

/// Build the plan: filter by safety level, drop vanished paths, total bytes.
pub fn plan(findings: &[Finding], opts: &CleanOptions) -> CleanPlan {
    let mut items = Vec::new();
    let mut total_bytes: u64 = 0;
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
                if !path.exists() {
                    items.push(skip(finding, "path already gone"));
                    continue;
                }
                total_bytes += finding.bytes;
                items.push(PlannedItem {
                    finding: finding.clone(),
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
                    reason: reason.clone(),
                });
            }
            continue;
        };
        if !opts.execute {
            receipt.freed_bytes += item.finding.bytes;
            receipt.removed.push(RemovedItem {
                path: action_path(&item.finding).unwrap_or_default(),
                bytes: item.finding.bytes,
                via: via.clone(),
            });
            continue;
        }
        let Some(path) = action_path(&item.finding) else {
            receipt
                .errors
                .push(format!("{}: no path to remove", item.finding.label));
            continue;
        };
        // Re-stat at delete time: sizes may have changed since the scan.
        let bytes_now = path
            .metadata()
            .map(|m| m.len())
            .unwrap_or(item.finding.bytes);
        let result = if opts.to_trash {
            trash::delete(&path).map_err(|e| e.to_string())
        } else if path.is_dir() {
            remove_dir_all::remove_dir_all(&path).map_err(|e| e.to_string())
        } else {
            std::fs::remove_file(&path).map_err(|e| e.to_string())
        };
        match result {
            Ok(()) => {
                receipt.freed_bytes += bytes_now;
                receipt.removed.push(RemovedItem {
                    path,
                    bytes: bytes_now,
                    via: via.clone(),
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

fn action_path(finding: &Finding) -> Option<std::path::PathBuf> {
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
    use std::path::Path;

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
    fn execute_permanent_deletes_and_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("junk");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("f"), vec![0u8; 50]).unwrap();
        let findings = vec![finding("junk", &target, 50, Safety::Safe)];
        let receipt = clean(&findings, &opts());
        assert!(!receipt.dry_run);
        assert!(!target.exists());
        assert_eq!(receipt.removed.len(), 1);
        assert_eq!(receipt.removed[0].via, "permanent");
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
