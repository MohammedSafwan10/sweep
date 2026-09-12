//! uv cache: report-only in v1.
//!
//! uv explicitly documents direct cache modification as unsafe, so sweep
//! reports the size and points at uv's own commands (`uv cache clean`,
//! `uv cache prune`). The finding is Manual and never auto-deleted.

use super::{Ctx, Detector};
use crate::model::{CleanAction, Finding, Safety};
use crate::scanner::size_of_dir;

pub struct UvCacheDetector;

impl Detector for UvCacheDetector {
    fn id(&self) -> &'static str {
        "uv-cache"
    }
    fn label(&self) -> &'static str {
        "uv package cache"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        if !ctx.uv_cache.is_dir() {
            return Vec::new();
        }
        let (bytes, _) = size_of_dir(&ctx.uv_cache);
        if bytes == 0 {
            return Vec::new();
        }
        vec![Finding {
            detector_id: self.id().to_string(),
            label: "uv cache".to_string(),
            bytes,
            safety: Safety::Caution,
            detail: "uv's cache. Direct modification is unsafe per uv docs; use uv itself."
                .to_string(),
            action: CleanAction::Manual {
                instructions:
                    "Run `uv cache prune` (safe refresh) or `uv cache clean` (full wipe)."
                        .to_string(),
            },
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use std::fs;

    #[test]
    fn reports_but_never_auto_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        fs::create_dir_all(&ctx.uv_cache).unwrap();
        fs::write(ctx.uv_cache.join("a"), vec![0u8; 55]).unwrap();
        let findings = super::UvCacheDetector.scan(&ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].bytes, 55);
        assert!(matches!(
            findings[0].action,
            crate::model::CleanAction::Manual { .. }
        ));
    }
}
