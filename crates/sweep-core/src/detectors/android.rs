//! Android SDK: NDK versions, build-tools, platforms, sources, cmake.
//!
//! All flagged Caution: removing an NDK/platform your project targets
//! breaks the build until reinstalled via SDK Manager. sweep never
//! guesses which one is "current" — the human/agent picks.

use super::{dir_finding, Ctx, Detector};
use crate::model::{Finding, Safety};

pub struct AndroidDetector;

impl Detector for AndroidDetector {
    fn id(&self) -> &'static str {
        "android-sdk"
    }
    fn label(&self) -> &'static str {
        "Android SDK components"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        let Some(sdk) = ctx.android_sdk.clone() else {
            return out;
        };
        // Per-version dirs: one finding each so the user picks.
        for (group, detail) in [
            ("ndk", "Native toolchain; ~2 GiB per version."),
            ("build-tools", "Per-version build tools."),
            ("platforms", "android-XX platform frameworks."),
            ("cmake", "Bundled CMake versions."),
            ("sources", "Sources for debugging (rarely needed)."),
        ] {
            let group_dir = sdk.join(group);
            let Ok(entries) = std::fs::read_dir(&group_dir) else {
                continue;
            };
            for entry in entries.filter_map(|e| e.ok()) {
                if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                if let Some(f) = dir_finding(
                    self.id(),
                    format!("android {group}/{}", entry.file_name().to_string_lossy()),
                    &entry.path(),
                    Safety::Caution,
                    detail,
                ) {
                    out.push(f);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use std::fs;

    #[test]
    fn flags_ndk_versions_as_caution() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let sdk = ctx.android_sdk.clone().unwrap();
        fs::create_dir_all(sdk.join("ndk").join("28.2.13676358")).unwrap();
        fs::write(
            sdk.join("ndk").join("28.2.13676358").join("tool"),
            vec![0u8; 42],
        )
        .unwrap();
        let findings = super::AndroidDetector.scan(&ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].safety, crate::model::Safety::Caution);
        assert_eq!(findings[0].bytes, 42);
    }

    #[test]
    fn no_sdk_no_findings() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = Ctx::for_tests(tmp.path());
        ctx.android_sdk = None;
        assert!(super::AndroidDetector.scan(&ctx).is_empty());
    }
}
