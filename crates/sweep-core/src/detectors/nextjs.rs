//! Next.js projects: `.next/` (safe) + `node_modules/` (caution).
//! Only directories whose `package.json` depends on `next` are flagged,
//! so plain Node projects are left for a future detector.

use super::{dir_finding, find_projects, Ctx, Detector};
use crate::model::{Finding, Safety};

pub struct NextJsDetector;

impl Detector for NextJsDetector {
    fn id(&self) -> &'static str {
        "nextjs-build"
    }
    fn label(&self) -> &'static str {
        "Next.js build output"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        for project in find_projects(&ctx.project_roots, "package.json") {
            if !depends_on_next(&project.join("package.json")) {
                continue;
            }
            let name = short_name(&project);
            if let Some(f) = dir_finding(
                self.id(),
                format!(".next/ ({name})"),
                &project.join(".next"),
                Safety::Safe,
                "Next.js build cache; rebuilt by `next build`.",
            ) {
                out.push(f);
            }
            if let Some(f) = dir_finding(
                self.id(),
                format!("node_modules/ ({name})"),
                &project.join("node_modules"),
                Safety::Caution,
                "Dependencies; `npm install` re-download can take minutes.",
            ) {
                out.push(f);
            }
        }
        out
    }
}

fn depends_on_next(manifest: &std::path::Path) -> bool {
    let Ok(text) = std::fs::read_to_string(manifest) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    for section in ["dependencies", "devDependencies", "peerDependencies"] {
        if json.get(section).and_then(|d| d.get("next")).is_some() {
            return true;
        }
    }
    false
}

fn short_name(project: &std::path::Path) -> String {
    project
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use std::fs;

    const PKG: &str = r#"{"dependencies": {"next": "15.0.0", "react": "19.0.0"}}"#;

    #[test]
    fn finds_next_and_modules() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let proj = ctx.project_roots[0].join("web");
        fs::create_dir_all(proj.join(".next")).unwrap();
        fs::create_dir_all(proj.join("node_modules").join("react")).unwrap();
        fs::write(proj.join("package.json"), PKG).unwrap();
        fs::write(proj.join(".next").join("a"), vec![0u8; 60]).unwrap();
        fs::write(
            proj.join("node_modules").join("react").join("b"),
            vec![0u8; 40],
        )
        .unwrap();

        let findings = super::NextJsDetector.scan(&ctx);
        assert_eq!(findings.len(), 2);
        let next = findings
            .iter()
            .find(|f| f.label.starts_with(".next"))
            .unwrap();
        assert_eq!((next.bytes, next.safety), (60, crate::model::Safety::Safe));
        let mods = findings
            .iter()
            .find(|f| f.label.starts_with("node_modules"))
            .unwrap();
        assert_eq!(
            (mods.bytes, mods.safety),
            (40, crate::model::Safety::Caution)
        );
    }

    #[test]
    fn ignores_non_next_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let proj = ctx.project_roots[0].join("plain");
        fs::create_dir_all(proj.join("node_modules")).unwrap();
        fs::write(proj.join("package.json"), r#"{"dependencies": {}}"#).unwrap();
        assert!(super::NextJsDetector.scan(&ctx).is_empty());
    }

    #[test]
    fn broken_manifest_is_skipped_safely() {
        assert!(!super::depends_on_next(std::path::Path::new(
            "/nope/package.json"
        )));
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("package.json");
        fs::write(&bad, "{not json").unwrap();
        assert!(!super::depends_on_next(&bad));
    }
}
