//! Python test/lint caches: `.pytest_cache`, `.mypy_cache`,
//! `.ruff_cache`, `.hypothesis`, `.coverage*`.
//!
//! Only flagged next to a Python project marker (`pyproject.toml`,
//! `setup.py`, `setup.cfg`, `requirements.txt`, `tox.ini`) — never by
//! bare directory name alone.

use super::{dir_finding, Ctx, Detector};
use crate::model::{Finding, Safety};
use ignore::WalkBuilder;

pub struct PyTestCachesDetector;

const MAX_FINDINGS: usize = 200;
const CACHE_DIRS: &[&str] = &[".pytest_cache", ".mypy_cache", ".ruff_cache", ".hypothesis"];
const PROJECT_MARKERS: &[&str] = &[
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "requirements.txt",
    "tox.ini",
];

impl Detector for PyTestCachesDetector {
    fn id(&self) -> &'static str {
        "pytest-caches"
    }
    fn label(&self) -> &'static str {
        "Python test/lint caches"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let managed: std::sync::Arc<Vec<std::path::PathBuf>> =
            std::sync::Arc::new(ctx.managed_roots());
        for root in &ctx.project_roots {
            if !root.is_dir() || out.len() >= MAX_FINDINGS {
                continue;
            }
            let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
            // The root itself is always honored; managed children are
            // pruned — unless the root sits inside a managed tree.
            let outside = !managed.iter().any(|m| root.starts_with(m));
            let managed = std::sync::Arc::clone(&managed);
            let walker = WalkBuilder::new(&root)
                .hidden(false)
                .git_ignore(false)
                .ignore(false)
                .git_exclude(false)
                .git_global(false)
                .parents(false)
                .follow_links(false)
                .filter_entry(move |e| {
                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        if outside && managed.iter().any(|m| e.path().starts_with(m)) {
                            return false;
                        }
                        if e.path().join("pyvenv.cfg").is_file() {
                            return false;
                        }
                        if let Some(name) = e.file_name().to_str() {
                            const SKIP: &[&str] = &[
                                ".git",
                                "node_modules",
                                "target",
                                "build",
                                ".venv",
                                "venv",
                                ".tox",
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
                let path = entry.path();
                let name = entry.file_name().to_str().unwrap_or("");
                let is_cache_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    && CACHE_DIRS.iter().any(|c| super::name_matches(c, name));
                let is_coverage = entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                    && (super::name_matches(".coverage", name)
                        || name_matches_prefix(".coverage.", name));
                if !is_cache_dir && !is_coverage {
                    continue;
                }
                if is_coverage && !is_coverage_data(path) {
                    continue;
                }
                // Validated by a Python project marker in the same dir —
                // never by bare directory name alone.
                if !path.parent().is_some_and(has_python_marker) {
                    continue;
                }
                let Ok(resolved) = std::fs::canonicalize(path) else {
                    continue;
                };
                if !seen.insert(resolved) {
                    continue;
                }
                let label = if is_cache_dir {
                    format!("{name}/ ({})", short_parent(path))
                } else {
                    format!("{name} ({})", short_parent(path))
                };
                if let Some(mut f) = dir_finding_or_file(
                    self.id(),
                    label,
                    path,
                    Safety::Safe,
                    "Test/lint cache; regenerated on next run.",
                ) {
                    if is_coverage || super::name_matches(".hypothesis", name) {
                        f.safety = Safety::Caution;
                        f.detail =
                            "Stored test results or examples may be needed to reproduce failures."
                                .into();
                        f.action = crate::model::CleanAction::Manual { instructions: format!("Review {} and retain any results or failing examples you need before removing it.", path.display()) };
                    }
                    out.push(f);
                    if out.len() >= MAX_FINDINGS {
                        break;
                    }
                }
            }
        }
        out
    }
}

fn name_matches_prefix(prefix: &str, name: &str) -> bool {
    if cfg!(windows) {
        name.get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    } else {
        name.starts_with(prefix)
    }
}

fn is_coverage_data(path: &std::path::Path) -> bool {
    use std::io::Read;
    if crate::scanner::is_link_or_reparse(path) {
        return false;
    }
    let mut header = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok()
        && &header == b"SQLite format 3\0"
}

fn has_python_marker(dir: &std::path::Path) -> bool {
    if PROJECT_MARKERS.iter().any(|m| dir.join(m).is_file()) {
        return true;
    }
    // requirements-dev.txt style names (case-insensitive for Windows).
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.filter_map(|e| e.ok()).any(|e| {
            if !e.file_type().is_ok_and(|t| t.is_file()) {
                return false;
            }
            e.file_name().to_str().is_some_and(|n| {
                let lower = n.to_ascii_lowercase();
                lower.starts_with("requirements-") && lower.ends_with(".txt")
            })
        })
    })
}

fn short_parent(path: &std::path::Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "?".to_string())
}

/// `dir_finding` for dirs, file-sized finding for single files.
fn dir_finding_or_file(
    detector_id: &'static str,
    label: String,
    path: &std::path::Path,
    safety: Safety,
    detail: &str,
) -> Option<Finding> {
    if path.is_dir() {
        return dir_finding(detector_id, label, path, safety, detail);
    }
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() || meta.len() == 0 {
        return None;
    }
    Some(Finding {
        detector_id: detector_id.to_string(),
        label,
        bytes: meta.len(),
        safety,
        detail: detail.to_string(),
        action: crate::model::CleanAction::RemovePath {
            path: path.to_path_buf(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use std::fs;

    #[test]
    fn flags_validated_caches_only() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let proj = ctx.project_roots[0].join("pyproj");
        fs::create_dir_all(proj.join(".pytest_cache").join("v")).unwrap();
        fs::create_dir_all(proj.join(".mypy_cache")).unwrap();
        fs::write(proj.join("pyproject.toml"), "[project]").unwrap();
        fs::write(
            proj.join(".pytest_cache").join("v").join("a"),
            vec![0u8; 30],
        )
        .unwrap();
        fs::write(proj.join(".mypy_cache").join("b"), vec![0u8; 20]).unwrap();
        // Unvalidated lookalike elsewhere: must be ignored.
        let stray = ctx.project_roots[0].join("docs");
        fs::create_dir_all(stray.join(".pytest_cache")).unwrap();
        fs::write(stray.join(".pytest_cache").join("c"), vec![0u8; 999]).unwrap();

        let findings = super::PyTestCachesDetector.scan(&ctx);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings.iter().map(|f| f.bytes).sum::<u64>(), 50);
    }

    #[test]
    fn coverage_files_need_markers_too() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let proj = ctx.project_roots[0].join("cov");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join("setup.cfg"), "[coverage]").unwrap();
        fs::write(proj.join(".coverage"), b"SQLite format 3\0").unwrap();
        let findings = super::PyTestCachesDetector.scan(&ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].bytes, 16);
    }
}
