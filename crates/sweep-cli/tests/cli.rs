//! CLI integration tests: JSON contract agents rely on.

mod common;
use assert_cmd::Command;
use common::isolated;
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
fn bin_rejects_invalid_drives_and_unconfirmed_json_execution() {
    sweep().args(["bin", "--drive", "C,D"]).assert().failure();
    sweep()
        .args(["bin", "--drive", "C", "--json", "--execute"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("without --yes"));
}

#[cfg(windows)]
#[test]
fn bin_json_dry_run_reports_without_emptying() {
    let out = sweep()
        .args(["bin", "--drive", "C", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["dry_run"], true);
    assert_eq!(value["drives"][0]["drive"], "C");
    assert!(value["drives"][0]["before"]["items"].is_number());
    assert!(value["drives"][0]["after"].is_null());
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
    assert_eq!(v["complete"], true);
    assert!(v["volume"]["total_bytes"].as_u64().unwrap() > 0);
    assert!(v["allocated_bytes"].is_null());
    assert_eq!(v["issues"]["permission_denied"], 0);
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
        .stdout(predicate::str::contains("complete: 2.0 KiB logical bytes"))
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
    let tmp = tempfile::tempdir().unwrap();
    let out = isolated(tmp.path())
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
    // All global cache locations and the project root are fixture paths.
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("myapp");
    fs::create_dir_all(app.join("build")).unwrap();
    fs::write(app.join("pubspec.yaml"), "name: myapp").unwrap();
    fs::write(app.join("build").join("out"), vec![0u8; 1234]).unwrap();

    let out = isolated(tmp.path())
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

#[test]
fn allocated_scan_adds_a_separate_size_without_changing_logical_total() {
    let tmp = fixture_tree();
    let out = sweep()
        .args([
            "scan",
            &tmp.path().display().to_string(),
            "--allocated",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["total_bytes"], 2064);
    assert!(report["allocated_bytes"].as_u64().is_some());
}

#[test]
fn multiple_roots_after_one_flag_are_scanned() {
    let tmp = tempfile::tempdir().unwrap();
    let first = tmp.path().join("first");
    let second = tmp.path().join("second");
    for root in [&first, &second] {
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join("pubspec.yaml"), "name: fixture").unwrap();
        fs::write(root.join("build").join("out"), vec![0u8; 1024]).unwrap();
    }

    let out = isolated(tmp.path())
        .args([
            "detectors",
            "--json",
            "--roots",
            &first.display().to_string(),
            &second.display().to_string(),
            "--no-docker",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    let builds: Vec<_> = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["detector_id"] == "flutter-build")
        .collect();
    assert_eq!(builds.len(), 2);
    let recommendations = v["recommendations"].as_array().unwrap();
    assert!(
        recommendations
            .iter()
            .filter(|recommendation| {
                recommendation["safety"] == "safe"
                    && recommendation["label"]
                        .as_str()
                        .is_some_and(|label| label.contains("flutter build"))
            })
            .count()
            >= 2
    );
}

#[test]
fn review_reports_unclassified_paths_without_cleanup_actions() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    fs::create_dir_all(project.join("build")).unwrap();
    fs::create_dir_all(project.join("recordings")).unwrap();
    fs::write(project.join("pubspec.yaml"), "name: fixture").unwrap();
    fs::write(project.join("build/out"), vec![0u8; 1024]).unwrap();
    fs::write(project.join("recordings/keep"), vec![0u8; 2048]).unwrap();

    let out = isolated(tmp.path())
        .args([
            "review",
            &project.display().to_string(),
            "--min-size",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["review_only"], true);
    let paths: Vec<_> = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["path"].as_str())
        .collect();
    assert!(paths.iter().any(|path| path.ends_with("recordings")));
    assert!(!paths.iter().any(|path| path.ends_with("build")));
    assert!(project.join("recordings/keep").exists());
}
