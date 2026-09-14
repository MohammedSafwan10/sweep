//! Go caches: build cache (fast to rebuild) and module cache.
//!
//! Resolved from `GOCACHE` / `GOMODCACHE` / `GOPATH` with platform
//! defaults — v1 never runs `go` itself (see SAFETY.md: no external
//! commands), so a nonstandard `go env` layout falls back to the
//! documented defaults. `go clean -cache` restores the build cache
//! (SAFE); `go clean -modcache` re-downloads modules (CAUTION).

use super::{dir_finding, Ctx, Detector};
use crate::model::{Finding, Safety};

pub struct GoCacheDetector;

impl Detector for GoCacheDetector {
    fn id(&self) -> &'static str {
        "go-cache"
    }
    fn label(&self) -> &'static str {
        "Go build and module caches"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        if let Some(f) = dir_finding(
            self.id(),
            "go build cache".to_string(),
            &ctx.go_build_cache,
            Safety::Safe,
            "Rebuilt on next `go build`; `go clean -cache` equivalent.",
        ) {
            out.push(f);
        }
        if let Some(f) = dir_finding(
            self.id(),
            "go module cache".to_string(),
            &ctx.go_mod_cache,
            Safety::Caution,
            "Downloaded modules; `go clean -modcache` re-downloads them.",
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
    fn finds_build_and_module_caches() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        fs::create_dir_all(&ctx.go_build_cache).unwrap();
        fs::write(ctx.go_build_cache.join("a"), vec![0u8; 10]).unwrap();
        fs::create_dir_all(&ctx.go_mod_cache).unwrap();
        fs::write(ctx.go_mod_cache.join("b"), vec![0u8; 20]).unwrap();

        let findings = super::GoCacheDetector.scan(&ctx);
        assert_eq!(findings.len(), 2);
        let build = findings.iter().find(|f| f.label.contains("build")).unwrap();
        assert_eq!(build.safety, crate::model::Safety::Safe);
        let modules = findings
            .iter()
            .find(|f| f.label.contains("module"))
            .unwrap();
        assert_eq!(modules.safety, crate::model::Safety::Caution);
    }
}
