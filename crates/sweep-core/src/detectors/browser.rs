//! Chromium browser caches: per-profile regenerable storage plus the
//! on-device AI model.
//!
//! Measured on a real machine: Chrome's `User Data` held 8.2 GiB —
//! 4.0 GiB of it a single `weights.bin` (on-device model) and 1.4 GiB of
//! Service Worker cache. No previous detector covered browsers.
//! Storage caches are SAFE (the browser rebuilds them); the model weights
//! are CAUTION (multi-gigabyte re-download).

use super::{dir_finding, Ctx, Detector};
use crate::model::{Finding, Safety};

pub struct BrowserCacheDetector;

/// Per-profile subdirs that are pure regenerable HTTP/GPU caches.
/// NOTE: CacheStorage / Service Worker storage is deliberately NOT here:
/// the Cache API is origin storage where offline-first apps can keep
/// user-authored payloads — that is CAUTION, handled separately below.
const SAFE_DIRS: &[&str] = &["Cache", "Code Cache", "ShaderCache", "GPUCache"];

/// Chrome-created profile dir names. Anything else under `User Data`
/// (`System Profile`, `GPUPersistentCache`, …) is browser internals,
/// not a user profile — never probe it as one.
fn is_profile(name: &str) -> bool {
    name == "Default" || name == "Guest Profile" || name.starts_with("Profile ")
}

impl Detector for BrowserCacheDetector {
    fn id(&self) -> &'static str {
        "browser-cache"
    }
    fn label(&self) -> &'static str {
        "Browser web caches"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        for base in [
            ctx.local_app_data
                .join("Google")
                .join("Chrome")
                .join("User Data"),
            ctx.local_app_data
                .join("Microsoft")
                .join("Edge")
                .join("User Data"),
        ] {
            if !base.is_dir() {
                continue;
            }
            let browser = if base.to_string_lossy().contains("Edge") {
                "edge"
            } else {
                "chrome"
            };
            // On-device model weights: versioned dir, huge single file.
            let dir = base.join("OptGuideOnDeviceModel");
            if dir.is_dir() {
                if let Some(f) = dir_finding(
                    self.id(),
                    format!("{browser} on-device model"),
                    &dir,
                    Safety::Caution,
                    "Browser AI model weights; re-downloaded on demand (gigabytes).",
                ) {
                    out.push(f);
                }
            }
            // Browser-level GPU shader caches (outside any profile).
            for shared in ["ShaderCache", "GrShaderCache", "GPUCache"] {
                let dir = base.join(shared);
                if dir.is_dir() {
                    if let Some(f) = dir_finding(
                        self.id(),
                        format!("{browser} {}", shared.to_lowercase()),
                        &dir,
                        Safety::Safe,
                        "GPU shader cache; the browser rebuilds it on next launch.",
                    ) {
                        out.push(f);
                    }
                }
            }
            // Per-profile storage caches.
            if let Ok(profiles) = std::fs::read_dir(&base) {
                for profile in profiles.filter_map(|e| e.ok()) {
                    let profile = profile.path();
                    if !profile.is_dir() || crate::scanner::is_link_or_reparse(&profile) {
                        continue;
                    }
                    let name = profile
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    if !is_profile(&name) {
                        continue;
                    }
                    // Origin storage: offline-first apps can keep
                    // user-authored payloads here. CAUTION, and close the
                    // browser first (files may be locked while running).
                    for storage in ["CacheStorage", "Service Worker/CacheStorage"] {
                        let dir = profile.join(storage);
                        if let Some(f) = dir_finding(
                            self.id(),
                            format!("{browser} {name}/origin storage"),
                            &dir,
                            Safety::Caution,
                            "Site origin storage (Cache API); may hold offline app data. Close the browser first; files may be locked.",
                        ) {
                            out.push(f);
                        }
                    }
                    for cache in SAFE_DIRS {
                        let dir = profile.join(cache);
                        if let Some(f) = dir_finding(
                            self.id(),
                            format!("{browser} {name}/{}", cache.to_lowercase()),
                            &dir,
                            Safety::Safe,
                            "Browser web cache; rebuilt automatically on next visit. Close the browser first; files may be locked while running.",
                        ) {
                            out.push(f);
                        }
                    }
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
    fn flags_profile_caches_and_model() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        // for_tests puts LOCALAPPDATA under root/AppData/Local.
        let profile = ctx.local_app_data.join("Google/Chrome/User Data/Profile 1");
        fs::create_dir_all(profile.join("Cache")).unwrap();
        fs::write(profile.join("Cache").join("blob"), vec![0u8; 64]).unwrap();
        fs::create_dir_all(profile.join("CacheStorage")).unwrap();
        fs::write(profile.join("CacheStorage").join("blob"), vec![0u8; 64]).unwrap();
        // Non-profile internals must not be probed as profiles.
        fs::create_dir_all(
            ctx.local_app_data
                .join("Google/Chrome/User Data/GPUPersistentCache"),
        )
        .unwrap();
        let model = ctx
            .local_app_data
            .join("Google/Chrome/User Data/OptGuideOnDeviceModel");
        fs::create_dir_all(&model).unwrap();
        fs::write(model.join("weights.bin"), vec![0u8; 128]).unwrap();

        let findings = super::BrowserCacheDetector.scan(&ctx);
        assert_eq!(findings.len(), 3);
        let cache = findings
            .iter()
            .find(|f| f.label.ends_with("/cache"))
            .unwrap();
        assert_eq!(cache.safety, crate::model::Safety::Safe);
        // Origin storage is CAUTION: offline apps may keep user data there.
        assert!(findings.iter().any(
            |f| f.label.contains("origin storage") && f.safety == crate::model::Safety::Caution
        ));
        let weights = findings.iter().find(|f| f.label.contains("model")).unwrap();
        assert_eq!(weights.safety, crate::model::Safety::Caution);
    }

    #[test]
    fn profile_allowlist() {
        assert!(super::is_profile("Default"));
        assert!(super::is_profile("Profile 1"));
        assert!(super::is_profile("Guest Profile"));
        assert!(!super::is_profile("System Profile"));
        assert!(!super::is_profile("GPUPersistentCache"));
        assert!(!super::is_profile("ShaderCache"));
    }
}
