//! uv cache: report-only in v1.
//!
//! uv explicitly documents direct cache modification as unsafe, so sweep
//! reports the size and points at uv's own commands (`uv cache clean`,
//! `uv cache prune`). The finding is Manual and never auto-deleted.

use super::{dir_finding, Ctx, Detector};
use crate::model::{CleanAction, Finding, Safety};
use crate::scanner::size_of_dir;

pub struct UvCacheDetector;

/// Ignore default-location leftovers below this: a fresh redirect leaves
/// small metadata behind that is not worth prompting about.
const ORPHAN_MIN_BYTES: u64 = 50 * 1024 * 1024;

impl Detector for UvCacheDetector {
    fn id(&self) -> &'static str {
        "uv-cache"
    }
    fn label(&self) -> &'static str {
        "uv package cache"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        if ctx.uv_cache.is_dir() {
            let (bytes, _) = size_of_dir(&ctx.uv_cache);
            if bytes > 0 {
                out.push(Finding {
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
                });
            }
        }
        // Orphaned default location: when UV_CACHE_DIR redirects elsewhere,
        // the old default accumulates stale gigabytes no uv command will
        // ever prune (measured: 10.8 GiB orphan vs 1.1 GiB live cache).
        // The redirect target is authoritative, so the leftover is CAUTION.
        let redirected = std::env::var_os("UV_CACHE_DIR").is_some_and(|v| !v.is_empty());
        if redirected {
            if let Some(defunct) = orphan_default(&ctx.uv_cache, &ctx.local_app_data) {
                if let Some(f) = dir_finding(
                    self.id(),
                    "stale default uv cache".to_string(),
                    &defunct,
                    Safety::Caution,
                    "UV_CACHE_DIR points elsewhere, so uv never prunes this default-location leftover.",
                ) {
                    if f.bytes >= ORPHAN_MIN_BYTES {
                        out.push(f);
                    }
                }
            }
        }
        out
    }
}

/// Default-location cache dir when it is NOT the live one (env redirect).
/// Pure in its inputs (helper is unit-testable); existence is checked by
/// the caller via `dir_finding`.
fn orphan_default(
    cache_dir: &std::path::Path,
    local_app_data: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let defunct = local_app_data.join("uv").join("cache");
    let same = std::fs::canonicalize(&defunct)
        .ok()
        .zip(std::fs::canonicalize(cache_dir).ok())
        .is_some_and(|(a, b)| a == b);
    (!same).then_some(defunct)
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

    #[test]
    fn orphan_points_at_defunct_default_only() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("AppData/Local");
        let live = tmp.path().join("elsewhere/uv");
        fs::create_dir_all(local.join("uv/cache")).unwrap();
        fs::create_dir_all(&live).unwrap();
        // Redirected layout: default differs from live -> orphan reported.
        let orphan = super::orphan_default(&live, &local);
        assert_eq!(orphan, Some(local.join("uv/cache")));
        // No redirect: default IS live -> no orphan.
        assert_eq!(super::orphan_default(&local.join("uv/cache"), &local), None);
    }
}
