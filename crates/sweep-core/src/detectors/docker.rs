//! Docker Desktop: the WSL data disk (`docker_data.vhdx`).
//!
//! The vhdx is a sparse file — its on-disk size IS the space Docker owns.
//! sweep reports it as Caution + Manual: the file must never be deleted
//! by hand while Desktop runs. The supported path is Desktop prune
//! commands (volumes can hold database data — see SAFETY.md), followed
//! by a Desktop restart to compact the disk.

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
            let Ok(meta) = std::fs::metadata(path) else {
                continue;
            };
            if !meta.is_file() || meta.len() == 0 {
                continue;
            }
            let mut detail = String::from(
                "Docker Desktop's VM disk. Free inside Docker first (`docker system prune -a --volumes`, `docker builder prune -a -f`), then restart Desktop to compact. Volumes may hold database data — confirm before pruning.",
            );
            if let Some(df) = best_effort_df() {
                detail.push_str("\n\n`docker system df` right now:\n");
                detail.push_str(&df);
            }
            out.push(Finding {
                detector_id: self.id().to_string(),
                label: format!("docker data disk ({})", file_name(path)),
                bytes: meta.len(),
                safety: Safety::Caution,
                detail,
                action: CleanAction::Manual {
                    instructions: "1) docker system prune -a --volumes -f  2) docker builder prune -a -f  3) restart Docker Desktop to compact the vhdx."
                        .to_string(),
                },
            });
        }
        out
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string()
}

/// Best-effort `docker system df`; silently `None` when the daemon is down.
fn best_effort_df() -> Option<String> {
    let out = std::process::Command::new("docker")
        .args(["system", "df"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if text.trim().is_empty() {
        None
    } else {
        Some(text.chars().take(800).collect())
    }
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
