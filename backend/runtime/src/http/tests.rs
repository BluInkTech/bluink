// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::application::{ApplicationOptions, BluinkApplication};
mod analytics;
mod catalogue;
mod contract;
mod diagnostics;
mod exports;
mod files;
mod items;
mod listener;
mod queries;
mod settings;
mod snippets;
mod structured_query;
mod support;
mod theme_previews;
mod workspace_notes;

#[test]
fn http_status_is_classified_by_type_and_preserves_messages() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        for (error, expected) in [
            (
                ApplicationError::ShuttingDown,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                ApplicationError::NotInitialized("Initialize the project before using it."),
                StatusCode::BAD_REQUEST,
            ),
            // Identical prose must not accidentally classify a validation error.
            (
                invalid_request("The application is shutting down."),
                StatusCode::BAD_REQUEST,
            ),
            (
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Read denied").into(),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ] {
            let message = error.to_string();
            let response = error_response(&trace::Record::default(), error);
            assert_eq!(response.status(), expected);
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({"error":message,"diagnostics":[]})
            );
        }
    });
}

#[test]
fn paths_are_decoded_once_and_keep_literal_percent_and_question_marks() {
    use super::files::file;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("index.html"), "home").unwrap();
    std::fs::write(root.path().join("%2e%2e.css"), "percent").unwrap();
    assert!(file(root.path(), "/?query=ignored").is_some());
    assert_eq!(
        file(root.path(), "/%252e%252e.css").unwrap(),
        root.path().join("%2e%2e.css")
    );
    for path in [
        "/%zz",
        "/%",
        "/%2",
        "/%ff",
        "/%00",
        "/%5cindex.html",
        "/a/%2e%2e/index.html",
        "/%2e%2e/index.html",
        "/C:%2findex.html",
    ] {
        assert!(file(root.path(), path).is_none(), "{path}");
    }
    #[cfg(unix)]
    {
        std::fs::write(root.path().join("what?.css"), "question").unwrap();
        assert_eq!(
            file(root.path(), "/what%3f.css").unwrap(),
            root.path().join("what?.css")
        );
    }
}

#[test]
fn blocking_admission_stays_owned_after_disconnect_and_stop_rejects_queued_work() {
    let folder = tempfile::tempdir().unwrap();
    let application = BluinkApplication::new(ApplicationOptions {
        workspace_template_path: concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/vendor/workspace-starter.json"
        )
        .into(),
        project_root: folder.path().to_string_lossy().into_owned(),
        timezone: Some("UTC".to_owned()),
        cache_location: None,
        public_dir: None,
        data_dir: None,
        web_font_folder: None,
        pdf_font_folder: None,
        docx_font_folder: None,
        typst_binary: None,
    })
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    runtime.block_on(async {
        application.initialize(false).await.unwrap();
        let state = HttpState {
            application: Arc::downgrade(&application.inner),
            work: Arc::new(Semaphore::new(1)),
        };
        let (started, running) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let (finished, done) = std::sync::mpsc::channel();
        let first_state = state.clone();
        let first = tokio::spawn(async move {
            worker(&first_state, trace::Record::default(), move |application| {
                let _operation = application.begin_operation()?;
                started.send(()).unwrap();
                wait.recv().unwrap();
                finished.send(()).unwrap();
                Ok(())
            })
            .await
        });
        running.recv_timeout(Duration::from_secs(5)).unwrap();
        // Dropping the HTTP handler cannot abort the already-started blocking action.
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        assert_eq!(state.work.available_permits(), 0);
        let queued_state = state.clone();
        let mut queued = tokio::spawn(async move {
            worker::<()>(&queued_state, trace::Record::default(), |_| {
                panic!("queued action started")
            })
            .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut queued)
                .await
                .is_err()
        );
        state.work.close();
        assert!(matches!(
            queued.await.unwrap(),
            Err(ApplicationError::ShuttingDown)
        ));
        assert_eq!(state.work.available_permits(), 0);
        release.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(5)).unwrap();
        application.stop().await.unwrap();
    });
}
