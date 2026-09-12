//! Detector registry: each detector knows where one ecosystem hides
//! its regenerable bytes and how safe each location is to delete.
//!
//! Detectors are pure functions of [`Ctx`] so tests inject a fake home
//! directory instead of touching the real machine.

pub mod android;
pub mod angular;
pub mod cargo;
pub mod docker;
pub mod dotnet;
pub mod flutter;
pub mod gradle;
pub mod jscaches;
pub mod nextjs;
pub mod pip_cache;
pub mod pubcache;
pub mod pycache;
pub mod pytest_caches;
pub mod temp;
pub mod uv_cache;
pub mod vite;

use crate::model::{CleanAction, Finding, Safety};
use crate::scanner::size_of_dir;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

/// All locations a detector may need. Resolved from the environment in
/// production, pointed at a temp dir in tests.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub home: PathBuf,
    pub local_app_data: PathBuf,
    pub cargo_home: PathBuf,
    pub rustup_home: PathBuf,
    pub pub_cache: PathBuf,
    pub android_sdk: Option<PathBuf>,
    pub npm_cache: PathBuf,
    pub pnpm_store: PathBuf,
    pub yarn_cache: PathBuf,
    pub pip_cache: PathBuf,
    pub uv_cache: PathBuf,
    pub gradle_home: PathBuf,
    pub temp_dir: PathBuf,
    pub docker_data_files: Vec<PathBuf>,
    /// Roots walked to find project artifact dirs (target/, build/, ...).
    pub project_roots: Vec<PathBuf>,
    pub docker_enabled: bool,
}

impl Ctx {
    /// Resolve from environment variables with platform defaults.
    pub fn from_env() -> Self {
        let home = dirs_home();
        let local_app_data = var_path("LOCALAPPDATA")
            .or_else(|| directories::BaseDirs::new().map(|b| b.data_local_dir().to_path_buf()))
            .unwrap_or_else(|| home.join("AppData").join("Local"));
        let cargo_home = var_path("CARGO_HOME").unwrap_or_else(|| home.join(".cargo"));
        let rustup_home = var_path("RUSTUP_HOME").unwrap_or_else(|| home.join(".rustup"));
        let pub_cache =
            var_path("PUB_CACHE").unwrap_or_else(|| local_app_data.join("Pub").join("Cache"));
        let android_sdk = var_path("ANDROID_SDK_ROOT")
            .or_else(|| var_path("ANDROID_HOME"))
            .or_else(|| {
                let p = local_app_data.join("Android").join("Sdk");
                p.is_dir().then_some(p)
            });
        let npm_cache = var_path("npm_config_cache")
            .or_else(|| var_path("NPM_CONFIG_CACHE"))
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    local_app_data.join("npm-cache")
                } else {
                    home.join(".npm")
                }
            });
        let pnpm_store = local_app_data.join("pnpm").join("store");
        let yarn_cache = var_path("YARN_CACHE_FOLDER")
            .unwrap_or_else(|| local_app_data.join("Yarn").join("Cache"));
        let pip_cache = var_path("PIP_CACHE_DIR").unwrap_or_else(|| {
            if cfg!(windows) {
                local_app_data.join("pip").join("Cache")
            } else {
                var_path("XDG_CACHE_HOME")
                    .filter(|p| p.is_absolute())
                    .unwrap_or_else(|| {
                        if cfg!(target_os = "macos") {
                            home.join("Library/Caches")
                        } else {
                            home.join(".cache")
                        }
                    })
                    .join("pip")
            }
        });
        let uv_cache = var_path("UV_CACHE_DIR").unwrap_or_else(|| {
            if cfg!(windows) {
                local_app_data.join("uv").join("cache")
            } else {
                var_path("XDG_CACHE_HOME")
                    .filter(|p| p.is_absolute())
                    .unwrap_or_else(|| home.join(".cache"))
                    .join("uv")
            }
        });
        let gradle_home = var_path("GRADLE_USER_HOME").unwrap_or_else(|| home.join(".gradle"));
        let temp_dir = std::env::temp_dir();
        let docker_data_files = vec![
            local_app_data
                .join("Docker")
                .join("wsl")
                .join("disk")
                .join("docker_data.vhdx"),
            local_app_data
                .join("Docker")
                .join("wsl")
                .join("data")
                .join("ext4.vhdx"),
        ];
        let project_roots = vec![std::env::current_dir()
            .ok()
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| home.clone())];
        Self {
            home,
            local_app_data,
            cargo_home,
            rustup_home,
            pub_cache,
            android_sdk,
            npm_cache,
            pnpm_store,
            yarn_cache,
            pip_cache,
            uv_cache,
            gradle_home,
            temp_dir,
            docker_data_files,
            project_roots,
            docker_enabled: true,
        }
    }

    /// Point every location under `root` for hermetic tests (and for
    /// agents that want to scan a sandbox before touching the real PC).
    pub fn for_tests(root: &Path) -> Self {
        let local = root.join("AppData").join("Local");
        Self {
            home: root.to_path_buf(),
            local_app_data: local.clone(),
            cargo_home: root.join(".cargo"),
            rustup_home: root.join(".rustup"),
            pub_cache: local.join("Pub").join("Cache"),
            android_sdk: Some(local.join("Android").join("Sdk")),
            npm_cache: local.join("npm-cache"),
            pnpm_store: local.join("pnpm").join("store"),
            yarn_cache: local.join("Yarn").join("Cache"),
            pip_cache: local.join("pip").join("Cache"),
            uv_cache: local.join("uv").join("cache"),
            gradle_home: root.join(".gradle"),
            temp_dir: root.join("Temp"),
            docker_data_files: vec![local.join("Docker").join("docker_data.vhdx")],
            project_roots: vec![root.join("projects")],
            docker_enabled: true,
        }
    }
}

