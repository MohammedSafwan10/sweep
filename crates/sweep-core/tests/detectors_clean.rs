//! Integration tests for sweep-core: detectors + cleaner against a fake home.

use std::fs;
use sweep_core::cleaner;
use sweep_core::detectors::Ctx;
use sweep_core::model::{CleanAction, CleanOptions, IncludeLevel};

fn build_fake_home() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    // cargo registry
    let src = ctx
        .cargo_home
        .join("registry")
        .join("src")
        .join("serde-1.0");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("lib.rs"), vec![0u8; 100]).unwrap();
    // pub hosted
    let hosted = ctx.pub_cache.join("hosted").join("pub.dev");
    fs::create_dir_all(&hosted).unwrap();
    fs::write(hosted.join("a.tar"), vec![0u8; 200]).unwrap();
    // npm cache
    fs::create_dir_all(&ctx.npm_cache).unwrap();
    fs::write(ctx.npm_cache.join("x"), vec![0u8; 50]).unwrap();
    // gradle: one old + one new dist
    for (d, n) in [("gradle-8.11.1-all", 10), ("gradle-9.3.1-all", 20)] {
        let p = ctx.gradle_home.join("wrapper").join("dists").join(d);
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("f"), vec![0u8; n]).unwrap();
    }
    // flutter project with build/
    let app = ctx.project_roots[0].join("myapp");
    fs::create_dir_all(app.join("build")).unwrap();
    fs::write(app.join("pubspec.yaml"), "name: myapp").unwrap();
    fs::write(app.join("build").join("o"), vec![0u8; 300]).unwrap();
    tmp
}

#[test]
fn full_detector_sweep_finds_expected_bytes() {
    let tmp = build_fake_home();
    let ctx = Ctx::for_tests(tmp.path());
    let findings = sweep_core::detectors::scan_all(&ctx);
    // cargo src 100, pub hosted 200, npm 50, gradle old dist 10, flutter build 300
    let total: u64 = findings.iter().map(|f| f.bytes).sum();
    assert_eq!(total, 660, "findings: {findings:#?}");
    assert!(findings
        .iter()
        .all(|f| matches!(f.action, CleanAction::RemovePath { .. })));
    assert!(findings.windows(2).all(|w| w[0].bytes >= w[1].bytes));
}

#[test]
fn dry_run_clean_receipt_is_exact() {
    let tmp = build_fake_home();
    let ctx = Ctx::for_tests(tmp.path());
    let findings = sweep_core::detectors::scan_all(&ctx);
    let receipt = cleaner::clean(&findings, &CleanOptions::default());
    assert!(receipt.dry_run);
    assert_eq!(receipt.freed_bytes, 660);
    assert_eq!(receipt.removed.len(), findings.len());
    assert!(receipt.errors.is_empty());
    // Nothing gone: cargo src still there.
    assert!(ctx
        .cargo_home
        .join("registry")
        .join("src")
        .join("serde-1.0")
        .exists());
}

#[test]
fn execute_clean_removes_only_included_levels() {
    let tmp = build_fake_home();
    let ctx = Ctx::for_tests(tmp.path());
    let findings = sweep_core::detectors::scan_all(&ctx);
    let opts = CleanOptions {
        execute: true,
        to_trash: false,
        include: IncludeLevel::Safe,
        force_danger: false,
    };
    let receipt = cleaner::clean(&findings, &opts);
    assert!(!receipt.dry_run);
    assert!(receipt.errors.is_empty());
    assert!(!ctx.cargo_home.join("registry").join("src").exists());
    assert!(!ctx.project_roots[0].join("myapp").join("build").exists());
}
