//! Dart/Flutter pub cache (`PUB_CACHE` or `%LOCALAPPDATA%\Pub\Cache`).

use super::{dir_finding, Ctx, Detector};
use crate::model::{Finding, Safety};

pub struct PubCacheDetector;

impl Detector for PubCacheDetector {
    fn id(&self) -> &'static str {
        "pub-cache"
    }
    fn label(&self) -> &'static str {
        "Dart pub package cache"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        for (name, detail) in [
            (
                "hosted",
                "Downloaded pub.dev packages; restored by `flutter pub get`.",
            ),
            (
                "git",
                "Git-sourced packages; restored by `flutter pub get`.",
            ),
            (
                "_temp",
                "Leftover temp files from interrupted pub operations.",
            ),
        ] {
            if let Some(f) = dir_finding(
                self.id(),
                format!("pub cache/{name}"),
                &ctx.pub_cache.join(name),
                Safety::Safe,
                detail,
            ) {
                out.push(f);
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
    fn finds_hosted_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let hosted = ctx.pub_cache.join("hosted").join("pub.dev");
        fs::create_dir_all(&hosted).unwrap();
        fs::write(hosted.join("pkg-1.0.0.tar"), vec![0u8; 64]).unwrap();
        let findings = super::PubCacheDetector.scan(&ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].bytes, 64);
    }
}
