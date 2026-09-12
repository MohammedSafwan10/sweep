//! Angular projects: `.angular/cache/` build cache.
//!
//! Only flagged when the project's `package.json` depends on
//! `@angular/cli` or `@angular/core` — never by bare directory name.

use super::{dir_finding, find_projects, Ctx, Detector};
use crate::model::{Finding, Safety};

pub struct AngularDetector;

impl Detector for AngularDetector {
    fn id(&self) -> &'static str {
        "angular-build"
    }
    fn label(&self) -> &'static str {
        "Angular build cache"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        for project in find_projects(&ctx.project_roots, "package.json") {
            let manifest = project.join("package.json");
            if !super::package_has_dep(&manifest, "@angular/cli")
                && !super::package_has_dep(&manifest, "@angular/core")
            {
                continue;
            }
            if let Some(f) = dir_finding(
                self.id(),
                format!(".angular/cache/ ({})", short_name(&project)),
                &project.join(".angular").join("cache"),
                Safety::Safe,
                "Angular build cache; rebuilt by `ng build`/`ng serve`.",
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

    #[test]
    fn flags_angular_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let proj = ctx.project_roots[0].join("ngapp");
        fs::create_dir_all(proj.join(".angular").join("cache")).unwrap();
        fs::write(
            proj.join("package.json"),
            r#"{"devDependencies": {"@angular/cli": "19.0.0"}}"#,
        )
        .unwrap();
        fs::write(proj.join(".angular").join("cache").join("a"), vec![0u8; 44]).unwrap();
        let findings = super::AngularDetector.scan(&ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].bytes, 44);
    }

    #[test]
    fn ignores_vite_only_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let proj = ctx.project_roots[0].join("front");
        fs::create_dir_all(proj.join(".angular").join("cache")).unwrap();
        fs::write(
            proj.join("package.json"),
            r#"{"devDependencies": {"vite": "6"}}"#,
        )
        .unwrap();
        assert!(super::AngularDetector.scan(&ctx).is_empty());
    }
}
