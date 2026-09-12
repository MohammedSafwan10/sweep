use std::{fs, path::Path};
use sweep_core::{
    cleaner,
    detectors::{
        angular::AngularDetector, dotnet::DotNetDetector, pip_cache::PipCacheDetector,
        pycache::PyCacheDetector, pytest_caches::PyTestCachesDetector, Ctx, Detector,
    },
    model::{CleanAction, CleanOptions},
};

fn write(path: &Path, data: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

fn backed_cache(ctx: &Ctx) -> std::path::PathBuf {
    let project = ctx.project_roots[0].join("pkg");
    write(&project.join("module.py"), b"x = 1");
    write(
        &project.join("__pycache__/module.cpython-312.pyc"),
        b"bytecode",
    );
    project
}

#[test]
fn unicode_file_names_do_not_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    write(&ctx.project_roots[0].join("123456789é.txt"), b"keep");
    assert!(PyTestCachesDetector.scan(&ctx).is_empty());
}

#[test]
fn bytecode_cache_rejects_unrelated_files_and_subdirectories() {
    for name in ["notes.txt", "nested/orphan.pyc"] {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_tests(tmp.path());
        let project = backed_cache(&ctx);
        write(&project.join("__pycache__").join(name), b"keep");
        assert!(PyCacheDetector.scan(&ctx).is_empty(), "accepted {name}");
    }
}

#[test]
fn dotted_module_names_require_the_exact_source() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    let project = ctx.project_roots[0].join("pkg");
    write(&project.join("module.py"), b"wrong source");
    write(
        &project.join("__pycache__/module.other.cpython-312.pyc"),
        b"keep",
    );
    assert!(PyCacheDetector.scan(&ctx).is_empty());
    write(&project.join("module.other.py"), b"correct source");
    assert_eq!(PyCacheDetector.scan(&ctx).len(), 1);
}

#[test]
fn bytecode_sources_are_revalidated_before_execution() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    let project = backed_cache(&ctx);
    let opts = CleanOptions {
        execute: true,
        to_trash: false,
        ..CleanOptions::default()
    };
    let plan = cleaner::plan(&PyCacheDetector.scan(&ctx), &opts);
    fs::remove_file(project.join("module.py")).unwrap();
    let receipt = cleaner::execute(&plan, &opts);
    assert!(receipt.removed.is_empty());
    assert!(!receipt.errors.is_empty());
    assert!(project.join("__pycache__/module.cpython-312.pyc").exists());
}

#[test]
fn angular_targets_only_its_cache_subdirectory() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    let project = &ctx.project_roots[0];
    write(
        &project.join("package.json"),
        br#"{"devDependencies":{"@angular/cli":"19"}}"#,
    );
    write(&project.join(".angular/cache/entry"), b"cache");
    write(&project.join(".angular/notes.txt"), b"keep");
    let findings = AngularDetector.scan(&ctx);
    assert_eq!(findings.len(), 1);
    assert!(
        matches!(&findings[0].action, CleanAction::RemovePath { path } if path.ends_with(".angular/cache"))
    );
}

#[test]
fn pip_preserves_unknown_files_in_the_configured_cache_root() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    write(&ctx.pip_cache.join("http-v2/response"), b"cache");
    write(&ctx.pip_cache.join("notes.txt"), b"keep");
    let findings = PipCacheDetector.scan(&ctx);
    let receipt = cleaner::clean(
        &findings,
        &CleanOptions {
            execute: true,
            to_trash: false,
            ..CleanOptions::default()
        },
    );
    assert!(receipt.errors.is_empty());
    assert!(ctx.pip_cache.join("notes.txt").exists());
}

#[test]
fn dotnet_does_not_auto_delete_runtime_data_from_bin() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    let project = &ctx.project_roots[0];
    write(&project.join("app.csproj"), b"<Project/>");
    write(&project.join("bin/Debug/data.db"), b"live application data");
    let findings = DotNetDetector.scan(&ctx);
    assert_eq!(findings.len(), 1);
    assert!(matches!(findings[0].action, CleanAction::Manual { .. }));
}

#[test]
fn coverage_configuration_is_not_test_output() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    write(&ctx.project_roots[0].join("pyproject.toml"), b"[project]");
    write(
        &ctx.project_roots[0].join(".coverage.settings"),
        b"[run]\nbranch=true",
    );
    assert!(PyTestCachesDetector.scan(&ctx).is_empty());
}

