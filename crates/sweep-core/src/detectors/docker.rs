//! Docker Desktop: the WSL data disk (`docker_data.vhdx`).
//!
//! Reports logical file size, not allocated or reclaimable bytes.
//! Docker owns this disk; cleanup is manual and never deletes the vhdx.

use super::{Ctx, Detector};
use crate::model::{CleanAction, Finding, Safety};

pub struct DockerDetector;

impl Detector for DockerDetector {
    fn id(&self) -> &'static str {
        "docker"
    }
    fn label(&self) -> &'static str {
        "Docker Desktop data disk"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        for path in &ctx.docker_data_files {
            let Ok(meta) = std::fs::symlink_metadata(path) else {
                continue;
            };
            if !meta.is_file()
                || meta.len() == 0
                || crate::scanner::metadata_is_link_or_reparse(&meta)
            {
                continue;
            }
            out.push(Finding {
                detector_id: self.id().to_string(),
                label: format!("docker data disk ({})", file_name(path)),
                bytes: meta.len(),
                safety: Safety::Caution,
                detail: "Docker VM disk logical size; includes live data and is not an estimate of reclaimable space.".into(),
                action: CleanAction::Manual {
                    instructions: "Review usage with `docker system df` and Docker Desktop. Remove only resources you recognize as unused. Volumes may contain databases; back them up before removal. Do not delete the vhdx directly."
                        .to_string(),
                },
            });
        }
        out
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use std::fs;

    #[test]
    fn reports_vhdx_size() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let vhdx = &ctx.docker_data_files[0];
        fs::create_dir_all(vhdx.parent().unwrap()).unwrap();
        fs::write(vhdx, vec![0u8; 1234]).unwrap();
        let findings = super::DockerDetector.scan(&ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].bytes, 1234);
        assert!(matches!(
            findings[0].action,
            crate::model::CleanAction::Manual { .. }
        ));
    }
}
