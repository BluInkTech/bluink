// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::*;

#[test]
fn only_successfully_initialized_applications_own_shared_history_workers() {
    let project = TestProject::new("history-ownership");
    let server = project.application();
    server.initialize_sync().unwrap();
    assert_eq!(server.history.active_sessions_for_test(), 1);

    // An uninitialized application has no worker ownership to release.
    drop(project.application());
    assert_eq!(server.history.active_sessions_for_test(), 1);

    let direct = project.application();
    direct.initialize_sync().unwrap();
    assert_eq!(server.history.active_sessions_for_test(), 2);
    direct.initialize_sync().unwrap();
    assert_eq!(server.history.active_sessions_for_test(), 2);
    direct.stop_sync().unwrap();
    direct.stop_sync().unwrap();
    assert_eq!(server.history.active_sessions_for_test(), 1);
    server.stop_sync().unwrap();
    assert_eq!(server.history.active_sessions_for_test(), 0);
    // Windows refuses this cleanup if the last history session retained SQLite.
    project.close().unwrap();
}