#[test]
fn overlapping_roots_do_not_duplicate_new_detector_findings() {
    let tmp = tempfile::tempdir().unwrap();
    let mut ctx = Ctx::for_tests(tmp.path());
    let project = backed_cache(&ctx);
    write(&project.join("pyproject.toml"), b"[project]");
    write(&project.join(".pytest_cache/v/nodeids"), b"[]");
    write(&project.join("app.csproj"), b"<Project/>");
    write(&project.join("obj/build"), b"cache");
    ctx.project_roots.push(project);
    for detector in [
        &PyCacheDetector as &dyn Detector,
        &PyTestCachesDetector,
        &DotNetDetector,
    ] {
        assert_eq!(detector.scan(&ctx).len(), 1, "{}", detector.id());
    }
}

#[test]
fn dotnet_respects_finding_cap_and_ignores_solution_only_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    let root = &ctx.project_roots[0];
    write(&root.join("repository.sln"), b"solution");
    write(&root.join("bin/keep"), b"repository scripts");
    assert!(DotNetDetector.scan(&ctx).is_empty());
    for i in 0..210 {
        let project = root.join(format!("project-{i}"));
        write(&project.join("app.csproj"), b"<Project/>");
        write(&project.join("bin/app.dll"), b"output");
        write(&project.join("obj/generated"), b"output");
    }
    assert_eq!(DotNetDetector.scan(&ctx).len(), 200);
}

#[test]
fn custom_named_virtual_environments_are_preserved() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    let project = backed_cache(&ctx);
    write(&project.join("pyvenv.cfg"), b"home = fixture");
    write(&project.join("pyproject.toml"), b"[project]");
    write(&project.join(".pytest_cache/v/nodeids"), b"[]");
    assert!(PyCacheDetector.scan(&ctx).is_empty());
    assert!(PyTestCachesDetector.scan(&ctx).is_empty());
}

#[test]
fn invalid_dependency_values_do_not_authorize_cache_cleanup() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    let root = &ctx.project_roots[0];
    write(&root.join("node_modules/.vite/entry"), b"keep");
    write(&root.join(".angular/cache/entry"), b"keep");
    for value in ["null", "false", "123", "{}", "\"\""] {
        write(
            &root.join("package.json"),
            format!("{{\"dependencies\":{{\"vite\":{value},\"@angular/cli\":{value}}}}}")
                .as_bytes(),
        );
        assert!(sweep_core::detectors::vite::ViteDetector
            .scan(&ctx)
            .is_empty());
        assert!(AngularDetector.scan(&ctx).is_empty());
    }
}

#[test]
fn requirements_directory_is_not_a_project_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    write(
        &ctx.project_roots[0].join("requirements-dev.txt/keep"),
        b"directory",
    );
    write(
        &ctx.project_roots[0].join(".pytest_cache/keep"),
        b"user data",
    );
    assert!(PyTestCachesDetector.scan(&ctx).is_empty());
}

#[test]
fn new_manual_findings_survive_even_a_forced_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::for_tests(tmp.path());
    let root = &ctx.project_roots[0];
    write(&root.join("pyproject.toml"), b"[project]");
    write(&root.join(".coverage"), b"SQLite format 3\0");
    write(
        &root.join(".hypothesis/examples/example"),
        b"failing example",
    );
    write(&root.join("app.csproj"), b"<Project/>");
    write(&root.join("bin/data.db"), b"application data");
    write(&ctx.uv_cache.join("entry"), b"uv cache");
    let findings = sweep_core::detectors::scan_selected(
        &ctx,
        &[
            "pytest-caches".into(),
            "dotnet-build".into(),
            "uv-cache".into(),
        ],
    );
    assert_eq!(findings.len(), 4);
    let receipt = cleaner::clean(
        &findings,
        &CleanOptions {
            execute: true,
            to_trash: false,
            include: sweep_core::model::IncludeLevel::All,
            force_danger: true,
        },
    );
    assert!(receipt.removed.is_empty());
    assert_eq!(receipt.skipped.len(), 4);
    assert!(root.join("bin/data.db").exists());
    assert!(root.join(".coverage").exists());
    assert!(root.join(".hypothesis/examples/example").exists());
    assert!(ctx.uv_cache.join("entry").exists());
}
