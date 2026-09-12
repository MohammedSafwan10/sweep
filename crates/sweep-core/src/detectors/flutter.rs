//! Flutter projects: `build/` + `.dart_tool/` next to `pubspec.yaml`.
//! (`flutter clean` equivalent; restored by the next build / `pub get`.)

use super::{dir_finding, find_projects, Ctx, Detector};
use crate::model::{Finding, Safety};

pub struct FlutterDetector;

impl Detector for FlutterDetector {
    fn id(&self) -> &'static str {
        "flutter-build"
    }
    fn label(&self) -> &'static str {
        "Flutter build output"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        for project in find_projects(&ctx.project_roots, "pubspec.yaml") {
            let name = short_name(&project);
            for (dir, what) in [
                ("build", "Compiled app output; `flutter clean` equivalent."),
                (
                    ".dart_tool",
                    "Package config; restored by `flutter pub get`.",
                ),
            ] {
                if let Some(f) = dir_finding(
                    self.id(),
                    format!("flutter {dir}/ ({name})"),
                    &project.join(dir),
                    Safety::Safe,
                    what,
                ) {
                    out.push(f);
                }
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
    fn finds_build_and_dart_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let proj = ctx.project_roots[0].join("myapp");
        fs::create_dir_all(proj.join("build")).unwrap();
        fs::create_dir_all(proj.join(".dart_tool")).unwrap();
        fs::write(proj.join("pubspec.yaml"), "name: myapp").unwrap();
        fs::write(proj.join("build").join("out"), vec![0u8; 100]).unwrap();
        fs::write(proj.join(".dart_tool").join("cfg"), vec![0u8; 25]).unwrap();

        let findings = super::FlutterDetector.scan(&ctx);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings.iter().map(|f| f.bytes).sum::<u64>(), 125);
    }
}
