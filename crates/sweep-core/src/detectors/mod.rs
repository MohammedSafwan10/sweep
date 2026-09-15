//! Detector registry: each detector knows where one ecosystem hides
//! its regenerable bytes and how safe each location is to delete.
//!
//! Detectors are pure functions of [`Ctx`] so tests inject a fake home
//! directory instead of touching the real machine.

pub mod agent;
pub mod android;
pub mod angular;
pub mod browser;
pub mod cargo;
pub mod docker;
pub mod dotnet;
pub mod flutter;
pub mod gocache;
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
pub mod winupdate;

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
    /// Windows installation root (report-only detectors). `None` on
    /// non-Windows and in hermetic tests with no fixture.
    pub system_root: Option<PathBuf>,
    pub go_build_cache: PathBuf,
    pub go_mod_cache: PathBuf,
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
        let system_root =
            var_path("SystemRoot").or_else(|| cfg!(windows).then(|| PathBuf::from(r"C:\Windows")));
        // Go caches: explicit env wins, else documented platform defaults.
        // (v1 never runs `go env`; see SAFETY.md.)
        let go_path = var_path("GOPATH").unwrap_or_else(|| home.join("go"));
        let go_build_cache = var_path("GOCACHE").unwrap_or_else(|| {
            if cfg!(windows) {
                local_app_data.join("go-build")
            } else {
                var_path("XDG_CACHE_HOME")
                    .filter(|p| p.is_absolute())
                    .unwrap_or_else(|| home.join(".cache"))
                    .join("go-build")
            }
        });
        let go_mod_cache =
            var_path("GOMODCACHE").unwrap_or_else(|| go_path.join("pkg").join("mod"));
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
            system_root,
            go_build_cache,
            go_mod_cache,
            docker_data_files,
            project_roots,
            docker_enabled: true,
        }
    }

    /// Managed cache/runtime trees that never contain user projects.
    /// Used to prune project-marker walks (which would otherwise descend
    /// into e.g. `Pub/Cache` fixture dirs and emit false findings).
    /// Canonicalized for lexical `starts_with` comparison against walk
    /// paths; entries that fail to canonicalize are skipped (safe
    /// direction: a missed prune costs speed, never correctness).
    pub fn managed_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![
            self.local_app_data.clone(),
            // Whole AppData tree (Local + Roaming): runtimes live in both
            // (e.g. uv Pythons under Roaming), never user projects.
            self.home.join("AppData"),
            self.home.join(".cache"),
            self.home.join(".codex"),
            self.cargo_home.clone(),
            self.rustup_home.clone(),
            self.pub_cache.clone(),
            self.npm_cache.clone(),
            self.pnpm_store.clone(),
            self.yarn_cache.clone(),
            self.pip_cache.clone(),
            self.uv_cache.clone(),
            self.gradle_home.clone(),
            self.temp_dir.clone(),
            self.go_build_cache.clone(),
            self.go_mod_cache.clone(),
        ];
        if let Some(sdk) = &self.android_sdk {
            roots.push(sdk.clone());
        }
        roots
            .into_iter()
            .filter_map(|p| std::fs::canonicalize(p).ok())
            .collect()
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
            // Hermetic: an empty fixture dir, never the real system root.
            system_root: Some(root.join("Windows")),
            go_build_cache: local.join("go-build"),
            go_mod_cache: root.join("go").join("pkg").join("mod"),
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
        Box::new(gocache::GoCacheDetector),
        Box::new(agent::AgentArtifactsDetector),
        Box::new(browser::BrowserCacheDetector),
        Box::new(docker::DockerDetector),
        Box::new(temp::TempDetector),
        Box::new(winupdate::WindowsUpdateDetector),
    ]
}

/// Run every detector, largest finding first.
pub fn scan_all(ctx: &Ctx) -> Vec<Finding> {
    scan_selected(ctx, &[])
}

/// Scan only requested detector IDs, or every detector when empty.
///
/// Detectors run in parallel (rayon): each detector is independent and
/// mostly I/O-bound, so 16 sequential walks become ~1 concurrent batch.
/// Results are re-sorted largest-first for stable output.
pub fn scan_selected(ctx: &Ctx, ids: &[String]) -> Vec<Finding> {
    use rayon::prelude::*;
    let mut out: Vec<Finding> = all_detectors()
        .into_par_iter()
        .filter(|d| {
            (ids.is_empty() || ids.iter().any(|id| id == d.id()))
                && (d.id() != "docker" || ctx.docker_enabled)
        })
        .flat_map(|d| d.scan(ctx))
        .collect();
    // Stable order: largest first, id+label tiebreak for determinism
    // across runs and thread schedules.
    out.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then(a.detector_id.cmp(&b.detector_id))
            .then(a.label.cmp(&b.label))
    });
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

/// Walk the context's project roots for marker files (e.g. `pubspec.yaml`),
/// returning the directories that contain one. Artifact/output dirs are
/// pruned by name; managed cache trees (package caches, runtimes, SDKs)
/// are pruned by anchored path prefix from [`Ctx::managed_roots`].
/// Prefix (not name) matching avoids false negatives: a real user project
/// under `…/Android/myapp` or `…/uv/myapp` is still found. Capped at 500
/// projects.
///
/// Prefix pruning matters: package caches ship test fixtures containing
/// markers (e.g. `Pub/Cache/hosted/.../build_runner-*/test/fixtures/
/// flutter_pkg/pubspec.yaml`) that are not user projects. Walking them
/// wastes minutes and emits false findings that overlap (and hide) the
/// real parent cache finding.
pub(crate) fn find_projects(ctx: &Ctx, marker: &str) -> Vec<PathBuf> {
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
    let managed: std::sync::Arc<Vec<PathBuf>> = std::sync::Arc::new(ctx.managed_roots());
    let mut projects = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in &ctx.project_roots {
        if !root.is_dir() || projects.len() >= 500 {
            continue;
        }
        // Canonicalize the walk root so prefix comparison uses the same
        // lexical form as the canonicalized managed roots. The root itself
        // is always honored; managed children are pruned — unless the root
        // itself sits inside a managed tree (explicit `--roots`/CWD scope,
        // including tempdirs in tests), in which case prefix pruning is
        // disabled for that walk and only name/links prune.
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
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
