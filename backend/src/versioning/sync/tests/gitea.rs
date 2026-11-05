// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

fn sign_in(store: &History, origin: &str, fixture: &Value, passphrase: &str) {
    let remote = transport::Remote::parse(origin).unwrap();
    let identity = transport::inspect_host(&remote).unwrap();
    if store
        .read_sync_auth(origin)
        .unwrap()
        .host_fingerprint
        .is_none()
    {
        store
            .trust_sync_host(&auth::TrustSyncHost {
                origin: origin.into(),
                trust: transport::model::TrustInput {
                    fingerprint: identity.fingerprint,
                    previous_fingerprint: None,
                },
            })
            .unwrap();
    }
    store
        .configure_sync_auth(auth::ConfigureSyncAuth {
            origin: origin.into(),
            auth: transport::model::AuthInput {
                account: fixture["account"].as_str().unwrap().into(),
                token: None,
                private_key: Some(fixture["key"].as_str().unwrap().into()),
                passphrase: Some(passphrase.into()),
                forget: false,
                use_agent: Some(false),
            },
        })
        .unwrap();
}

#[test]
#[ignore = "Run scripts/verify-gitea.ps1 to provision a disposable local TLS/SSH/LFS service"]
fn gitea_two_projects_merge_retry_and_retrieve_named_attachment() {
    let fixture: Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("BLUINK_GITEA_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let origin = fixture["project"].as_str().unwrap();
    let secret = fixture["passphrase"].as_str().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let first_root = temp.path().join("first");
    crate::versioning::test_support::create(&first_root, "UTC").unwrap();
    let first = History::register(&first_root, temp.path().join("user1").to_str()).unwrap();
    first.initialize().unwrap();
    sign_in(&first, origin, &fixture, secret);
    first
        .configure_sync(&model::ConfigureSync {
            origin: Some(origin.into()),
            create: Some(false),
        })
        .unwrap();
    sync(&first);
    let second_root = temp.path().join("second");
    first
        .clone_project(&model::CloneProject {
            origin: origin.into(),
            destination: second_root.to_string_lossy().into_owned(),
        })
        .unwrap();
    let second = History::register(&second_root, temp.path().join("user2").to_str()).unwrap();
    second.initialize().unwrap();
    sign_in(&second, origin, &fixture, secret);

    let a = page(&first, "A remote draft");
    let b = page(&second, "A local chapter");
    let id = a["id"].as_str().unwrap();
    let local_id = b["id"].as_str().unwrap();
    crate::versioning::test_support::asset(
        &first.root,
        id,
        "cover.png",
        b"actual attachment bytes",
    )
    .unwrap();
    let named = first
        .named_version(&crate::versioning::model::NamedVersionRequest {
            request_id: uuid::Uuid::now_v7().to_string(),
            title: "First cover".into(),
            item_id: Some(id.into()),
        })
        .unwrap();
    sync(&first);
    sign_in(&second, origin, &fixture, "wrong passphrase");
    assert!(second.prepare_sync().is_err());
    assert_eq!(load(&second, local_id), b);
    sign_in(&second, origin, &fixture, secret);
    sync(&second);
    sync(&first);
    assert_eq!(first.status().unwrap().head, second.status().unwrap().head);
    assert_eq!(load(&first, local_id), b);
    assert_eq!(load(&second, id), a);
    assert_eq!(
        std::fs::read(
            second
                .historical_asset(&named.commit_id, id, "cover.png")
                .unwrap()
        )
        .unwrap(),
        b"actual attachment bytes"
    );
    assert_eq!(
        crate::versioning::checkpoints::open(&second.root)
            .unwrap()
            .tag_names(Some("bluink/versions/*"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(first.sync_status().unwrap().state, "synced");
    first.stop().unwrap();
    second.stop().unwrap();
    drop((first, second));
    temp.close().unwrap();
}
