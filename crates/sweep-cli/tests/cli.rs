//! CLI integration tests: JSON contract agents rely on.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;

fn sweep() -> Command {
    Command::cargo_bin("sweep").unwrap()
}

fn fixture_tree() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("big")).unwrap();
    fs::write(tmp.path().join("big").join("f"), vec![0u8; 2048]).unwrap();
    fs::create_dir_all(tmp.path().join("small")).unwrap();
    fs::write(tmp.path().join("small").join("f"), vec![0u8; 16]).unwrap();
    tmp
}

#[test]
fn scan_json_contract() {
    let tmp = fixture_tree();
    let out = sweep()
        .args([
            "scan",
            &tmp.path().display().to_string(),
            "--json",
            "--min-size",
            "1KB",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["total_bytes"], 2064);
    assert_eq!(v["total_files"], 2);
    let entries = v["entries"].as_array().unwrap();
    assert!(!entries.is_empty());
    assert!(entries[0]["bytes"].as_u64().unwrap() >= entries[1]["bytes"].as_u64().unwrap());
    assert!(entries.iter().all(|e| e["bytes"].as_u64().unwrap() >= 1024));
}

#[test]
fn scan_human_output_shows_ranked_table() {
    let tmp = fixture_tree();
    sweep()
        .args(["scan", &tmp.path().display().to_string(), "--min-size", "0"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2.0 KiB"))
        .stdout(predicate::str::contains("big"));
}

#[test]
fn scan_missing_path_fails_with_exit_1() {
    sweep()
        .args(["scan", "C:/definitely/not/here/sweep-xyz"])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn detectors_json_contract_smoke() {
    // Machine-dependent (real home dir): only the envelope is asserted.
    let out = sweep()
        .args(["detectors", "--json", "--no-docker"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["schema_version"], 1);
    assert!(v["findings"].is_array());
    assert!(v["total_bytes"].as_u64().is_some());
}

#[test]
fn clean_dry_run_receipt_is_exact_and_deletes_nothing() {
    // Hermetic: a fake Flutter project under --roots, scoped by --id so no
    // real-home detector can leak into the assertion.
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("myapp");
    fs::create_dir_all(app.join("build")).unwrap();
    fs::write(app.join("pubspec.yaml"), "name: myapp").unwrap();
    fs::write(app.join("build").join("out"), vec![0u8; 1234]).unwrap();

    let out = sweep()
        .args([
            "clean",
            "--json",
            "--roots",
            &app.display().to_string(),
            "--no-docker",
            "--id",
            "flutter-build",
        ])
        .assert()
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["freed_bytes"], 1234);
    assert_eq!(v["removed"].as_array().unwrap().len(), 1);
    assert_eq!(v["removed"][0]["bytes"], 1234);
    assert_eq!(v["removed"][0]["via"], "trash");
    assert_eq!(v["removed"][0]["safety"], "safe");
    assert!(v["errors"].as_array().unwrap().is_empty());
    // Dry run: build output still on disk.
    assert!(app.join("build").join("out").exists());
}

#[test]
fn json_execute_requires_yes() {
    sweep()
        .args(["clean", "--json", "--execute"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("--yes"));
}

#[test]
fn bad_only_value_is_rejected_with_exit_1() {
    sweep()
        .args(["clean", "--only", "everything"])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn invalid_roots_dir_is_rejected() {
    sweep()
        .args(["detectors", "--roots", "C:/definitely/not/here/sweep-xyz"])
        .assert()
        .failure()
        .code(1);
}
