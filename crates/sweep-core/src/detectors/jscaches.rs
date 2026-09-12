//! JS caches: npm cache, pnpm store, yarn cache.

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
                "npm cache",
                ctx.npm_cache.clone(),
                "Restored automatically; `npm cache verify` compacts it.",
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
}
