//! JS caches: npm download cache, pnpm store, yarn cache.

use super::{dir_finding, Ctx, Detector};
use crate::model::{Finding, Safety};

pub struct JsCachesDetector;

impl Detector for JsCachesDetector {
    fn id(&self) -> &'static str {
        "js-caches"
    }
    fn label(&self) -> &'static str {
        "npm / pnpm / yarn caches"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        let candidates: &[(&str, std::path::PathBuf, &str)] = &[
            (
                "npm download cache",
                ctx.npm_cache.join("_cacache"),
                "Downloaded packages are restored automatically; preserves live npm exec (_npx) tools.",
            ),
            (
                "pnpm store",
                ctx.pnpm_store.clone(),
                "Content-addressable store; `pnpm store prune` removes the rest.",
            ),
            (
                "yarn cache",
                ctx.yarn_cache.clone(),
                "Restored on next install; `yarn cache clean` equivalent.",
            ),
        ];
        for (name, path, detail) in candidates {
            if let Some(f) =
                dir_finding(self.id(), (*name).to_string(), path, Safety::Safe, *detail)
            {
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
    fn finds_npm_and_pnpm() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        fs::create_dir_all(ctx.npm_cache.join("_cacache")).unwrap();
        fs::write(ctx.npm_cache.join("_cacache").join("a"), vec![0u8; 10]).unwrap();
        fs::create_dir_all(ctx.pnpm_store.join("v3")).unwrap();
        fs::write(ctx.pnpm_store.join("v3").join("b"), vec![0u8; 20]).unwrap();
        let findings = super::JsCachesDetector.scan(&ctx);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings.iter().map(|f| f.bytes).sum::<u64>(), 30);
    }

    #[test]
    fn npm_finding_preserves_live_npx_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        fs::create_dir_all(ctx.npm_cache.join("_cacache")).unwrap();
        fs::write(ctx.npm_cache.join("_cacache").join("package"), b"package").unwrap();
        fs::create_dir_all(ctx.npm_cache.join("_npx").join("running-tool")).unwrap();
        fs::write(ctx.npm_cache.join("_npx/running-tool/index.js"), b"tool").unwrap();

        let finding = super::JsCachesDetector
            .scan(&ctx)
            .into_iter()
            .find(|finding| finding.label == "npm download cache")
            .unwrap();
        match finding.action {
            crate::model::CleanAction::RemovePath { path } => {
                assert_eq!(path, ctx.npm_cache.join("_cacache"));
            }
            other => panic!("unexpected action: {other:?}"),
        }
        assert_eq!(finding.bytes, 7);
    }
}
