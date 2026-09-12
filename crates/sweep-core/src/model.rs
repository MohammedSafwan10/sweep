//! Shared data model. These types are the stable contract between
//! sweep-core, the CLI (`--json` output agents parse) and the future TUI.
//!
//! JSON stability rule: only *add* fields/values, never rename or remove,
//! until a major version bump. Agents pin on `schema_version`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Version of the JSON contract. Bumped on any breaking change.
pub const SCHEMA_VERSION: u32 = 1;

/// How safe it is to delete something.
/// Ordering matters: Safe < Caution < Danger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    /// Regenerable caches/build output (`pub-cache`, `build/`, `.next/`).
    Safe,
    /// Deletable but rebuild costs real time (`node_modules`, gradle `modules-2`).
    Caution,
    /// Data loss risk (docker volumes with DB data, anything unclassified).
    /// Never touched without explicit opt-in + `--force`.
    Danger,
}

impl Safety {
    /// Parse from CLI `--only safe|caution|all` style input.
    pub fn parse_level(s: &str) -> Option<IncludeLevel> {
        match s.to_ascii_lowercase().as_str() {
            "safe" => Some(IncludeLevel::Safe),
            "caution" => Some(IncludeLevel::Caution),
            "all" => Some(IncludeLevel::All),
            _ => None,
        }
    }
}

/// Which safety levels a clean run may include.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncludeLevel {
    Safe,
    Caution,
    All,
}

impl IncludeLevel {
    /// Danger always additionally requires `force`.
    pub fn allows(self, safety: Safety) -> bool {
        match self {
            IncludeLevel::Safe => safety == Safety::Safe,
            IncludeLevel::Caution => safety <= Safety::Caution,
            IncludeLevel::All => true,
        }
    }
}

/// Options for [`crate::scanner::scan_dir`].
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Max entries in the ranked report.
    pub top: usize,
    /// Hide entries smaller than this (bytes).
    pub min_bytes: u64,
    /// Individually list files at/above this size (bytes). Larger value = less memory.
    pub min_file_bytes: u64,
    /// Stay on the same filesystem as the scan root.
    pub same_filesystem: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            top: 30,
            min_bytes: 1024 * 1024,
            min_file_bytes: 10 * 1024 * 1024,
            same_filesystem: false,
        }
    }
}

/// One ranked entry in a scan report (directory or large file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub path: PathBuf,
    pub bytes: u64,
    /// Files contained (1 for a file entry itself).
    pub files: u64,
    /// Depth relative to scan root (root = 0).
    pub depth: usize,
    pub is_dir: bool,
}

/// Result of scanning one root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanReport {
    pub schema_version: u32,
    pub root: PathBuf,
    pub total_bytes: u64,
    pub total_files: u64,
    pub total_dirs: u64,
    /// Non-fatal problems (permission denied, ...). Capped; see `warnings_suppressed`.
    pub warnings: Vec<String>,
    pub warnings_suppressed: usize,
    /// Ranked largest-first, already filtered + truncated to `top`.
    pub entries: Vec<DirEntry>,
}

/// How a finding can be cleaned.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CleanAction {
    /// Delete this path (trash by default, permanent with flag).
    RemovePath { path: PathBuf },
    /// Requires running an external program (v1 cleaner reports it, does not run it).
    RunCommand {
        program: String,
        args: Vec<String>,
        description: String,
    },
    /// Human steps, e.g. Docker Desktop prune flow.
    Manual { instructions: String },
}

/// One cleanable thing found by a detector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Stable detector id, e.g. `"cargo-registry"`. Selectable via `--id`.
    pub detector_id: String,
    pub label: String,
    pub bytes: u64,
    pub safety: Safety,
    pub detail: String,
    pub action: CleanAction,
}

/// Options for [`crate::cleaner`] planning/execution.
#[derive(Debug, Clone)]
pub struct CleanOptions {
    /// false = dry-run (default): plan and report, delete nothing.
    pub execute: bool,
    /// true = move to Recycle Bin/Trash (default). false = permanent delete.
    pub to_trash: bool,
    pub include: IncludeLevel,
    /// Required to touch `Danger` items even with `IncludeLevel::All`.
    pub force_danger: bool,
}

impl Default for CleanOptions {
    fn default() -> Self {
        Self {
            execute: false,
            to_trash: true,
            include: IncludeLevel::Safe,
            force_danger: false,
        }
    }
}

/// One item of a clean plan with its disposition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedItem {
    pub finding: Finding,
    #[serde(flatten)]
    pub disposition: Disposition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum Disposition {
    Remove { via: String },
    Skipped { reason: String },
}

/// The receipt every clean run (dry or real) produces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanReceipt {
    pub schema_version: u32,
    pub dry_run: bool,
    /// Bytes that would be / were freed.
    pub freed_bytes: u64,
    pub removed: Vec<RemovedItem>,
    pub skipped: Vec<SkippedItem>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemovedItem {
    pub path: PathBuf,
    pub bytes: u64,
    /// "trash" or "permanent".
    pub via: String,
    pub safety: Safety,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedItem {
    pub label: String,
    pub safety: Safety,
    pub reason: String,
}
