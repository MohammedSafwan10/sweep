use std::{fs, path::Path, sync::mpsc, time::Duration};
use sweep_core::{
    cleaner,
    detectors::{self, Ctx, Detector},
    model::{CleanAction, CleanOptions, Disposition, Finding, PlannedItem, Safety, ScanOptions},
    scanner,
};

fn write(path: &Path, size: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![42; size]).unwrap();
}

fn finding(path: &Path, bytes: u64) -> Finding {
    Finding {
        detector_id: "fixture".into(),
        label: "fixture".into(),
        bytes,
        safety: Safety::Safe,
        detail: String::new(),
        action: CleanAction::RemovePath { path: path.into() },
    }
}

fn permanent() -> CleanOptions {
    CleanOptions {
        execute: true,
        to_trash: false,
        ..CleanOptions::default()
    }
}

#[test]
fn unconsumed_progress_never_blocks_scan() {
    let tmp = tempfile::tempdir().unwrap();
    write(&tmp.path().join("file"), 8);
    for capacity in [0, 1] {
        let (tx, _rx) = mpsc::sync_channel(capacity);
        let (done_tx, done_rx) = mpsc::channel();
        let root = tmp.path().to_path_buf();
        let worker = std::thread::spawn(move || {
            done_tx
                .send(scanner::scan_dir(&root, &ScanOptions::default(), Some(tx)))
                .unwrap();
        });
        assert_eq!(
            done_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("scanner blocked on progress")
                .unwrap()
                .total_bytes,
            8
        );
        worker.join().unwrap();
    }
}

#[test]
fn sizes_reject_numeric_overflow() {
    for value in ["18446744073709551616", "999999999999999999999TB", "999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999TB"] {
        assert!(scanner::parse_size(value).is_err(), "accepted {value}");
    }
}

#[test]
fn duplicates_and_parent_findings_do_not_inflate_preview() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("cache");
    let child = cache.join("sub");
    write(&child.join("data"), 32);
    let report = cleaner::clean(
        &[
            finding(&cache, 32),
            finding(&child, 32),
            finding(&child, 32),
        ],
        &CleanOptions::default(),
    );
    assert_eq!(report.freed_bytes, 32);
    assert_eq!(report.removed.len(), 1);
    assert!(child.join("data").exists());
}

#[test]
fn skipped_caution_child_is_not_deleted_by_safe_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let child = tmp.path().join("cache/dependencies");
    write(&child.join("keep"), 32);
    let mut caution = finding(&child, 32);
    caution.safety = Safety::Caution;
    let report = cleaner::clean(
        &[finding(child.parent().unwrap(), 32), caution],
        &permanent(),
    );
    assert!(report.removed.is_empty());
    assert!(child.join("keep").exists());
}

#[test]
fn execute_rejects_unknown_method_and_unapproved_danger() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("keep");
    write(&file, 16);
    for (via, safety) in [("typo", Safety::Safe), ("permanent", Safety::Danger)] {
        let mut item = finding(&file, 16);
        item.safety = safety;
        let plan = cleaner::CleanPlan {
            items: vec![PlannedItem {
                finding: item,
                disposition: Disposition::Remove { via: via.into() },
            }],
            total_bytes: 16,
        };
        let receipt = cleaner::execute(&plan, &permanent());
        assert_eq!(receipt.errors.len(), 1);
        assert!(file.exists());
    }
}

#[test]
fn current_directory_is_never_a_clean_target() {
    let plan = cleaner::plan(&[finding(Path::new("."), 1)], &CleanOptions::default());
    assert_eq!(plan.total_bytes, 0);
    assert!(matches!(
        plan.items[0].disposition,
        Disposition::Skipped { .. }
    ));
}

