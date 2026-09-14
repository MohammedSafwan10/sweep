//! .NET projects: report `bin/` + `obj/` next to a project manifest.
//!
//! These directories can also contain application data. Report manual
//! `dotnet clean` guidance instead of deleting whole directories.

use super::{Ctx, Detector};
use crate::model::{CleanAction, Finding, Safety};
use ignore::WalkBuilder;

pub struct DotNetDetector;

const MAX_FINDINGS: usize = 200;
const MANIFEST_EXTS: &[&str] = &["csproj", "fsproj", "vbproj"];

impl Detector for DotNetDetector {
    fn id(&self) -> &'static str {
        "dotnet-build"
    }
    fn label(&self) -> &'static str {
        ".NET build output"
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
            // Manifest dirs first; bin/obj lookups are then O(1) each.
            for project_dir in find_manifest_dirs(root, &managed) {
                let Ok(project_dir) = std::fs::canonicalize(project_dir) else {
                    continue;
                };
                if !seen.insert(project_dir.clone()) {
                    continue;
                }
                for artifact in ["bin", "obj"] {
                    if let Some(mut f) = super::dir_finding(
                        self.id(),
                        format!("{artifact}/ ({})", short_name(&project_dir)),
                        &project_dir.join(artifact),
                        Safety::Caution,
                        "Potential build output; may also contain application data. Not automatically deleted.",
                    ) {
                        f.action = CleanAction::Manual { instructions: format!("Review the project in {} and use `dotnet clean` to remove tracked build outputs. Preserve application data in bin/obj.", project_dir.display()) };
                        out.push(f);
                        if out.len() >= MAX_FINDINGS {
                            return out;
                        }
                    }
                }
            }
        }
        out
    }
}

fn is_manifest(name: &std::ffi::OsStr) -> bool {
    let Some(text) = name.to_str() else {
        return false;
    };
    let Some(dot) = text.rfind('.') else {
        return false;
    };
    let ext = &text[dot + 1..];
    MANIFEST_EXTS.iter().any(|known| {
        if cfg!(windows) {
            known.eq_ignore_ascii_case(ext)
        } else {
            *known == ext
        }
    })
}

fn find_manifest_dirs(
    root: &std::path::Path,
    managed: &std::sync::Arc<Vec<std::path::PathBuf>>,
) -> Vec<std::path::PathBuf> {
    use std::collections::HashSet;
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    // The root itself is always honored; managed children are pruned —
    // unless the root sits inside a managed tree (explicit scope).
    let outside = !managed.iter().any(|m| root.starts_with(m));
    let managed = std::sync::Arc::clone(managed);
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
                if let Some(name) = e.file_name().to_str() {
                    const SKIP: &[&str] = &[
                        ".git",
                        "node_modules",
                        "target",
                        "build",
                        ".venv",
                        "venv",
                        "bin",
                        "obj",
                        "TestResults",
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
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) && is_manifest(entry.file_name())
        {
            if let Some(parent) = entry.path().parent() {
                if seen.insert(parent.to_path_buf()) {
                    dirs.push(parent.to_path_buf());
                }
                if dirs.len() >= MAX_FINDINGS {
                    break;
                }
            }
        }
    }
    dirs
}

fn short_name(project: &std::path::Path) -> String {
    project
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use std::fs;

    #[test]
    fn flags_bin_and_obj_beside_csproj() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let proj = ctx.project_roots[0].join("app");
        fs::create_dir_all(proj.join("bin").join("Debug")).unwrap();
        fs::create_dir_all(proj.join("obj")).unwrap();
        fs::write(proj.join("app.csproj"), "<Project/>").unwrap();
        fs::write(proj.join("bin").join("Debug").join("a.dll"), vec![0u8; 60]).unwrap();
        fs::write(proj.join("obj").join("b"), vec![0u8; 40]).unwrap();
        let findings = super::DotNetDetector.scan(&ctx);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings.iter().map(|f| f.bytes).sum::<u64>(), 100);
    }

    #[test]
    fn ignores_bare_bin_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        // A bin/ dir with no manifest nearby: user data, not build output.
        // (Stored under a marker-free folder; also note "output" is not a
        // pruned name, so the walk reaches it and must still skip it.)
        let stray = ctx.project_roots[0].join("output");
        fs::create_dir_all(stray.join("bin")).unwrap();
        fs::write(stray.join("bin").join("keep.dll"), vec![0u8; 500]).unwrap();
        assert!(super::DotNetDetector.scan(&ctx).is_empty());
    }
}
