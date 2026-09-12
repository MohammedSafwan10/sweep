//! Cargo: registry src/cache (re-downloadable) + per-project `target/`.

use super::{dir_finding, find_projects, Ctx, Detector};
use crate::model::{Finding, Safety};

pub struct CargoDetector;

impl Detector for CargoDetector {
    fn id(&self) -> &'static str {
        "cargo"
    }
    fn label(&self) -> &'static str {
        "Cargo registry + build output"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        let registry = ctx.cargo_home.join("registry");
        for (name, detail) in [
            (
                "src",
                "Extracted crate sources; re-downloaded on next build.",
            ),
            (
                "cache",
                "Downloaded .crate files; re-downloaded on next build.",
            ),
        ] {
            if let Some(f) = dir_finding(
                self.id(),
                format!("cargo registry/{name}"),
                &registry.join(name),
                Safety::Safe,
                detail,
            ) {
                out.push(f);
            }
        }
        for project in find_projects(&ctx.project_roots, "Cargo.toml") {
            let target = project.join("target");
            if let Some(f) = dir_finding(
                self.id(),
                format!("cargo target/ ({})", project_name(&project)),
                &target,
                Safety::Safe,
                "cargo build output; `cargo clean` equivalent. Rebuilt on demand.",
            ) {
                out.push(f);
            }
        }
        out
    }
}

fn project_name(project: &std::path::Path) -> String {
    project
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use crate::model::Safety;
    use std::fs;

    #[test]
    fn finds_registry_src_and_target() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        fs::create_dir_all(ctx.cargo_home.join("registry").join("src").join("x")).unwrap();
        fs::write(
            ctx.cargo_home
                .join("registry")
                .join("src")
                .join("x")
                .join("lib.rs"),
            vec![0u8; 100],
        )
        .unwrap();
        let proj = ctx.project_roots[0].join("app");
        fs::create_dir_all(proj.join("target").join("debug")).unwrap();
        fs::write(proj.join("Cargo.toml"), "[package]").unwrap();
        fs::write(
            proj.join("target").join("debug").join("app"),
            vec![0u8; 200],
        )
        .unwrap();

        let findings = super::CargoDetector.scan(&ctx);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|f| f.safety == Safety::Safe));
        let total: u64 = findings.iter().map(|f| f.bytes).sum();
        assert_eq!(total, 300);
    }
}
