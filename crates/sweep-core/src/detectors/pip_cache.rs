//! pip cache: HTTP responses + locally built wheels.
//!
//! The cache location is resolved explicitly (platform default or
//! `PIP_CACHE_DIR` override) — the internal layout is never assumed.
//! Deleting the cache equals `pip cache purge`; pip re-downloads on demand.

use super::{dir_finding, Ctx, Detector};
use crate::model::{Finding, Safety};

pub struct PipCacheDetector;

impl Detector for PipCacheDetector {
    fn id(&self) -> &'static str {
        "pip-cache"
    }
    fn label(&self) -> &'static str {
        "pip package cache"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        if let Some(f) = dir_finding(
            self.id(),
            "pip cache".to_string(),
            &ctx.pip_cache,
            Safety::Safe,
            "HTTP/wheel cache; `pip cache purge` does the same. Re-downloaded on next install.",
        ) {
            out.push(f);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use std::fs;

    #[test]
    fn finds_pip_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        fs::create_dir_all(ctx.pip_cache.join("http-v2")).unwrap();
        fs::write(ctx.pip_cache.join("http-v2").join("a"), vec![0u8; 77]).unwrap();
        let findings = super::PipCacheDetector.scan(&ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].bytes, 77);
        assert_eq!(findings[0].safety, crate::model::Safety::Safe);
    }

    #[test]
    fn missing_cache_is_silent() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        assert!(super::PipCacheDetector.scan(&ctx).is_empty());
    }
}
