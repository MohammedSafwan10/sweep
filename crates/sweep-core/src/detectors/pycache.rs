//! Python bytecode: `__pycache__` dirs whose every `.pyc`/`.pyo` has a
//! matching source file next to it.
//!
//! Conservative by construction: a single source-less bytecode file
//! (a deployment artifact, not a cache) disqualifies the whole directory.

use super::{Ctx, Detector};
use crate::model::{CleanAction, Finding, Safety};
use crate::scanner::size_of_dir;
use ignore::WalkBuilder;

pub struct PyCacheDetector;

/// Cap on reported dirs; the walk itself is pruned to stay fast.
const MAX_FINDINGS: usize = 200;

impl Detector for PyCacheDetector {
    fn id(&self) -> &'static str {
        "pycache"
    }
    fn label(&self) -> &'static str {
        "Python bytecode caches"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        for root in &ctx.project_roots {
            if !root.is_dir() || out.len() >= MAX_FINDINGS {
                continue;
            }
            let walker = WalkBuilder::new(root)
                .hidden(false)
                .git_ignore(false)
                .ignore(false)
                .git_exclude(false)
                .git_global(false)
                .parents(false)
                .follow_links(false)
                .filter_entry(|e| {
                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        if let Some(name) = e.file_name().to_str() {
                            // Prune everything that can't contain a
                            // source-backed __pycache__.
                            const SKIP: &[&str] = &[
                                ".git",
                                "node_modules",
                                "target",
                                "build",
                                ".venv",
                                "venv",
                                ".tox",
                                ".mypy_cache",
                                ".pytest_cache",
                            ];
                            if SKIP.iter().any(|s| super::name_matches(s, name)) {
                                return false;
                            }
                        }
                        return !crate::scanner::is_link_or_reparse(e.path());
                    }
                    true
                })
                .build();
            for entry in walker {
                let Ok(entry) = entry else { continue };
                if entry.path_is_symlink() {
                    continue;
                }
                let is_cache_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    && entry
                        .file_name()
                        .to_str()
                        .is_some_and(|n| super::name_matches("__pycache__", n));
                if !is_cache_dir {
                    continue;
                }
                if let Some(finding) = self.backed_dir(entry.path()) {
                    out.push(finding);
                    if out.len() >= MAX_FINDINGS {
                        break;
                    }
                }
            }
        }
        out
    }
}

impl PyCacheDetector {
    /// A finding for the dir when it is non-empty and every bytecode file
    /// maps to an existing source file; `None` otherwise.
    fn backed_dir(&self, dir: &std::path::Path) -> Option<Finding> {
        let mut backed_bytes: u64 = 0;
        let mut count: u64 = 0;
        let Ok(entries) = std::fs::read_dir(dir) else {
            return None;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let is_bytecode = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("pyc") || e.eq_ignore_ascii_case("pyo"));
            if !is_bytecode {
                continue;
            }
            let source = bytecode_source(&path)?;
            if !source.is_file() {
                return None;
            }
            count += 1;
            backed_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
        if count == 0 || backed_bytes == 0 {
            return None;
        }
        // Re-stat the dir for the receipt so it matches cleaner accounting.
        let (bytes, _) = size_of_dir(dir);
        Some(Finding {
            detector_id: self.id().to_string(),
            label: format!("__pycache__/ ({})", short_parent(dir)),
            bytes: bytes.max(backed_bytes),
            safety: Safety::Safe,
            detail: "Bytecode with matching sources; regenerated on next import.".to_string(),
            action: CleanAction::RemovePath {
                path: dir.to_path_buf(),
            },
        })
    }
}

/// `.../pkg/__pycache__/mod.cpython-312.pyc` -> `.../pkg/mod.py`.
fn bytecode_source(pyc: &std::path::Path) -> Option<std::path::PathBuf> {
    let stem = pyc.file_stem()?.to_str()?;
    let module = stem.split('.').next()?;
    let parent = pyc.parent()?.parent()?;
    Some(parent.join(format!("{module}.py")))
}

fn short_parent(dir: &std::path::Path) -> String {
    dir.parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use std::fs;

    fn project(ctx: &Ctx, name: &str) -> std::path::PathBuf {
        let p = ctx.project_roots[0].join(name);
        fs::create_dir_all(p.join("pkg").join("__pycache__")).unwrap();
        p
    }

    #[test]
    fn flags_fully_backed_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let proj = project(&ctx, "backed");
        fs::write(proj.join("pkg").join("mod.py"), "x = 1").unwrap();
        fs::write(
            proj.join("pkg")
                .join("__pycache__")
                .join("mod.cpython-312.pyc"),
            vec![0u8; 40],
        )
        .unwrap();
        let findings = super::PyCacheDetector.scan(&ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].safety, crate::model::Safety::Safe);
    }

    #[test]
    fn sourceless_bytecode_disqualifies_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let proj = project(&ctx, "deploy");
        fs::write(proj.join("pkg").join("mod.py"), "x = 1").unwrap();
        fs::write(
            proj.join("pkg")
                .join("__pycache__")
                .join("mod.cpython-312.pyc"),
            vec![0u8; 40],
        )
        .unwrap();
        // Deployment artifact: no mod2.py anywhere.
        fs::write(
            proj.join("pkg")
                .join("__pycache__")
                .join("mod2.cpython-312.pyc"),
            vec![0u8; 40],
        )
        .unwrap();
        assert!(super::PyCacheDetector.scan(&ctx).is_empty());
    }

    #[test]
    fn empty_cache_is_silent() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let proj = project(&ctx, "empty");
        fs::write(proj.join("pkg").join("mod.py"), "x = 1").unwrap();
        assert!(super::PyCacheDetector.scan(&ctx).is_empty());
    }
}
