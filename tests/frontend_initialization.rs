//! Run the real CLI with private XDG directories, never the user's config.
use std::{fs, path::Path, process::Command};

fn cli(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_riced"));
    command
        .env("XDG_CONFIG_HOME", home)
        .env("XDG_RUNTIME_DIR", home)
        .env("WAYLAND_DISPLAY", home.join("no-compositor"));
    command
}

#[test]
fn plain_startup_installs_before_connecting_to_wayland() {
    let home = tempfile::tempdir_in("/tmp/opencode").unwrap();
    let output = cli(home.path()).output().unwrap();
    // Deliberately no compositor: only installation is under test.
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot connect to Wayland"));
    let root = home.path().join("riced");
    assert!(root.join(".frontend-initialized").is_file());
    assert!(root.join("layers/bar.lua").is_file());
    assert!(root.join("root.lua").is_file());
}

#[test]
fn explicit_projects_never_populate_the_default_directory() {
    let home = tempfile::tempdir_in("/tmp/opencode").unwrap();
    let project = tempfile::tempdir_in("/tmp/opencode").unwrap();
    fs::write(project.path().join("config.lua"), "return {}").unwrap();
    let output = cli(home.path())
        .arg("-p")
        .arg(project.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!home.path().join("riced").exists());
    assert_eq!(fs::read_dir(project.path()).unwrap().count(), 1);
}

#[test]
fn explicit_repair_targets_only_the_selected_project() {
    let home = tempfile::tempdir_in("/tmp/opencode").unwrap();
    let project = tempfile::tempdir_in("/tmp/opencode").unwrap();
    fs::write(project.path().join("config.lua"), "invalid but user-owned").unwrap();
    let output = cli(home.path())
        .arg("-p")
        .arg(project.path())
        .args(["init", "--repair"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(project.path().join("root.lua").is_file());
    assert_eq!(
        fs::read_to_string(project.path().join("config.lua")).unwrap(),
        "invalid but user-owned"
    );
    assert!(!home.path().join("riced").exists());
}

#[test]
fn initialized_startup_does_not_restore_deleted_sources() {
    let home = tempfile::tempdir_in("/tmp/opencode").unwrap();
    assert!(cli(home.path()).arg("init").status().unwrap().success());
    let removed = home.path().join("riced/layers/bar.lua");
    fs::remove_file(&removed).unwrap();
    let _ = cli(home.path()).output().unwrap();
    assert!(!removed.exists());
    let output = cli(home.path())
        .args(["init", "--repair"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(removed.is_file());
}

#[test]
fn help_does_not_initialize_anything() {
    let home = tempfile::tempdir_in("/tmp/opencode").unwrap();
    assert!(
        cli(home.path())
            .arg("--help")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!home.path().join("riced").exists());
}

#[test]
fn ipc_queues_independent_requests_without_initializing_a_frontend() {
    let home = tempfile::tempdir_in("/tmp/opencode").unwrap();
    for args in [
        vec!["ipc", "settings"],
        vec!["ipc", "remove", "--id", "bar main"],
    ] {
        let output = cli(home.path()).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let requests: Vec<serde_json::Value> = fs::read_dir(home.path().join("riced.queue"))
        .unwrap()
        .map(|entry| {
            serde_json::from_str(&fs::read_to_string(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .any(|request| request["Ipc"]["command"] == "settings")
    );
    assert!(
        requests
            .iter()
            .any(|request| request["Ipc"]["args"] == serde_json::json!(["--id", "bar main"]))
    );
    assert!(!home.path().join("riced").exists());
}
