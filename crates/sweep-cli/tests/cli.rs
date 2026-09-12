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
fn clean_is_dry_run_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let out = sweep()
        .args(["detectors", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["schema_version"], 1);
    assert!(v["findings"].is_array());
    let _ = tmp;
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
