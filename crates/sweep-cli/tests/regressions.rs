use assert_cmd::Command;
use predicates::prelude::*;
use std::{fs, path::Path};

fn isolated(root: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sweep").unwrap();
    cmd.current_dir(root);
    for variable in [
        "LOCALAPPDATA",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "PUB_CACHE",
        "ANDROID_SDK_ROOT",
        "ANDROID_HOME",
        "GRADLE_USER_HOME",
        "YARN_CACHE_FOLDER",
        "npm_config_cache",
        "TEMP",
        "TMP",
    ] {
        cmd.env(variable, root.join("empty-home"));
    }
    cmd.timeout(std::time::Duration::from_secs(15));
    cmd
}

#[test]
fn known_detector_with_no_findings_is_successful() {
    let tmp = tempfile::tempdir().unwrap();
    isolated(tmp.path())
        .args(["clean", "--id", "cargo", "--json", "--no-docker"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"removed\": []"));
}

#[test]
fn mixed_valid_and_unknown_ids_are_rejected_before_scanning() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("Cargo.toml"), "[workspace]").unwrap();
    fs::create_dir(tmp.path().join("target")).unwrap();
    fs::write(tmp.path().join("target/cache"), b"fixture").unwrap();
    isolated(tmp.path())
        .args([
            "clean",
            "--id",
            "cargo",
            "--id",
            "typo",
            "--json",
            "--no-docker",
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("unknown detector"));
    assert!(tmp.path().join("target/cache").exists());
}

#[test]
fn dry_run_then_execute_cleans_only_fixture_artifacts() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("项目 with spaces");
    fs::create_dir_all(project.join("target/debug")).unwrap();
    fs::write(project.join("Cargo.toml"), "[workspace]").unwrap();
    fs::write(project.join("source.rs"), b"keep source").unwrap();
    fs::write(project.join("target/debug/cache"), vec![0; 4096]).unwrap();
    let output = isolated(tmp.path())
        .args(["clean", "--id", "cargo", "--json", "--no-docker"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let preview: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(preview["dry_run"], true);
    assert_eq!(preview["freed_bytes"], 4096);
    assert!(project.join("target/debug/cache").exists());
    isolated(tmp.path())
        .args([
            "clean",
            "--id",
            "cargo",
            "--json",
            "--no-docker",
            "--execute",
            "--yes",
            "--permanent",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"freed_bytes\": 4096"));
    assert!(!project.join("target").exists());
    assert!(project.join("source.rs").exists());
}
