//! Vite projects: `node_modules/.vite` (pre-bundled deps cache).
//!
//! Only flagged when the project's `package.json` actually depends on
//! `vite` — never by bare directory name.

use super::{dir_finding, find_projects, Ctx, Detector};
use crate::model::{Finding, Safety};

pub struct ViteDetector;

impl Detector for ViteDetector {
    fn id(&self) -> &'static str {
        "vite-build"
    }
    fn label(&self) -> &'static str {
        "Vite dependency cache"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        for project in find_projects(&ctx.project_roots, "package.json") {
            if !super::package_has_dep(&project.join("package.json"), "vite") {
                continue;
            }
            if let Some(f) = dir_finding(
                self.id(),
                format!("node_modules/.vite/ ({})", short_name(&project)),
                &project.join("node_modules").join(".vite"),
                Safety::Safe,
                "Vite pre-bundled deps; rebuilt on next dev/build run.",
            ) {
                out.push(f);
            }
        }
        out
    }
}

fn short_name(project: &std::path::Path) -> String {
    project
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use std::fs;

    const PKG: &str = r#"{"devDependencies": {"vite": "6.0.0"}}"#;

    #[test]
    fn flags_vite_cache_in_vite_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let proj = ctx.project_roots[0].join("front");
        fs::create_dir_all(proj.join("node_modules").join(".vite")).unwrap();
        fs::write(proj.join("package.json"), PKG).unwrap();
        fs::write(
            proj.join("node_modules").join(".vite").join("deps.json"),
            vec![0u8; 33],
        )
        .unwrap();
        let findings = super::ViteDetector.scan(&ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].bytes, 33);
    }

    #[test]
    fn ignores_projects_without_vite() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let proj = ctx.project_roots[0].join("plain");
        fs::create_dir_all(proj.join("node_modules").join(".vite")).unwrap();
        fs::write(proj.join("package.json"), r#"{"dependencies": {}}"#).unwrap();
        assert!(super::ViteDetector.scan(&ctx).is_empty());
    }
}
