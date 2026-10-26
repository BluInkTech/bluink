// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use std::path::Path;
use std::process::{Child, Command, Stdio};

fn at(root: &Path) -> Application {
    Application::new(ApplicationOptions {
        workspace_template_path: concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/vendor/workspace-starter.json"
        )
        .into(),
        project_root: root.to_string_lossy().into_owned(),
        timezone: Some("UTC".into()),
        cache_location: Some("project".into()),
        public_dir: None,
        data_dir: None,
        web_font_folder: None,
        pdf_font_folder: None,
        docx_font_folder: None,
        typst_binary: None,
    })
    .unwrap()
}

// Own the child even if readiness, exclusion or recovery assertions fail.
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "invoked by cache_ownership_recovers_after_abrupt_process_exit"]
fn cache_owner_child() {
    let root = PathBuf::from(std::env::var_os("BLUINK_CACHE_OWNER_CHILD").unwrap());
    let app = at(&root.join("project"));
    app.initialize_sync(true).unwrap();
    std::fs::write(root.join("ready"), "ready").unwrap();
    // The parent forcibly terminates this process; no graceful stop or destructor runs.
    loop {
        std::thread::park();
    }
}

#[test]
fn cache_ownership_recovers_after_abrupt_process_exit() {
    let root = tempfile::tempdir().unwrap();
    let mut child = OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "application::tests::cache_process::cache_owner_child",
                "--ignored",
                "--nocapture",
            ])
            .env("BLUINK_CACHE_OWNER_CHILD", root.path())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.path().join("ready").is_file() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "Cache owner exited before readiness"
        );
        assert!(
            Instant::now() < deadline,
            "Cache owner did not become ready"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let app = at(&root.path().join("project"));
    let error = app.initialize_sync(true).unwrap_err();
    assert!(error.to_string().contains("cache is already in use"));
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let opened = app.initialize_sync(true).unwrap();
    assert_eq!(app.status().unwrap()["index"]["state"], "ready");
    app.stop_sync().unwrap();
    assert!(PathBuf::from(format!("{}.lock", opened["indexPath"].as_str().unwrap())).is_file());
}
