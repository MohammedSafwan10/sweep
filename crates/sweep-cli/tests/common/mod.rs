use assert_cmd::Command;
use std::path::Path;

pub fn isolated(root: &Path) -> Command {
    let mut cmd = Command::from_std(native(root));
    cmd.timeout(std::time::Duration::from_secs(15));
    cmd
}

pub fn native(root: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_sweep"));
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
    cmd
}
