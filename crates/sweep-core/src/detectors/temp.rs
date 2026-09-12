//! OS temp dir: flags only the largest entries older than 7 days.
//!
//! Conservative by design: fresh temp files may belong to running
//! programs (editors, installers). Age + size + top-5 cap keeps this
//! strictly to forgotten junk like stale installer extractions.

use super::{Ctx, Detector};
use crate::model::{CleanAction, Finding, Safety};

pub struct TempDetector;

/// Entries must be at least this old and this big to be flagged.
const MIN_AGE_DAYS: u64 = 7;
const MIN_BYTES: u64 = 1024 * 1024;
const MAX_FINDINGS: usize = 5;

impl Detector for TempDetector {
    fn id(&self) -> &'static str {
        "temp"
    }
    fn label(&self) -> &'static str {
        "Stale temp files"
    }

    fn scan(&self, ctx: &Ctx) -> Vec<Finding> {
        let cutoff = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(MIN_AGE_DAYS * 86400));
        let Ok(entries) = std::fs::read_dir(&ctx.temp_dir) else {
            return Vec::new();
        };
        let mut scored: Vec<(u64, std::path::PathBuf, bool)> = Vec::new();
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            // file_type() is free (no extra syscall); metadata() follows
            // links, so the dead `meta.is_symlink()` check it replaces
            // could never fire. Links are refused before sizing.
            if crate::scanner::is_link_or_reparse(&path) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let old_enough = meta.modified().ok().zip(cutoff).is_some_and(|(m, c)| m < c);
            if !old_enough {
                continue;
            }
            let Some(bytes) = cutoff.and_then(|cutoff| stale_bytes(&path, cutoff)) else {
                continue;
            };
            if bytes >= MIN_BYTES {
                scored.push((bytes, path, meta.is_dir()));
            }
        }
        scored.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        scored.truncate(MAX_FINDINGS);
        scored
            .into_iter()
            .map(|(bytes, path, _)| Finding {
                detector_id: self.id().to_string(),
                label: format!("stale temp: {}", short(&path)),
                bytes,
                safety: Safety::Caution,
                detail: format!("All entries last modified over {MIN_AGE_DAYS} days ago. Age alone does not prove a file is unused; review before deleting."),
                action: CleanAction::RemovePath { path },
            })
            .collect()
    }
}

fn stale_bytes(path: &std::path::Path, cutoff: std::time::SystemTime) -> Option<u64> {
    let found_link = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let link_flag = found_link.clone();
    let walker = ignore::WalkBuilder::new(path)
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .follow_links(false)
        .filter_entry(move |entry| {
            if crate::scanner::is_link_or_reparse(entry.path()) {
                link_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                return false;
            }
            true
        })
        .build();
    let mut bytes = 0u64;
    for entry in walker {
        let entry = entry.ok()?;
        let meta = std::fs::symlink_metadata(entry.path()).ok()?;
        if crate::scanner::metadata_is_link_or_reparse(&meta) || meta.modified().ok()? >= cutoff {
            return None;
        }
        if meta.is_file() {
            bytes = bytes.checked_add(meta.len())?;
        }
    }
    (!found_link.load(std::sync::atomic::Ordering::Relaxed)).then_some(bytes)
}

pub(crate) fn is_stale(path: &std::path::Path) -> bool {
    std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(MIN_AGE_DAYS * 86400))
        .and_then(|cutoff| stale_bytes(path, cutoff))
        .is_some()
}

fn short(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::super::{Ctx, Detector};
    use std::fs;

    #[test]
    fn flags_old_large_entries_only() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        fs::create_dir_all(&ctx.temp_dir).unwrap();
        // Old + big -> flagged. Fresh + big -> kept. Old + tiny -> kept.
        let old_big = ctx.temp_dir.join("old_big");
        fs::create_dir_all(&old_big).unwrap();
        fs::write(old_big.join("f"), vec![0u8; 2 * 1024 * 1024]).unwrap();
        let aged = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86400);
        filetime::set_file_mtime(
            old_big.join("f"),
            filetime::FileTime::from_system_time(aged),
        )
        .unwrap();
        filetime::set_file_mtime(&old_big, filetime::FileTime::from_system_time(aged)).unwrap();
        let fresh = ctx.temp_dir.join("fresh_big.bin");
        fs::write(&fresh, vec![0u8; 2 * 1024 * 1024]).unwrap();
        let tiny = ctx.temp_dir.join("tiny.bin");
        fs::write(&tiny, vec![0u8; 10]).unwrap();
        filetime::set_file_mtime(&tiny, filetime::FileTime::from_system_time(aged)).unwrap();

        let findings = super::TempDetector.scan(&ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].label.contains("old_big"));
        assert_eq!(findings[0].bytes, 2 * 1024 * 1024);
    }
}
