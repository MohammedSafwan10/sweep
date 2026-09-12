//! Gradle: stale wrapper distributions + stale version caches.
//!
//! Keeps the newest wrapper dist and newest version cache; everything
//! older is regenerable (`gradle wrapper` re-downloads on demand).

use super::{dir_finding, Ctx, Detector};
use crate::model::{Finding, Safety};

pub struct GradleDetector;

impl Detector for GradleDetector {
    fn id(&self) -> &'static str {
        "gradle"
    }
    fn label(&self) -> &'static str {
        "Gradle wrappers + caches"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        let dists = ctx.gradle_home.join("wrapper").join("dists");
        let versions = read_dir_names(&dists);
        let newest_dist = versions
            .iter()
            .filter_map(|n| parse_dist_version(n).map(|v| (v, n)))
            .max();
        for name in &versions {
            let is_newest = newest_dist.as_ref().is_some_and(|(_, n)| *n == name);
            if is_newest {
                continue;
            }
            // Only flag real `gradle-<version>-<dist>` dirs; anything else
            // (custom dists, markers) is left alone.
            if parse_dist_version(name).is_none() {
                continue;
            }
            if let Some(f) = dir_finding(
                self.id(),
                format!("gradle wrapper dist ({name})"),
                &dists.join(name),
                Safety::Safe,
                "Superseded Gradle distribution; re-downloaded if ever needed.",
            ) {
                out.push(f);
            }
        }

        let caches = ctx.gradle_home.join("caches");
        let cache_versions: Vec<String> = read_dir_names(&caches)
            .into_iter()
            .filter(|n| parse_simple_version(n).is_some())
            .collect();
        let newest_cache = cache_versions
            .iter()
            .filter_map(|n| parse_simple_version(n).map(|v| (v, n)))
            .max();
        for name in &cache_versions {
            if newest_cache.as_ref().is_some_and(|(_, n)| *n == name) {
                continue;
            }
            if let Some(f) = dir_finding(
                self.id(),
                format!("gradle version cache ({name})"),
                &caches.join(name),
                Safety::Safe,
                "Stale Gradle version cache.",
            ) {
                out.push(f);
            }
        }
        // Shared dependency cache: safe to drop but re-download is slow.
        if let Some(f) = dir_finding(
            self.id(),
            "gradle dependency cache (modules-2)".to_string(),
            &caches.join("modules-2"),
            Safety::Caution,
            "Shared dependency artifacts. Deleting forces a full re-download on next build.",
        ) {
            out.push(f);
        }
        out
    }
}

/// `gradle-8.13-all` -> (8, 13, 0). Accepts `-bin`/`-all` suffixes.
fn parse_dist_version(name: &str) -> Option<(u64, u64, u64)> {
    let rest = name.strip_prefix("gradle-")?;
    let version = rest.split('-').next()?;
    parse_simple_version(version)
}

fn parse_simple_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.split('.');
    let major: u64 = parts.next()?.parse().ok()?;
    let minor: u64 = parts.next().unwrap_or("0").parse().ok()?;
    let patch: u64 = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn read_dir_names(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        // Lossy, not loss-less filtering: unparseable names fail version
        // parsing below and are skipped — never silently corrupting the
        // "newest" calculation by vanishing.
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use std::fs;

    fn write_sized(path: &std::path::Path, n: usize) {
        fs::write(path, vec![0u8; n]).unwrap();
    }

    #[test]
    fn keeps_newest_dist_only() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        for (dist, size) in [("gradle-8.11.1-all", 10), ("gradle-9.3.1-all", 20)] {
            let d = ctx.gradle_home.join("wrapper").join("dists").join(dist);
            fs::create_dir_all(&d).unwrap();
            write_sized(&d.join("f"), size);
        }
        // A non-gradle dir must never be flagged.
        let other = ctx
            .gradle_home
            .join("wrapper")
            .join("dists")
            .join("custom-distro");
        fs::create_dir_all(&other).unwrap();
        write_sized(&other.join("f"), 999);

        let findings = super::GradleDetector.scan(&ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].label.contains("8.11.1"));
        assert_eq!(findings[0].bytes, 10);
    }

    #[test]
    fn version_parsing() {
        assert_eq!(
            super::parse_dist_version("gradle-8.13-all"),
            Some((8, 13, 0))
        );
        assert_eq!(
            super::parse_dist_version("gradle-9.1.0-bin"),
            Some((9, 1, 0))
        );
        assert_eq!(super::parse_dist_version("custom-distro"), None);
        assert_eq!(super::parse_simple_version("9.3"), Some((9, 3, 0)));
        assert_eq!(super::parse_simple_version("nope"), None);
    }
}