fn dirs_home() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .or_else(|| var_path("USERPROFILE"))
        .or_else(|| var_path("HOME"))
        // A relative fallback like "." would poison every derived path
        // (cargo_home, gradle_home, ...) into CWD-relative locations.
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Non-empty env var as a path. Empty strings must not become `""` joins
/// (which resolve against the current directory).
fn var_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Name comparison following filesystem rules: ASCII case-insensitive
/// on Windows (where `Build` and `build` are the same dir), exact
/// elsewhere (where they can be different dirs).
pub(crate) fn name_matches(known: &str, actual: &str) -> bool {
    if cfg!(windows) {
        known.eq_ignore_ascii_case(actual)
    } else {
        known == actual
    }
}

/// True when a `package.json` manifest lists `dep` in dependencies,
/// devDependencies or peerDependencies. Broken JSON or missing files
/// are `false` — never an error.
pub(crate) fn package_has_dep(manifest: &Path, dep: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(manifest) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    ["dependencies", "devDependencies", "peerDependencies"]
        .iter()
        .any(|section| {
            json.get(section)
                .and_then(|d| d.get(dep))
                .and_then(|v| v.as_str())
                .is_some_and(|v| !v.trim().is_empty())
        })
}
/// One detector: one ecosystem's known cache/artifact locations.
pub trait Detector: Send + Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn scan(&self, ctx: &Ctx) -> Vec<Finding>;
}

/// All detectors in a stable order (registry order = display order
/// before size sorting).
pub fn all_detectors() -> Vec<Box<dyn Detector>> {
    vec![
        Box::new(cargo::CargoDetector),
        Box::new(pubcache::PubCacheDetector),
        Box::new(jscaches::JsCachesDetector),
        Box::new(pip_cache::PipCacheDetector),
        Box::new(uv_cache::UvCacheDetector),
        Box::new(pycache::PyCacheDetector),
        Box::new(pytest_caches::PyTestCachesDetector),
        Box::new(vite::ViteDetector),
        Box::new(angular::AngularDetector),
        Box::new(dotnet::DotNetDetector),
        Box::new(gradle::GradleDetector),
        Box::new(android::AndroidDetector),
        Box::new(flutter::FlutterDetector),
        Box::new(nextjs::NextJsDetector),
        Box::new(docker::DockerDetector),
        Box::new(temp::TempDetector),
    ]
}

/// Run every detector, largest finding first.
pub fn scan_all(ctx: &Ctx) -> Vec<Finding> {
    scan_selected(ctx, &[])
}

/// Scan only requested detector IDs, or every detector when empty.
pub fn scan_selected(ctx: &Ctx, ids: &[String]) -> Vec<Finding> {
    let mut out = Vec::new();
    for d in all_detectors() {
        if !ids.is_empty() && !ids.iter().any(|id| id == d.id()) {
            continue;
        }
        if d.id() == "docker" && !ctx.docker_enabled {
            continue;
        }
        out.extend(d.scan(ctx));
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.bytes));
    out
}

/// Helper: a `RemovePath` finding for an existing dir, sized on the spot.
/// Returns `None` when the path does not exist or is empty.
pub(crate) fn dir_finding(
    detector_id: &'static str,
    label: impl Into<String>,
    path: &Path,
    safety: Safety,
    detail: impl Into<String>,
) -> Option<Finding> {
    if !path.is_dir() || crate::scanner::is_link_or_reparse(path) {
        return None;
    }
    let (bytes, _) = size_of_dir(path);
    if bytes == 0 {
        return None;
    }
    Some(Finding {
        detector_id: detector_id.to_string(),
        label: label.into(),
        bytes,
        safety,
        detail: detail.into(),
        action: CleanAction::RemovePath {
            path: path.to_path_buf(),
        },
    })
}

/// Walk `roots` for project markers (e.g. `pubspec.yaml`), returning the
/// directories that contain one. Artifact/output dirs are pruned from the
/// walk so scanning stays fast. Capped at 500 projects.
pub(crate) fn find_projects(roots: &[PathBuf], marker: &str) -> Vec<PathBuf> {
    const SKIP: &[&str] = &[
        "build",
        ".dart_tool",
        "node_modules",
        ".git",
        "target",
        ".next",
        "out",
        "dist",
        ".gradle",
        "Pods",
        ".venv",
        "venv",
        "__pycache__",
    ];
    let mut projects = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        if !root.is_dir() || projects.len() >= 500 {
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
                        if SKIP.iter().any(|s| name_matches(s, name)) {
                            return false;
                        }
                    }
                    // Never descend into links/junctions: artifact dirs must
                    // be real directories inside the project.
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
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| name_matches(marker, n))
            {
                if let Some(parent) = entry.path().parent() {
                    let Ok(parent) = std::fs::canonicalize(parent) else {
                        continue;
                    };
                    if !seen.insert(parent.clone()) {
                        continue;
                    }
                    projects.push(parent.to_path_buf());
                    if projects.len() >= 500 {
                        break;
                    }
                }
            }
        }
    }
    projects
}
