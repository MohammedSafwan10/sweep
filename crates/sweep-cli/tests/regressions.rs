mod common;
use common::isolated;
use predicates::prelude::*;
use std::fs;

#[test]
fn human_confirmation_executes_only_the_displayed_findings() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::sync::mpsc;
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("Cargo.toml"), "[workspace]").unwrap();
    fs::create_dir(tmp.path().join("target")).unwrap();
    fs::write(tmp.path().join("target/cache"), b"fixture").unwrap();
    let mut command = common::native(tmp.path());
    let mut child = command
        .args([
            "clean",
            "--id",
            "cargo",
            "--no-docker",
            "--execute",
            "--permanent",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let line = line.unwrap();
            if line.contains("Would remove") {
                let _ = tx.send(());
            }
        }
    });
    if rx.recv_timeout(std::time::Duration::from_secs(15)).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("preview did not appear");
    }
    let added = tmp.path().join("new-project");
    fs::create_dir_all(added.join("target")).unwrap();
    fs::write(added.join("Cargo.toml"), "[workspace]").unwrap();
    fs::write(added.join("target/new-cache"), b"not in the preview").unwrap();
    child.stdin.take().unwrap().write_all(b"y\n").unwrap();
    assert!(child.wait().unwrap().success());
    reader.join().unwrap();
    assert!(!tmp.path().join("target").exists());
    assert!(added.join("target/new-cache").exists());
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
