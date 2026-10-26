mod common;

use common::isolated;
use std::fs;
use std::path::Path;

fn project(root: &Path) {
    fs::create_dir_all(root.join("target")).unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]").unwrap();
    fs::write(root.join("target/cache"), b"reviewed").unwrap();
}

fn save(root: &Path) -> String {
    let result = isolated(root)
        .args([
            "clean",
            "--id",
            "cargo",
            "--no-docker",
            "--permanent",
            "--save-plan",
            "plan.json",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(report["dry_run"], true);
    report["plan_id"].as_str().unwrap().to_string()
}

#[test]
fn agent_executes_saved_targets_without_rediscovering_new_projects() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let id = save(tmp.path());
    project(&tmp.path().join("new-project"));
    let result = isolated(tmp.path())
        .args([
            "clean",
            "--from-plan",
            "plan.json",
            "--execute",
            "--permanent",
            "--yes",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(report["plan_id"], id);
    assert_eq!(report["dry_run"], false);
    assert!(
        report["execution_timings"]["recheck_worker_seconds"]
            .as_f64()
            .unwrap()
            >= 0.0
    );
    assert!(
        report["execution_timings"]["deletion_worker_seconds"]
            .as_f64()
            .unwrap()
            >= 0.0
    );
    assert_eq!(report["removed"].as_array().unwrap().len(), 1);
    assert!(!tmp.path().join("target").exists());
    assert!(tmp.path().join("new-project/target/cache").exists());
}

#[test]
fn saved_target_changes_return_error_and_preserve_the_whole_target() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    save(tmp.path());
    fs::write(tmp.path().join("target/new-file"), b"not approved").unwrap();
    isolated(tmp.path())
        .args([
            "clean",
            "--from-plan",
            "plan.json",
            "--execute",
            "--permanent",
            "--yes",
            "--json",
        ])
        .assert()
        .code(2)
        .stdout(predicates::str::contains("changed since preview"));
    assert!(tmp.path().join("target/cache").exists());
    assert!(tmp.path().join("target/new-file").exists());
}

#[test]
fn progress_uses_stderr_and_json_includes_stage_timings() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let output = isolated(tmp.path())
        .args([
            "clean",
            "--id",
            "cargo",
            "--no-docker",
            "--json",
            "--progress",
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["dry_run"], true);
    for stage in [
        "discovery_seconds",
        "planning_seconds",
        "execution_seconds",
        "total_seconds",
    ] {
        assert!(report["timings"][stage].as_f64().unwrap() >= 0.0);
    }
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Discovering"));
    assert!(stderr.contains("Fingerprinting preview"));
    assert!(stderr.contains("1 files checked"));
    assert!(tmp.path().join("target/cache").exists());
}

#[test]
fn version_one_plans_request_fresh_review() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    save(tmp.path());
    let path = tmp.path().join("plan.json");
    let mut saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["plan_schema_version"], 2);
    saved["plan_schema_version"] = 1.into();
    fs::write(path, serde_json::to_vec(&saved).unwrap()).unwrap();
    isolated(tmp.path())
        .args([
            "clean",
            "--from-plan",
            "plan.json",
            "--execute",
            "--yes",
            "--permanent",
            "--json",
        ])
        .assert()
        .code(1)
        .stderr(predicates::str::contains("create and review a new plan"));
    assert!(tmp.path().join("target/cache").exists());
}

#[test]
fn from_plan_rejects_scope_changes_and_unapproved_execution_modes() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    save(tmp.path());
    for flag in ["--id", "--roots"] {
        isolated(tmp.path())
            .args(["clean", "--from-plan", "plan.json", flag, "cargo"])
            .assert()
            .code(2);
    }
    isolated(tmp.path())
        .args(["clean", "--from-plan", "plan.json", "--execute", "--json"])
        .assert()
        .code(1);
    isolated(tmp.path())
        .args([
            "clean",
            "--from-plan",
            "plan.json",
            "--execute",
            "--yes",
            "--json",
        ])
        .assert()
        .code(2)
        .stdout(predicates::str::contains("deletion mode differs"));
    assert!(tmp.path().join("target/cache").exists());
}

#[test]
fn interactive_approval_rejects_new_contents_inside_a_reviewed_target() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::sync::mpsc;
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let mut child = common::native(tmp.path())
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
            if line.unwrap().contains("Would remove") && tx.send(()).is_err() {
                break;
            }
        }
    });
    if rx.recv_timeout(std::time::Duration::from_secs(15)).is_err() {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("interactive preview never appeared");
    }
    fs::write(tmp.path().join("target/new-file"), b"not reviewed").unwrap();
    child.stdin.take().unwrap().write_all(b"y\n").unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(2));
    reader.join().unwrap();
    assert!(tmp.path().join("target/cache").exists());
    assert!(tmp.path().join("target/new-file").exists());
}
