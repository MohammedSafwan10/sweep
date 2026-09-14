//! AI coding-agent artifacts: task work outputs and old session logs.
//!
//! Agents (Codex and friends) keep per-task build outputs and rolling
//! session transcripts under the home directory. These grow silently —
//! a single mobile build artifact dir measured 4.9 GiB — and no previous
//! detector covered them. All findings are CAUTION: re-running the task
//! regenerates them, but that can take minutes.

use super::{dir_finding, Ctx, Detector};
use crate::model::{Finding, Safety};
use std::time::{Duration, SystemTime};

pub struct AgentArtifactsDetector;

/// Ignore subdirs below this: session shards and tiny state files are
/// noise, not cleanup targets.
const MIN_BYTES: u64 = 10 * 1024 * 1024;
/// Task outputs must be untouched for this long: a live task writing
/// mid-run must never match.
const TASK_QUIET_AGE: Duration = Duration::from_secs(7 * 86400);
/// Session transcripts older than this are no longer active.
const SESSION_MAX_AGE: Duration = Duration::from_secs(30 * 86400);

impl Detector for AgentArtifactsDetector {
    fn id(&self) -> &'static str {
        "agent-artifacts"
    }
    fn label(&self) -> &'static str {
        "AI agent task artifacts"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        let codex = ctx.home.join(".codex");
        // Task build outputs: one finding per task dir (they are
        // independent; deleting one task never affects another).
        if let Ok(entries) = std::fs::read_dir(codex.join("work_artifacts")) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if !path.is_dir() || crate::scanner::is_link_or_reparse(&path) {
                    continue;
                }
                // Live tasks write here: only flag whole trees quiet for a
                // full week (see tree_older_than).
                if !tree_older_than(&path, TASK_QUIET_AGE) {
                    continue;
                }
                if let Some(f) = sized(
                    self.id(),
                    format!(
                        "agent task output ({})",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    &path,
                    "Task build output; re-running the agent task regenerates it.",
                ) {
                    out.push(f);
                }
            }
        }
        // Session transcripts/logs older than 30 days, per month shard.
        if let Ok(years) = std::fs::read_dir(codex.join("sessions")) {
            for year in years.filter_map(|e| e.ok()) {
                let year = year.path();
                if !year.is_dir() {
                    continue;
                }
                if let Ok(months) = std::fs::read_dir(&year) {
                    for month in months.filter_map(|e| e.ok()) {
                        let path = month.path();
                        if !path.is_dir() || crate::scanner::is_link_or_reparse(&path) {
                            continue;
                        }
                        // Dir mtime alone is not enough: it only bumps on
                        // direct-child changes, so a month shard with a
                        // freshly appended transcript would look old. Check
                        // the whole tree.
                        if !tree_older_than(&path, SESSION_MAX_AGE) {
                            continue;
                        }
                        if let Some(f) = sized(
                            self.id(),
                            format!(
                                "old agent sessions ({})",
                                path.file_name().unwrap_or_default().to_string_lossy()
                            ),
                            &path,
                            "Session transcripts older than 30 days; the agent keeps running without them.",
                        ) {
                            out.push(f);
                        }
                    }
                }
            }
        }
        out
    }
}

/// A finding for `path` when it holds at least MIN_BYTES, else None.
fn sized(
    detector_id: &'static str,
    label: String,
    path: &std::path::Path,
    detail: &str,
) -> Option<Finding> {
    let finding = dir_finding(detector_id, label, path, Safety::Caution, detail)?;
    (finding.bytes >= MIN_BYTES).then_some(finding)
}

/// True when every entry in the tree (files and dirs) is older than `age`.
/// Links, unreadable metadata, or any fresh entry mean "not quiet".
/// Mirrors `temp::stale_bytes` semantics: conservative by construction.
fn tree_older_than(path: &std::path::Path, age: Duration) -> bool {
    let Some(cutoff) = SystemTime::now().checked_sub(age) else {
        return false;
    };
    let walker = ignore::WalkBuilder::new(path)
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .follow_links(false)
        .build();
    for entry in walker {
        let Ok(entry) = entry else { return false };
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            return false;
        };
        if crate::scanner::metadata_is_link_or_reparse(&meta) {
            return false;
        }
        match meta.modified() {
            Ok(modified) if modified < cutoff => {}
            _ => return false,
        }
    }
    true
}

/// Execute-time recheck for the cleaner: the tree must have been quiet
/// for a week at delete time too (catches tasks started after planning).
pub(crate) fn is_quiet(path: &std::path::Path) -> bool {
    tree_older_than(path, TASK_QUIET_AGE)
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use std::fs;

    fn aged(path: &std::path::Path) {
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 86400);
        let ft = filetime::FileTime::from_system_time(old);
        // Whole-tree aging: the freshness check looks at every entry.
        for entry in walkdir_simple(path) {
            let _ = filetime::set_file_mtime(&entry, ft);
        }
        let _ = filetime::set_file_mtime(path, ft);
    }

    fn walkdir_simple(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path.clone());
                    }
                    out.push(path);
                }
            }
        }
        out
    }

    #[test]
    fn flags_task_outputs_and_old_sessions_only() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let task = ctx.home.join(".codex/work_artifacts/noor_build");
        fs::create_dir_all(&task).unwrap();
        fs::write(task.join("app.apk"), vec![0u8; 11 * 1024 * 1024]).unwrap();
        aged(&task);
        let tiny = ctx.home.join(".codex/work_artifacts/tiny");
        fs::create_dir_all(&tiny).unwrap();
        fs::write(tiny.join("log"), vec![0u8; 10]).unwrap();
        let old_session = ctx.home.join(".codex/sessions/2026/01");
        fs::create_dir_all(&old_session).unwrap();
        fs::write(
            old_session.join("rollout.jsonl"),
            vec![0u8; 11 * 1024 * 1024],
        )
        .unwrap();
        aged(&old_session);
        let fresh_session = ctx.home.join(".codex/sessions/2026/12");
        fs::create_dir_all(&fresh_session).unwrap();
        fs::write(
            fresh_session.join("rollout.jsonl"),
            vec![0u8; 11 * 1024 * 1024],
        )
        .unwrap();
        // Live task output (fresh tree) must never match, even when big.
        let live = ctx.home.join(".codex/work_artifacts/live_run");
        fs::create_dir_all(&live).unwrap();
        fs::write(live.join("app.apk"), vec![0u8; 11 * 1024 * 1024]).unwrap();
        // Recently appended transcript inside an old month shard: the shard
        // must not match on dir mtime alone.
        let sneaky = ctx.home.join(".codex/sessions/2025/06");
        fs::create_dir_all(&sneaky).unwrap();
        fs::write(sneaky.join("old.jsonl"), vec![0u8; 11 * 1024 * 1024]).unwrap();
        aged(&sneaky);
        fs::write(sneaky.join("fresh.jsonl"), vec![0u8; 11 * 1024 * 1024]).unwrap();

        let findings = super::AgentArtifactsDetector.scan(&ctx);
        assert_eq!(findings.len(), 2);
        assert!(findings
            .iter()
            .all(|f| f.safety == crate::model::Safety::Caution));
        // Execute-time gate agrees with scan-time gates.
        assert!(super::is_quiet(&task));
        assert!(!super::is_quiet(&live));
    }
}
