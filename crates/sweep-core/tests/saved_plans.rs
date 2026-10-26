use std::fs;
use std::path::Path;
use sweep_core::cleaner;
use sweep_core::model::{CleanAction, CleanOptions, Disposition, Finding, Safety};
use sweep_core::saved_plan::SavedPlan;

fn options() -> CleanOptions {
    CleanOptions {
        execute: true,
        to_trash: false,
        ..CleanOptions::default()
    }
}

fn reviewed(root: &Path) -> cleaner::CleanPlan {
    let finding = Finding {
        detector_id: "fixture".into(),
        label: "reviewed cache".into(),
        bytes: 4,
        safety: Safety::Safe,
        detail: String::new(),
        action: CleanAction::RemovePath { path: root.into() },
    };
    let plan = cleaner::plan(&[finding], &options());
    assert!(matches!(
        plan.items[0].disposition,
        Disposition::Remove { .. }
    ));
    plan
}

#[test]
fn changed_trees_are_rejected_before_any_of_their_contents_are_deleted() {
    for mutation in [
        "added",
        "removed",
        "renamed",
        "edited",
        "empty-directory",
        "root-replaced",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cache");
        fs::create_dir(&root).unwrap();
        let file = root.join("entry");
        fs::write(&file, b"old!").unwrap();
        let plan = reviewed(&root);
        match mutation {
            "added" => fs::write(root.join("new"), b"not approved").unwrap(),
            "removed" => fs::remove_file(&file).unwrap(),
            "renamed" => fs::rename(&file, root.join("new-name")).unwrap(),
            "edited" => {
                // Equal length and restored mtime must not defeat content hashing.
                let time =
                    filetime::FileTime::from_last_modification_time(&fs::metadata(&file).unwrap());
                fs::write(&file, b"new!").unwrap();
                filetime::set_file_mtime(&file, time).unwrap();
            }
            "empty-directory" => fs::create_dir(root.join("new-directory")).unwrap(),
            "root-replaced" => {
                fs::rename(&root, tmp.path().join("old-root")).unwrap();
                fs::create_dir(&root).unwrap();
                fs::write(&file, b"old!").unwrap();
            }
            _ => unreachable!(),
        }
        let receipt = cleaner::execute(&plan, &options());
        assert!(receipt.removed.is_empty(), "{mutation}");
        assert_eq!(receipt.errors.len(), 1, "{mutation}");
        assert!(
            receipt.errors[0].contains("changed since preview"),
            "{:?}",
            receipt.errors
        );
        assert!(root.exists(), "{mutation}");
        if mutation == "added" {
            assert!(root.join("new").exists());
        }
    }
}

#[test]
fn saved_plan_round_trip_executes_only_unchanged_reviewed_target() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("cache");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("entry"), b"old!").unwrap();
    let saved = SavedPlan::new(reviewed(&root)).unwrap();
    let path = tmp.path().join("plan.json");
    saved.write_new(&path).unwrap();
    assert!(
        saved.write_new(&path).is_err(),
        "never overwrite a reviewed plan"
    );
    let loaded = SavedPlan::read(&path).unwrap();
    assert_eq!(loaded.plan_id, saved.plan_id);
    let receipt = cleaner::execute(&loaded.plan, &options());
    assert!(receipt.errors.is_empty(), "{:?}", receipt.errors);
    assert_eq!(receipt.freed_bytes, 4);
    assert!(!root.exists());
    assert!(path.exists());
}

#[test]
fn tampered_unsupported_and_unfingerprinted_saved_plans_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("cache");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("entry"), b"old!").unwrap();
    let saved = SavedPlan::new(reviewed(&root)).unwrap();
    let value = serde_json::to_value(&saved).unwrap();
    for mutation in ["checksum", "schema", "old-schema", "snapshot", "path"] {
        let mut value = value.clone();
        match mutation {
            "checksum" => value["plan_id"] = "invalid".into(),
            "schema" => value["plan_schema_version"] = 99.into(),
            "old-schema" => value["plan_schema_version"] = 1.into(),
            "snapshot" => value["plan"]["items"][0]["snapshot"] = serde_json::Value::Null,
            "path" => value["plan"]["items"][0]["finding"]["action"]["path"] = "unreviewed".into(),
            _ => unreachable!(),
        }
        let path = tmp.path().join(format!("{mutation}.json"));
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(SavedPlan::read(&path).is_err(), "{mutation}");
        assert!(root.join("entry").exists());
    }
    let mut missing = reviewed(&root);
    missing.items[0].snapshot = None;
    assert!(SavedPlan::new(missing.clone()).is_err());
    let receipt = cleaner::execute(&missing, &options());
    assert!(receipt.errors[0].contains("missing reviewed snapshot"));
    assert!(root.exists());
}

#[test]
fn plan_cannot_be_written_inside_its_own_deletion_target() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("cache");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("entry"), b"old!").unwrap();
    let saved = SavedPlan::new(reviewed(&root)).unwrap();
    assert!(saved.write_new(&root.join("plan.json")).is_err());
    assert!(!root.join("plan.json").exists());
}

#[test]
fn deletion_mode_cannot_change_after_review() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("cache");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("entry"), b"old!").unwrap();
    let mut plan = reviewed(&root);
    plan.items[0].disposition = Disposition::Remove {
        via: "trash".into(),
    };
    let receipt = cleaner::execute(&plan, &options());
    assert!(receipt.errors[0].contains("deletion mode differs"));
    assert!(root.join("entry").exists());
}

#[test]
fn a_changed_target_does_not_delete_its_unchanged_siblings() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("cache");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("entry"), b"old!").unwrap();
    let saved = SavedPlan::new(reviewed(&root)).unwrap();
    fs::write(root.join("late-arrival"), b"keep").unwrap();
    let receipt = cleaner::execute(&saved.plan, &options());
    assert!(receipt.removed.is_empty());
    assert_eq!(fs::read(root.join("entry")).unwrap(), b"old!");
    assert_eq!(fs::read(root.join("late-arrival")).unwrap(), b"keep");
}
