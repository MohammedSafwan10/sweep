//! Windows servicing leftovers: update downloads and superseded setup.
//!
//! Report-only in v1 (CAUTION + Manual): servicing state is owned by
//! Windows Update / DISM, and deleting under a running `wuauserv` or
//! without admin rights fails midway. Sweep reports the size with the
//! exact remediation instead of touching it.

use super::{Ctx, Detector};
use crate::model::{CleanAction, Finding, Safety};

pub struct WindowsUpdateDetector;

/// Ignore leftovers below this: an idle Windows keeps a few megabytes
/// of servicing metadata that is not worth prompting about.
const MIN_BYTES: u64 = 50 * 1024 * 1024;

impl Detector for WindowsUpdateDetector {
    fn id(&self) -> &'static str {
        "windows-update"
    }
    fn label(&self) -> &'static str {
        "Windows update leftovers"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        // Hermetic: the system root comes from Ctx (real machine resolves
        // SystemRoot; tests get an empty fixture so real Windows Update
        // leftovers can never leak into fixture-based assertions).
        let Some(windows) = &ctx.system_root else {
            return Vec::new();
        };
        scan_roots(windows, self.id())
    }
}

/// Pure scan over a given Windows dir (hermetic under test).
fn scan_roots(windows: &std::path::Path, detector_id: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    let windows_old = windows.parent().map(|drive| drive.join("Windows.old"));
    let mut targets: Vec<(&str, std::path::PathBuf, &str)> = vec![(
        "update downloads",
        windows.join("SoftwareDistribution").join("Download"),
        "Run Disk Cleanup (cleanmgr) as admin, or: `net stop wuauserv`, delete the contents of SoftwareDistribution\\Download, `net start wuauserv`.",
    )];
    if let Some(path) = windows_old {
        targets.push((
            "previous Windows install",
            path,
            "Use Settings > System > Storage > Temporary files > Previous Windows installations. Do not delete by hand while rollback is offered.",
        ));
    }
    for (name, dir, instructions) in targets {
        if let Some(bytes) = sized_bytes(&dir) {
            if bytes >= MIN_BYTES {
                out.push(Finding {
                    detector_id: detector_id.to_string(),
                    label: format!("windows {name}"),
                    bytes,
                    safety: Safety::Caution,
                    detail: "Servicing leftovers owned by Windows Update.".to_string(),
                    action: CleanAction::Manual {
                        instructions: instructions.to_string(),
                    },
                });
            }
        }
    }
    out
}

/// Best-effort size; None when the dir is absent or unreadable.
fn sized_bytes(path: &std::path::Path) -> Option<u64> {
    if !path.is_dir() {
        return None;
    }
    // Reuse the shared parallel sizer via a SAFE probe finding and take
    // its bytes (avoids duplicating walk logic for a report-only check).
    super::dir_finding("windows-update", "probe", path, Safety::Caution, "").map(|f| f.bytes)
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use std::fs;

    #[test]
    fn silent_without_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        // Ctx::for_tests points system_root at an empty fixture, so the
        // real machine's Windows Update downloads can never leak in.
        assert!(super::WindowsUpdateDetector.scan(&ctx).is_empty());
    }

    #[test]
    fn ctx_scan_finds_fixture_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let dl = ctx
            .system_root
            .as_ref()
            .unwrap()
            .join("SoftwareDistribution/Download");
        fs::create_dir_all(&dl).unwrap();
        fs::write(dl.join("big.cab"), vec![0u8; 60 * 1024 * 1024]).unwrap();
        let findings = super::WindowsUpdateDetector.scan(&ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].safety, crate::model::Safety::Caution);
        assert!(matches!(
            findings[0].action,
            crate::model::CleanAction::Manual { .. }
        ));
    }

    #[test]
    fn flags_big_downloads_and_old_install() {
        let tmp = tempfile::tempdir().unwrap();
        let windows = tmp.path().join("Windows");
        let dl = windows.join("SoftwareDistribution/Download");
        fs::create_dir_all(&dl).unwrap();
        fs::write(dl.join("big.cab"), vec![0u8; 60 * 1024 * 1024]).unwrap();
        let old = tmp.path().join("Windows.old");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("image.esd"), vec![0u8; 60 * 1024 * 1024]).unwrap();

        let findings = super::scan_roots(&windows, "windows-update");
        assert_eq!(findings.len(), 2);
        assert!(findings
            .iter()
            .all(|f| f.safety == crate::model::Safety::Caution));
        assert!(findings
            .iter()
            .all(|f| matches!(f.action, crate::model::CleanAction::Manual { .. })));
        let dl_finding = findings
            .iter()
            .find(|f| f.label.contains("downloads"))
            .unwrap();
        assert!(dl_finding.label.contains("windows update downloads"));
        if let crate::model::CleanAction::Manual { instructions } = &dl_finding.action {
            assert!(instructions.contains("wuauserv"));
        } else {
            panic!("expected manual instructions");
        }
    }

    #[test]
    fn tiny_leftovers_stay_silent() {
        let tmp = tempfile::tempdir().unwrap();
        let windows = tmp.path().join("Windows");
        let dl = windows.join("SoftwareDistribution/Download");
        fs::create_dir_all(&dl).unwrap();
        fs::write(dl.join("meta.dat"), vec![0u8; 10]).unwrap();
        assert!(super::scan_roots(&windows, "windows-update").is_empty());
    }
}
