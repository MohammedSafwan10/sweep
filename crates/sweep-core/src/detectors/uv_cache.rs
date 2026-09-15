//! uv cache: report-only in v1.
//!
//! uv explicitly documents direct cache modification as unsafe, so sweep
//! reports the size and points at uv's own commands (`uv cache clean`,
//! `uv cache prune`). The finding is Manual and never auto-deleted.
//!
//! The cache lock (`.lock`) tells us whether a uv process is using the
//! cache right now — `uvx`-launched tools (MCP servers) hold it for their
//! whole lifetime, which is why `uv cache prune` can sit 300s waiting.
//! A held lock is surfaced upfront. A redirected default-location cache
//! is only reported as a stale leftover when its lock is provably free;
//! otherwise it is live and must not be called stale.

use super::{dir_finding, Ctx, Detector};
use crate::model::{CleanAction, Finding, Safety};
use crate::scanner::size_of_dir;
use fs4::fs_std::FileExt;
use std::path::{Path, PathBuf};

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
                let in_use = cache_in_use(&ctx.uv_cache) == Some(true);
                out.push(Finding {
                    detector_id: self.id().to_string(),
                    label: "uv cache".to_string(),
                    bytes,
                    safety: Safety::Caution,
                    detail: if in_use {
                        "uv's cache — a uv process is using it right now (cache lock held). Direct modification is unsafe per uv docs.".to_string()
                    } else {
                        "uv's cache. Direct modification is unsafe per uv docs; use uv itself."
                            .to_string()
                    },
                    action: CleanAction::Manual {
                        instructions: if in_use {
                            "Close the uv/uvx process using this cache (uvx-launched tools and MCP servers hold the lock for their lifetime), then run `uv cache prune` or `uv cache clean`. Tip: set UV_LOCK_TIMEOUT=10 to fail fast instead of uv's 300s wait.".to_string()
                        } else {
                            "Run `uv cache prune` (safe refresh) or `uv cache clean` (full wipe)."
                                .to_string()
                        },
                    },
                });
            }
        }
        // Orphaned default location: when UV_CACHE_DIR redirects elsewhere
        // AND nothing holds the default cache's lock, the old default can
        // accumulate gigabytes no uv command will ever prune. A held lock
        // means uv processes still use it (redirect is per-process), so it
        // is live, not stale.
        let redirected = std::env::var_os("UV_CACHE_DIR").is_some_and(|v| !v.is_empty());
        if let Some(defunct) = stale_orphan(&ctx.uv_cache, &ctx.local_app_data, redirected) {
            if let Some(f) = dir_finding(
                self.id(),
                "stale default uv cache".to_string(),
                &defunct,
                Safety::Caution,
                "UV_CACHE_DIR points elsewhere and nothing holds this cache's lock; uv never prunes it.",
            ) {
                if f.bytes >= ORPHAN_MIN_BYTES {
                    out.push(f);
                }
            }
        }
        out
    }
}

/// Whether a uv process currently holds the cache lock.
/// `Some(true)` = lock held by someone else; `Some(false)` = free (or no
/// lock file: nothing is using the dir); `None` = cannot tell.
///
/// fs4's `try_lock_exclusive` follows flock/LockFileEx semantics:
/// `Ok(true)` = we acquired it (nobody else holds it), `Ok(false)` =
/// another process holds it, `Err` = could not determine.
fn cache_in_use(cache_dir: &Path) -> Option<bool> {
    let lock_path = cache_dir.join(".lock");
    if !lock_path.exists() {
        return Some(false);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .ok()?;
    match file.try_lock_exclusive() {
        Ok(true) => {
            // Explicit UFCS: `File::unlock` became an inherent std method in
            // Rust 1.89; this workspace's MSRV is lower and the lock was
            // taken through fs4, so it must be released through fs4.
            let _ = fs4::fs_std::FileExt::unlock(&file);
            Some(false)
        }
        Ok(false) => Some(true),
        Err(_) => None,
    }
}

/// Default-location cache when it is a genuinely idle leftover:
/// `UV_CACHE_DIR` redirects elsewhere, the default differs from the live
/// cache, and its lock is provably free.
/// Pure in its inputs (helper is unit-testable); existence is checked by
/// the caller via `dir_finding`.
fn stale_orphan(cache_dir: &Path, local_app_data: &Path, redirected: bool) -> Option<PathBuf> {
    if !redirected {
        return None;
    }
    let defunct = local_app_data.join("uv").join("cache");
    let same = std::fs::canonicalize(&defunct)
        .ok()
        .zip(std::fs::canonicalize(cache_dir).ok())
        .is_some_and(|(a, b)| a == b);
    if same || !defunct.is_dir() || cache_in_use(&defunct) != Some(false) {
        return None;
    }
    Some(defunct)
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use fs4::fs_std::FileExt;
    use std::fs;

    /// Re-exec mode marker: the child process locks the file in this env
    /// var and waits, simulating a running uv/uvx process. Windows locks
    /// are re-entrant within one process, so a real second process is
    /// required to test the in-use path at all.
    const HOLDER_ENV: &str = "SWEEP_TEST_UV_LOCK_HOLDER";

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
    fn orphan_requires_redirect_difference_and_free_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("AppData/Local");
        let live = tmp.path().join("elsewhere/uv");
        let default = local.join("uv/cache");
        fs::create_dir_all(&default).unwrap();
        fs::create_dir_all(&live).unwrap();

        // No redirect -> never an orphan.
        assert_eq!(super::stale_orphan(&live, &local, false), None);
        // Redirected, default idle (no lock file) -> orphan.
        assert_eq!(
            super::stale_orphan(&live, &local, true),
            Some(default.clone())
        );
        // Live cache IS the default -> not an orphan.
        assert_eq!(super::stale_orphan(&default, &local, true), None);
    }

    #[test]
    fn held_lock_marks_cache_live_not_stale() {
        // Helper mode: lock the target file, announce, and wait to be
        // killed by the parent test process.
        if let Some(path) = std::env::var_os(HOLDER_ENV) {
            let path = std::path::PathBuf::from(path);
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .unwrap();
            file.lock_exclusive().unwrap();
            fs::write(path.with_extension("ready"), b"1").unwrap();
            std::thread::sleep(std::time::Duration::from_secs(60));
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("AppData/Local");
        let live = tmp.path().join("elsewhere/uv");
        let default = local.join("uv/cache");
        fs::create_dir_all(&default).unwrap();
        fs::create_dir_all(&live).unwrap();
        let lock_path = default.join(".lock");
        let ready = lock_path.with_extension("ready");

        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "detectors::uv_cache::tests::held_lock_marks_cache_live_not_stale",
            ])
            .env(HOLDER_ENV, &lock_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();

        // Bounded wait for the child to actually hold the lock.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !ready.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "lock holder never became ready"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        assert_eq!(super::cache_in_use(&default), Some(true));
        assert_eq!(super::stale_orphan(&live, &local, true), None);

        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_file(&ready);

        // Lock is released once the holder process is gone; allow a brief
        // grace period for the OS to drop it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if super::cache_in_use(&default) == Some(false) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "lock still reported held after holder exit"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(super::stale_orphan(&live, &local, true), Some(default));
    }
}