#[test]
fn overlapping_roots_find_each_project_once() {
    let tmp = tempfile::tempdir().unwrap();
    let mut ctx = Ctx::for_tests(tmp.path());
    let project = ctx.project_roots[0].join("app");
    write(&project.join("Cargo.toml"), 10);
    write(&project.join("target/debug/cache"), 32);
    ctx.project_roots.push(project);
    assert_eq!(detectors::cargo::CargoDetector.scan(&ctx).len(), 1);
}

#[test]
fn gradle_preserves_unknown_distributions_and_both_newest_variants() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    for name in [
        "gradle-7.0-bin",
        "gradle-8.0-all",
        "gradle-8.0-bin",
        "gradle-6.0-personal",
        "gradle-9.0-rc-1-bin",
        "gradle-5.0",
    ] {
        write(
            &ctx.gradle_home
                .join("wrapper/dists")
                .join(name)
                .join("file"),
            8,
        );
    }
    let findings = detectors::gradle::GradleDetector.scan(&ctx);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].label.contains("7.0-bin"));
}

#[test]
fn old_temp_directory_with_fresh_descendants_is_preserved() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    let folder = ctx.temp_dir.join("active");
    write(&folder.join("nested/current"), 2 * 1024 * 1024);
    let old = filetime::FileTime::from_system_time(
        std::time::SystemTime::now() - Duration::from_secs(30 * 86400),
    );
    filetime::set_file_mtime(&folder, old).unwrap();
    assert!(detectors::temp::TempDetector.scan(&ctx).is_empty());
    filetime::set_file_mtime(folder.join("nested/current"), old).unwrap();
    filetime::set_file_mtime(folder.join("nested"), old).unwrap();
    let findings = detectors::temp::TempDetector.scan(&ctx);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].safety, Safety::Caution);
    assert!(cleaner::clean(&findings, &permanent()).removed.is_empty());
}

#[test]
fn temp_revalidation_preserves_files_modified_after_the_preview() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    let file = ctx.temp_dir.join("old-cache");
    write(&file, 2 * 1024 * 1024);
    let old = filetime::FileTime::from_system_time(
        std::time::SystemTime::now() - Duration::from_secs(30 * 86400),
    );
    filetime::set_file_mtime(&file, old).unwrap();
    let findings = detectors::temp::TempDetector.scan(&ctx);
    let options = CleanOptions {
        include: sweep_core::model::IncludeLevel::Caution,
        ..permanent()
    };
    let plan = cleaner::plan(&findings, &options);
    fs::write(&file, b"active work").unwrap();
    let receipt = cleaner::execute(&plan, &options);
    assert_eq!(receipt.errors.len(), 1);
    assert_eq!(fs::read(&file).unwrap(), b"active work");
}

#[test]
fn android_preserves_unknown_component_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    let sdk = ctx.android_sdk.as_ref().unwrap();
    for path in [
        "ndk/personal-backup",
        "platforms/notes",
        "build-tools/custom",
        "sources/android-",
        "cmake/3.22.1-backup",
    ] {
        write(&sdk.join(path).join("keep"), 16);
    }
    assert!(detectors::android::AndroidDetector.scan(&ctx).is_empty());
}

#[test]
fn yarn_cleanup_preserves_sibling_configuration_and_binaries() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    write(&ctx.yarn_cache.join("v6/package"), 32);
    let binary = ctx.yarn_cache.parent().unwrap().join("bin/yarn.cmd");
    write(&binary, 16);
    let findings = detectors::jscaches::JsCachesDetector.scan(&ctx);
    let receipt = cleaner::clean(&findings, &permanent());
    assert_eq!(receipt.freed_bytes, 32);
    assert!(binary.exists());
}

#[test]
fn long_unicode_paths_and_readonly_trees_can_be_cleaned() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("用户 cache café");
    let mut nested = cache.clone();
    for _ in 0..12 {
        nested.push("long-directory-component");
    }
    let file = nested.join("build.bin");
    write(&file, 64);
    let mut permissions = fs::metadata(&file).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&file, permissions).unwrap();
    let report = scanner::scan_dir(&cache, &ScanOptions::default(), None).unwrap();
    assert_eq!(report.total_bytes, 64);
    let receipt = cleaner::clean(&[finding(&cache, 64)], &permanent());
    assert!(receipt.errors.is_empty(), "{:?}", receipt.errors);
    assert_eq!(receipt.freed_bytes, 64);
    assert!(!cache.exists());
}

