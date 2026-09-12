//! OS temp dir: flags only the largest entries older than 7 days.
//!
//! Conservative by design: fresh temp files may belong to running
//! programs (editors, installers). Age + size + top-5 cap keeps this
//! strictly to forgotten junk like stale installer extractions.

use super::{Ctx, Detector};
use crate::model::{CleanAction, Finding, Safety};
use crate::scanner::size_of_dir;

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
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_symlink() {
                continue;
            }
            let old_enough = meta.modified().ok().zip(cutoff).is_some_and(|(m, c)| m < c);
            if !old_enough {
                continue;
            }
            let bytes = if meta.is_dir() {
                size_of_dir(&path).0
            } else {
                meta.len()
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
                safety: Safety::Safe,
                detail: format!("Untouched for over {MIN_AGE_DAYS} days inside the OS temp dir."),
                action: CleanAction::RemovePath { path },
            })
            .collect()
    }
}

fn short(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string()
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