#[test]
fn vanished_item_does_not_stop_other_deletions() {
    let tmp = tempfile::tempdir().unwrap();
    let first = tmp.path().join("first");
    let second = tmp.path().join("second");
    write(&first, 8);
    write(&second, 16);
    let plan = cleaner::plan(&[finding(&first, 8), finding(&second, 16)], &permanent());
    fs::remove_file(first).unwrap();
    let receipt = cleaner::execute(&plan, &permanent());
    assert_eq!(receipt.skipped.len(), 1);
    assert_eq!(receipt.freed_bytes, 16);
    assert!(receipt.errors.is_empty());
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    fn junction(link: &Path, target: &Path, sandbox: &Path) {
        assert!(link.is_absolute() && link.starts_with(sandbox));
        assert!(target.is_absolute() && target.starts_with(sandbox));
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "New-Item -ItemType Junction -Path $env:SWEEP_TEST_LINK -Target $env:SWEEP_TEST_TARGET -ErrorAction Stop | Out-Null"])
            .env("SWEEP_TEST_LINK", link).env("SWEEP_TEST_TARGET", target).status().unwrap();
        assert!(status.success(), "could not create junction fixture");
    }

    #[test]
    fn junctions_are_pruned_and_ancestor_swaps_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let scan = root.join("scan");
        let external = root.join("external");
        write(&scan.join("local"), 8);
        write(&external.join("cache/keep"), 64);
        let link = scan.join("link");
        junction(&link, &external, root);
        let report = scanner::scan_dir(&scan, &ScanOptions::default(), None).unwrap();
        assert_eq!(report.total_bytes, 8);
        assert_eq!(scanner::size_of_dir(&scan), (8, 1));
        assert_eq!(scanner::size_of_dir(&link), (0, 0));
        assert!(
            cleaner::clean(&[finding(&link.join("cache"), 64)], &permanent())
                .removed
                .is_empty()
        );
        fs::remove_dir(&link).unwrap();

        let parent = root.join("parent");
        write(&parent.join("cache/data"), 16);
        let plan = cleaner::plan(&[finding(&parent.join("cache"), 16)], &permanent());
        fs::rename(&parent, root.join("original-parent")).unwrap();
        junction(&parent, &external, root);
        let receipt = cleaner::execute(&plan, &permanent());
        assert!(!receipt.errors.is_empty());
        assert!(external.join("cache/keep").exists());
        fs::remove_dir(parent).unwrap();
    }

    #[test]
    fn locked_file_reports_error_and_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let locked = tmp.path().join("locked");
        let free = tmp.path().join("free");
        write(&locked, 8);
        write(&free, 16);
        let plan = cleaner::plan(&[finding(&locked, 8), finding(&free, 16)], &permanent());
        let handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)
            .unwrap();
        let receipt = cleaner::execute(&plan, &permanent());
        assert_eq!(receipt.errors.len(), 1);
        assert_eq!(receipt.freed_bytes, 16);
        assert!(locked.exists());
        assert!(!free.exists());
        drop(handle);
    }

    #[test]
    fn recycle_bin_accepts_disposable_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("sweep-trash-test");
        write(&cache.join("fixture-only"), 8);
        let opts = CleanOptions {
            execute: true,
            ..CleanOptions::default()
        };
        let receipt = cleaner::clean(&[finding(&cache, 8)], &opts);
        assert!(receipt.errors.is_empty(), "{:?}", receipt.errors);
        assert_eq!(receipt.removed[0].via, "trash");
        assert!(!cache.exists());
    }
}
