// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{
    super::{lfs, repository},
    *,
};
use git2::{Oid, Repository};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
};

fn missing_bytes(
    repo: &Repository,
    heads: &[Oid],
    history: bool,
) -> crate::versioning::Result<u64> {
    Ok(usage_for_pointers(repo, &inventory(repo, heads, history, &[])?)?.1)
}

/// Upload every attachment reachable from new commits, as a push does, not
/// from commits that the destination already has.
fn upload_new_history(
    repo: &Repository,
    remote: &Remote,
    auth: &Auth,
    heads: &[Oid],
    known_remote_commits: &[Oid],
) -> crate::versioning::Result<()> {
    let pointers = inventory(repo, heads, true, known_remote_commits)?;
    transfer_pointers(repo, remote, auth, &pointers, Direction::Upload)
}

pub(super) fn trusted_certificate_der() -> Option<Vec<u8>> {
    use base64::Engine as _;
    let path = std::env::var_os("BLUINK_GITEA_CERT")?;
    let pem = std::fs::read_to_string(path).ok()?;
    let base64: String = pem
        .lines()
        .filter(|line| !line.starts_with("---"))
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(base64)
        .ok()
}

fn commit(repo: &Repository, previous: Option<Oid>, files: &BTreeMap<String, Option<Oid>>) -> Oid {
    let tree = repository::changed_tree(repo, previous, files).unwrap();
    let signature = git2::Signature::now("Test", "test@invalid").unwrap();
    let parent = previous.map(|id| repo.find_commit(id).unwrap());
    repo.commit(
        None,
        &signature,
        &signature,
        "Test",
        &repo.find_tree(tree).unwrap(),
        &parent.iter().collect::<Vec<_>>(),
    )
    .unwrap()
}

thread_local! {
    pub(super) static LAST_SSH_TRANSFER: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

#[test]
#[ignore = "Run scripts/verify-gitea.ps1 to provision a disposable local TLS/SSH/LFS service"]
fn gitea_transport_matrix() {
    assert_no_client_tools();
    exercise_fixture();
}

fn assert_no_client_tools() {
    if std::env::var_os("BLUINK_GITEA_NO_TOOLS").is_some() {
        for tool in ["git", "git-lfs", "ssh"] {
            assert!(
                std::process::Command::new(tool)
                    .arg("--version")
                    .output()
                    .is_err(),
                "Test client unexpectedly has {tool} on PATH"
            );
        }
    }
}

fn exercise_fixture() {
    let fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("BLUINK_GITEA_FIXTURE").expect("Gitea fixture path"))
            .unwrap(),
    )
    .unwrap();
    for scheme in ["https", "ssh"] {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::parse(fixture[scheme].as_str().unwrap()).unwrap();
        let mut auth = Auth {
            account: fixture["account"].as_str().unwrap().into(),
            token: Some(fixture["token"].as_str().unwrap().into()),
            private_key: Some(std::path::PathBuf::from(fixture["key"].as_str().unwrap())),
            passphrase: Some(fixture["passphrase"].as_str().unwrap().into()),
            host_fingerprint: None,
            use_agent: false,
            cancel: None,
        };
        if scheme == "ssh" {
            auth.host_fingerprint = Some(inspect_host(&remote).unwrap().fingerprint);
            // Existing encrypted RSA PEM keys and native generated encrypted
            // Ed25519 keys both authenticate against the same real service.
            let probe = Repository::init(root.path().join("rsa-probe")).unwrap();
            fetch(&probe, &remote, &auth).unwrap();
            auth = generated_gitea_key(root.path(), &remote, &fixture, &auth);
        }
        let first = Repository::init(root.path().join("first")).unwrap();
        let second = Repository::init(root.path().join("second")).unwrap();
        let id = crate::domain::project::identity::ItemId::try_from(
            "00000000-0000-4000-8000-000000000001",
        )
        .unwrap();
        let content = format!(
            "{scheme} {} attachment {}",
            fixture["mode"],
            uuid::Uuid::new_v4()
        );
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(content.as_bytes()).unwrap();
        std::io::Seek::rewind(&mut file).unwrap();
        let pointer = lfs::capture(first.path(), &mut file).unwrap();
        let head = commit(
            &first,
            None,
            &BTreeMap::from([(
                format!("assets/{}/{}/test.png", id.bucket(), id.as_str()),
                Some(first.blob(&pointer.bytes()).unwrap()),
            )]),
        );
        transfer_lfs(&first, &remote, &auth, &[head], Direction::Upload, true).unwrap();
        if scheme == "ssh" {
            assert_eq!(LAST_SSH_TRANSFER.get(), fixture["mode"].as_str());
        }
        let signature = git2::Signature::now("Fixture", "fixture@invalid").unwrap();
        let tag_name = format!("bluink/versions/{}", uuid::Uuid::new_v4());
        let tag = first
            .tag(
                &tag_name,
                &first.find_object(head, None).unwrap(),
                &signature,
                "Fixture version",
                false,
            )
            .unwrap();
        let tags = BTreeMap::from([(format!("refs/tags/{tag_name}"), tag)]);
        push(&first, &remote, &auth, head, &tags).unwrap();
        let received = fetch(&second, &remote, &auth).unwrap();
        assert_eq!(received.head, Some(head));
        assert_eq!(received.tags, tags);
        transfer_lfs(&second, &remote, &auth, &[head], Direction::Download, false).unwrap();
        if scheme == "ssh" {
            assert_eq!(LAST_SSH_TRANSFER.get(), fixture["mode"].as_str());
        }
        assert_eq!(
            std::fs::read(pointer.verified_path(second.path()).unwrap()).unwrap(),
            content.as_bytes()
        );
        assert_push_rejected(&second, &remote, &auth, head);
        if scheme == "ssh" && std::env::var_os("BLUINK_GITEA_AGENT").is_some() {
            agent_sign_in(&second, &remote, &auth, &fixture, head);
        }
        if scheme == "ssh" {
            auth.host_fingerprint = Some(ssh::fingerprint(&[0; 32]));
            assert_eq!(
                fetch(&second, &remote, &auth).err().unwrap().state,
                "verify_server"
            );
            auth.host_fingerprint = Some(inspect_host(&remote).unwrap().fingerprint);
            auth.passphrase = Some("incorrect passphrase".into());
        } else {
            auth.token = Some("revoked-token".into());
        }
        let denied = fetch(&second, &remote, &auth).err().unwrap();
        assert_eq!(denied.state, "needs_sign_in", "{scheme}: {denied}");
    }
}

/// A commit that does not descend from the destination's main is refused,
/// and the destination keeps its head.
fn assert_push_rejected(repo: &Repository, remote: &Remote, auth: &Auth, head: Oid) {
    let diverged = commit(
        repo,
        None,
        &BTreeMap::from([(".gitignore".into(), Some(repo.blob(b"diverged").unwrap()))]),
    );
    let rejected = push(repo, remote, auth, diverged, &BTreeMap::new())
        .err()
        .unwrap();
    assert_eq!(rejected.state, "push_rejected", "{rejected}");
    assert_eq!(fetch(repo, remote, auth).unwrap().head, Some(head));
}

/// Serves the fixture key from an in-process agent at `SSH_AUTH_SOCK`, which
/// the client reaches the platform way: a Unix socket, or on Windows the named
/// pipe the OpenSSH agent uses. The agent then forgets the key, and sign-in
/// must fail without trying a key file.
fn agent_sign_in(
    repo: &Repository,
    remote: &Remote,
    trusted: &Auth,
    fixture: &serde_json::Value,
    head: Oid,
) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let socket = std::env::var_os("SSH_AUTH_SOCK").expect("Agent socket");
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ServerOptions;
        let first = runtime
            .block_on(async {
                ServerOptions::new()
                    .first_pipe_instance(true)
                    .create(&socket)
            })
            .unwrap();
        let name = socket.clone();
        let connections =
            futures_util::stream::unfold((first, name), |(server, name)| async move {
                let connected = server.connect().await.map(|()| server);
                let next = ServerOptions::new().create(&name).ok()?;
                Some((connected, (next, name)))
            });
        runtime.spawn(russh::keys::agent::server::serve(Box::pin(connections), ()));
    }
    #[cfg(unix)]
    {
        let listener = runtime
            .block_on(async { tokio::net::UnixListener::bind(&socket) })
            .unwrap();
        let connections = futures_util::stream::unfold(listener, |listener| async move {
            let accepted = listener.accept().await.map(|(stream, _)| stream);
            Some((accepted, listener))
        });
        runtime.spawn(russh::keys::agent::server::serve(Box::pin(connections), ()));
    }
    let agent = || async {
        #[cfg(windows)]
        let agent = russh::keys::agent::client::AgentClient::connect_named_pipe(&socket).await;
        #[cfg(unix)]
        let agent = russh::keys::agent::client::AgentClient::connect_uds(&socket).await;
        agent.unwrap()
    };
    let key = russh::keys::load_secret_key(
        fixture["key"].as_str().unwrap(),
        fixture["passphrase"].as_str(),
    )
    .unwrap();
    runtime.block_on(async { agent().await.add_identity(&key, &[]).await.unwrap() });
    let auth = Auth {
        account: trusted.account.clone(),
        host_fingerprint: trusted.host_fingerprint.clone(),
        use_agent: true,
        ..Auth::default()
    };
    assert_eq!(fetch(repo, remote, &auth).unwrap().head, Some(head));
    transfer_lfs(repo, remote, &auth, &[head], Direction::Download, false).unwrap();
    runtime.block_on(async {
        agent()
            .await
            .remove_identity(key.public_key())
            .await
            .unwrap();
    });
    assert_eq!(
        fetch(repo, remote, &auth).err().unwrap().state,
        "needs_sign_in"
    );
}

fn generated_gitea_key(
    root: &std::path::Path,
    remote: &Remote,
    fixture: &serde_json::Value,
    previous: &Auth,
) -> Auth {
    let user = root.join("user");
    let generated = keys::generate_key(
        &user,
        remote,
        &model::GenerateKeyInput {
            name: "generated".into(),
            account: "writer".into(),
            passphrase: "generated-key-test-passphrase".into(),
        },
    )
    .unwrap();
    let private = russh::keys::ssh_key::PrivateKey::read_openssh_file(std::path::Path::new(
        &generated.private_key,
    ))
    .unwrap();
    assert!(private.is_encrypted());
    assert!(private.decrypt("wrong passphrase").is_err());
    let client = reqwest::blocking::Client::builder()
        .add_root_certificate(
            reqwest::Certificate::from_der(&trusted_certificate_der().unwrap()).unwrap(),
        )
        .build()
        .unwrap();
    let mut endpoint = url::Url::parse(fixture["https"].as_str().unwrap()).unwrap();
    endpoint.set_path("/api/v1/user/keys");
    let response = client.post(endpoint).header("Authorization", format!("token {}", fixture["token"].as_str().unwrap()))
        .json(&serde_json::json!({"title":format!("native-{}", uuid::Uuid::new_v4()),"key":generated.public_key})).send().unwrap();
    assert!(response.status().is_success());
    credentials::trust_host(
        &user,
        remote,
        &model::TrustInput {
            fingerprint: previous.host_fingerprint.clone().unwrap(),
            previous_fingerprint: None,
        },
    )
    .unwrap();
    Auth::load(&user, remote).unwrap()
}

#[test]
fn generated_keys_are_encrypted_confined_and_never_overwritten() {
    let user = tempfile::tempdir().unwrap();
    let remote = Remote::parse("ssh://git@example.com/writing.git").unwrap();
    let mut input = model::GenerateKeyInput {
        name: "writing".into(),
        account: "writer".into(),
        passphrase: "secret test phrase".into(),
    };
    let generated = keys::generate_key(user.path(), &remote, &input).unwrap();
    assert!(std::path::Path::new(&generated.private_key).starts_with(user.path().join("ssh")));
    let key = russh::keys::ssh_key::PrivateKey::read_openssh_file(std::path::Path::new(
        &generated.private_key,
    ))
    .unwrap();
    assert!(key.is_encrypted());
    assert_eq!(
        key.decrypt(&input.passphrase)
            .unwrap()
            .public_key()
            .to_openssh()
            .unwrap(),
        generated.public_key
    );
    assert!(
        !std::fs::read_to_string(user.path().join("credentials.json"))
            .unwrap()
            .contains(&input.passphrase)
    );
    assert!(keys::generate_key(user.path(), &remote, &input).is_err());
    input.name = "../outside".into();
    assert!(keys::generate_key(user.path(), &remote, &input).is_err());
}

#[test]
fn local_git_and_lfs_transfer_preserve_immutable_refs_and_verify_bytes() {
    let root = tempfile::tempdir().unwrap();
    let remote = Remote::initialize_local(&root.path().join("remote.git")).unwrap();
    let first = Repository::init(root.path().join("first")).unwrap();
    let second = Repository::init(root.path().join("second")).unwrap();
    let owner =
        crate::domain::project::identity::ItemId::try_from("00000000-0000-4000-8000-000000000001")
            .unwrap();
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(b"first attachment").unwrap();
    std::io::Seek::rewind(&mut file).unwrap();
    let pointer = lfs::capture(first.path(), &mut file).unwrap();
    let path = format!("assets/{}/{}/image.png", owner.bucket(), owner.as_str());
    let initial = commit(
        &first,
        None,
        &BTreeMap::from([(path, Some(first.blob(&pointer.bytes()).unwrap()))]),
    );
    transfer_lfs(
        &first,
        &remote,
        &Auth::default(),
        &[initial],
        Direction::Upload,
        true,
    )
    .unwrap();
    push(&first, &remote, &Auth::default(), initial, &BTreeMap::new()).unwrap();
    let fetched = fetch(&second, &remote, &Auth::default()).unwrap();
    assert_eq!(fetched.head, Some(initial));
    assert_eq!(missing_bytes(&second, &[initial], true).unwrap(), 16);
    transfer_lfs(
        &second,
        &remote,
        &Auth::default(),
        &[initial],
        Direction::Download,
        false,
    )
    .unwrap();
    assert_eq!(
        std::fs::read(pointer.verified_path(second.path()).unwrap()).unwrap(),
        b"first attachment"
    );
    assert_eq!(missing_bytes(&second, &[initial], true).unwrap(), 0);
    let left = commit(
        &first,
        Some(initial),
        &BTreeMap::from([(".gitignore".into(), Some(first.blob(b"left").unwrap()))]),
    );
    let right = commit(
        &second,
        Some(initial),
        &BTreeMap::from([(".gitignore".into(), Some(second.blob(b"right").unwrap()))]),
    );
    push(&first, &remote, &Auth::default(), left, &BTreeMap::new()).unwrap();
    assert!(push(&second, &remote, &Auth::default(), right, &BTreeMap::new()).is_err());
    assert_eq!(
        remote
            .local_repository()
            .unwrap()
            .refname_to_id(repository::BRANCH)
            .unwrap(),
        left
    );
    assert!(pointer.verified_path(second.path()).is_ok());
}

#[test]
fn each_attachment_upload_confirms_the_actual_destination() {
    let root = tempfile::tempdir().unwrap();
    let first = Remote::initialize_local(&root.path().join("first.git")).unwrap();
    let second = Remote::initialize_local(&root.path().join("second.git")).unwrap();
    let source = Repository::init_bare(root.path().join("source.git")).unwrap();
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(b"cover").unwrap();
    std::io::Seek::rewind(&mut file).unwrap();
    let pointer = lfs::capture(source.path(), &mut file).unwrap();
    let transfer = |remote| {
        objects::transfer_pointers(
            &source,
            remote,
            &Auth::default(),
            std::slice::from_ref(&pointer),
            Direction::Upload,
        )
    };
    transfer(&first).unwrap();
    let first_git = first.local_repository().unwrap();
    let installed = pointer.verified_path(first_git.path()).unwrap();
    std::fs::remove_file(&installed).unwrap();
    transfer(&first).unwrap();
    assert!(pointer.verified_path(first_git.path()).is_ok());
    std::fs::remove_file(pointer.verified_path(source.path()).unwrap()).unwrap();
    transfer(&first).unwrap();
    assert!(
        transfer(&second).is_err(),
        "Another origin cannot borrow the first origin's evidence"
    );
    std::fs::write(installed, b"wrong").unwrap();
    assert!(
        transfer(&first).is_err(),
        "Same-size corruption is not valid remote evidence"
    );
    let nonexistent = Remote::Local(root.path().join("never-contacted.git"));
    objects::transfer_pointers(
        &source,
        &nonexistent,
        &Auth::default(),
        &[],
        Direction::Upload,
    )
    .unwrap();
}

#[test]
fn lightweight_clone_pushes_edits_without_fetching_already_remote_history() {
    let root = tempfile::tempdir().unwrap();
    let remote = Remote::initialize_local(&root.path().join("origin.git")).unwrap();
    let source = Repository::init(root.path().join("first")).unwrap();
    let clone = Repository::init(root.path().join("second")).unwrap();
    let auth = Auth::default();
    let owner =
        crate::domain::project::identity::ItemId::try_from("00000000-0000-4000-8000-000000000002")
            .unwrap();
    let path = format!("assets/{}/{}/cover.png", owner.bucket(), owner.as_str());
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(b"old").unwrap();
    std::io::Seek::rewind(&mut file).unwrap();
    let pointer = lfs::capture(source.path(), &mut file).unwrap();
    let first = commit(
        &source,
        None,
        &BTreeMap::from([(path.clone(), Some(source.blob(&pointer.bytes()).unwrap()))]),
    );
    let second = commit(&source, Some(first), &BTreeMap::from([(path, None)]));
    upload_new_history(&source, &remote, &auth, &[second], &[]).unwrap();
    push(&source, &remote, &auth, second, &BTreeMap::new()).unwrap();
    fetch(&clone, &remote, &auth).unwrap();
    transfer_lfs(
        &clone,
        &remote,
        &auth,
        &[second],
        Direction::Download,
        false,
    )
    .unwrap();
    assert_eq!(missing_bytes(&clone, &[second], true).unwrap(), 3);
    let edit = commit(
        &clone,
        Some(second),
        &BTreeMap::from([(".gitignore".into(), Some(clone.blob(b"edit").unwrap()))]),
    );
    upload_new_history(&clone, &remote, &auth, &[edit], &[second]).unwrap();
    push(&clone, &remote, &auth, edit, &BTreeMap::new()).unwrap();
    assert!(!pointer.path(clone.path()).exists());
    assert_eq!(fetch(&source, &remote, &auth).unwrap().head, Some(edit));
}

#[test]
fn failed_attachment_receive_never_publishes_a_partial_object() {
    let root = tempfile::tempdir().unwrap();
    let pointer = lfs::Pointer::parse(b"version https://git-lfs.github.com/spec/v1\noid sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nsize 0\n").unwrap();
    assert!(lfs::receive(root.path(), &pointer, &mut &b"unexpected bytes"[..]).is_err());
    assert!(!pointer.path(root.path()).exists());
    lfs::receive(root.path(), &pointer, &mut &b""[..]).unwrap();
    assert!(pointer.verified_path(root.path()).is_ok());
}

#[test]
fn current_tree_download_includes_attachment_decision_alternatives() {
    use crate::domain::merge::{
        decisions,
        model::{Context, Source},
    };
    use crate::domain::project::identity::ItemId;
    let root = tempfile::tempdir().unwrap();
    let remote = Remote::initialize_local(&root.path().join("remote.git")).unwrap();
    let source = Repository::init(root.path().join("source")).unwrap();
    let destination = Repository::init(root.path().join("destination")).unwrap();
    let mut pointers = Vec::new();
    for bytes in [b"first".as_slice(), b"second".as_slice()] {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(bytes).unwrap();
        std::io::Seek::rewind(&mut file).unwrap();
        pointers.push(lfs::capture(source.path(), &mut file).unwrap());
    }
    let owner = ItemId::try_from("00000000-0000-4000-8000-000000000001").unwrap();
    let path = format!("assets/{}/{}/photo.png", owner.bucket(), owner.as_str());
    let context = Context {
        target: owner.as_str().to_owned(),
        base_epoch: "base".into(),
        left: Source::Commit {
            id: "left".into(),
            committer_time: 1,
            device: "a".into(),
        },
        right: Source::Commit {
            id: "right".into(),
            committer_time: 2,
            device: "b".into(),
        },
    };
    let alternatives = pointers
        .iter()
        .map(|p| String::from_utf8(p.bytes()).unwrap())
        .collect::<Vec<_>>();
    let proposal = decisions::file::propose(
        &context,
        &path,
        None,
        Some(&alternatives[0]),
        Some(&alternatives[1]),
    )
    .unwrap();
    let record = crate::store::commands::with_authoring_values(|values| {
        decisions::prepare(proposal, None, "UTC", values)
    })
    .unwrap();
    let item_path = ItemId::try_from(record["id"].as_str().unwrap())
        .unwrap()
        .path(std::path::Path::new(""))
        .to_string_lossy()
        .replace('\\', "/");
    let head = commit(
        &source,
        None,
        &BTreeMap::from([(
            item_path,
            Some(source.blob(&serde_json::to_vec(&record).unwrap()).unwrap()),
        )]),
    );
    let index =
        crate::store::Store::create(&root.path().join("inventory"), uuid::Uuid::now_v7()).unwrap();
    let mut work = super::super::checkpoints::inventory::Work::default();
    let required =
        super::super::checkpoints::inventory::history(&source, &index, &[head], &[], &mut work)
            .unwrap();
    assert_eq!(required.len(), 2);
    assert!(pointers.iter().all(|pointer| required.contains(pointer)));
    transfer_lfs(
        &source,
        &remote,
        &Auth::default(),
        &[head],
        Direction::Upload,
        true,
    )
    .unwrap();
    push(&source, &remote, &Auth::default(), head, &BTreeMap::new()).unwrap();
    fetch(&destination, &remote, &Auth::default()).unwrap();
    assert_eq!(missing_bytes(&destination, &[head], false).unwrap(), 11);
    transfer_lfs(
        &destination,
        &remote,
        &Auth::default(),
        &[head],
        Direction::Download,
        false,
    )
    .unwrap();
    for pointer in pointers {
        pointer.verified_path(destination.path()).unwrap();
    }
}

#[test]
fn urls_do_not_embed_secrets_or_enable_untrusted_protocols() {
    for value in [
        "https://token@example.com/repo.git",
        "https://example.com/repo?token=secret",
        "ssh://git:secret@example.com/repo",
        "http://example.com/repo",
        "ext::bad",
        "relative/path",
        "ssh://host/repo",
    ] {
        assert!(Remote::parse(value).is_err(), "{value}");
    }
    assert!(matches!(
        Remote::parse("git@example.com:org/repo.git").unwrap(),
        Remote::Ssh(_)
    ));
    assert!(http::checked_url("https://objects.example.com/download?signature=ok").is_ok());
    assert!(http::checked_url("https://user:secret@objects.example.com/file").is_err());
}

#[test]
fn credential_updates_preserve_accounts_without_persisting_passphrases() {
    let user = tempfile::tempdir().unwrap();
    let first = Remote::parse("https://example.com/first.git").unwrap();
    let second = Remote::parse("https://example.com/second.git").unwrap();
    for remote in [&first, &second] {
        credentials::configure_auth(
            user.path(),
            remote,
            model::AuthInput {
                account: "writer".into(),
                token: Some(remote.url().unwrap()),
                private_key: None,
                passphrase: Some("session-secret".into()),
                forget: false,
                use_agent: None,
            },
        )
        .unwrap();
    }
    assert_eq!(
        Auth::load(user.path(), &first).unwrap().token,
        Some(first.url().unwrap())
    );
    credentials::configure_auth(
        user.path(),
        &first,
        model::AuthInput {
            account: "writer".into(),
            token: None,
            private_key: None,
            passphrase: None,
            forget: true,
            use_agent: None,
        },
    )
    .unwrap();
    assert!(!Auth::load(user.path(), &first).unwrap().status().has_token);
    assert!(Auth::load(user.path(), &second).unwrap().status().has_token);
    assert!(
        !std::fs::read_to_string(user.path().join("credentials.json"))
            .unwrap()
            .contains("session-secret")
    );
}

#[test]
fn trust_is_scoped_to_host_and_port_and_requires_expected_old_key() {
    let user = tempfile::tempdir().unwrap();
    let one = Remote::parse("ssh://git@example.com:2222/a.git").unwrap();
    let two = Remote::parse("ssh://git@example.com:2222/b.git").unwrap();
    let other_port = Remote::parse("ssh://git@example.com/b.git").unwrap();
    let first = ssh::fingerprint(&[1; 32]);
    let second = ssh::fingerprint(&[2; 32]);
    credentials::trust_host(
        user.path(),
        &one,
        &model::TrustInput {
            fingerprint: first.clone(),
            previous_fingerprint: None,
        },
    )
    .unwrap();
    assert_eq!(
        Auth::load(user.path(), &two).unwrap().host_fingerprint,
        Some(first.clone())
    );
    assert!(
        Auth::load(user.path(), &other_port)
            .unwrap()
            .host_fingerprint
            .is_none()
    );
    assert!(
        credentials::trust_host(
            user.path(),
            &one,
            &model::TrustInput {
                fingerprint: second.clone(),
                previous_fingerprint: None
            }
        )
        .is_err()
    );
    credentials::trust_host(
        user.path(),
        &one,
        &model::TrustInput {
            fingerprint: second.clone(),
            previous_fingerprint: Some(first),
        },
    )
    .unwrap();
    assert_eq!(
        Auth::load(user.path(), &two).unwrap().host_fingerprint,
        Some(second)
    );
}

#[test]
fn ssh_git_commands_match_libgit2_quoting() {
    for (url, upload) in [
        (
            "ssh://git@example.com:2222/writer/project.git",
            "git-upload-pack '/writer/project.git'",
        ),
        (
            "git@example.com:org/repo.git",
            "git-upload-pack '/org/repo.git'",
        ),
        (
            "ssh://git@example.com/~writer/repo.git",
            "git-upload-pack '~writer/repo.git'",
        ),
        (
            "ssh://git@example.com/it's!/x.git",
            "git-upload-pack '/it'\\''s'\\!'/x.git'",
        ),
    ] {
        let remote = Remote::parse(url).unwrap();
        assert_eq!(
            ssh::git_command(&remote, "git-upload-pack").unwrap(),
            upload
        );
    }
}

#[test]
fn git_transports_serve_only_the_scoped_destination() {
    let remote = Remote::parse("https://example.com/writer/project.git").unwrap();
    let auth = Auth::default();
    smart::with_operation(&remote, &auth, || {
        let operation = smart::require(None, "https://example.com/writer/project.git");
        assert!(operation.is_err(), "No operation outside a scope");
    });
    let operation = std::sync::Arc::new(smart::Operation {
        remote,
        auth: Auth::default(),
    });
    assert!(smart::require(Some(&operation), "https://example.com/writer/project.git").is_ok());
    for changed in [
        "https://example.com/writer/other.git",
        "https://elsewhere.example/writer/project.git",
        "ssh://git@example.com/writer/project.git",
    ] {
        assert_eq!(
            smart::require(Some(&operation), changed)
                .err()
                .unwrap()
                .message(),
            "Credential destination changed"
        );
    }
}

#[test]
fn cancellation_interrupts_a_silent_ssh_server() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let silent = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let _ = released.recv();
        drop(stream);
    });
    let root = tempfile::tempdir().unwrap();
    let repo = Repository::init(root.path()).unwrap();
    let remote = Remote::parse(&format!("ssh://git@127.0.0.1:{port}/writer/project.git")).unwrap();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let auth = Auth {
        account: "writer".into(),
        host_fingerprint: Some(ssh::fingerprint(&[0; 32])),
        cancel: Some(std::sync::Arc::clone(&cancel)),
        ..Auth::default()
    };
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let started = std::time::Instant::now();
    let error = fetch(&repo, &remote, &auth).err().unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "Cancellation waited for the handshake limit"
    );
    assert_eq!(error.state, "destination_unavailable", "{error}");
    release.send(()).unwrap();
    canceller.join().unwrap();
    silent.join().unwrap();
}

#[test]
fn ssh_framing_streams_binary_data_and_rejects_bad_lengths() {
    let mut encoded = Vec::new();
    wire::write(&mut encoded, b"hello\0world").unwrap();
    encoded.extend(b"0000");
    let mut source = encoded.as_slice();
    let mut body = wire::Body::new(&mut source);
    let mut output = Vec::new();
    body.read_to_end(&mut output).unwrap();
    assert_eq!(output, b"hello\0world");
    for header in [b"0002", b"0003", b"fff1", b"ffff", b"zzzz"] {
        assert!(wire::read(&mut &header[..]).is_err());
    }
}

#[test]
fn ssh_framing_accepts_the_largest_protocol_packet() {
    let largest = vec![7; 65_516];
    let mut encoded = Vec::new();
    wire::write(&mut encoded, &largest).unwrap();
    assert_eq!(&encoded[..4], b"fff0");
    assert!(wire::write(&mut Vec::new(), &vec![0; 65_517]).is_err());
    let Ok(wire::Packet::Data(bytes)) = wire::read(&mut encoded.as_slice()) else {
        panic!("expected one data packet");
    };
    assert_eq!(bytes, largest);
}

fn serve_once(
    listener: std::net::TcpListener,
    body: Vec<u8>,
    status: &str,
) -> std::thread::JoinHandle<String> {
    use std::io::BufRead as _;
    let status = status.to_owned();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            headers.push_str(&line);
        }
        let length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .and_then(|s| s.parse::<usize>().ok())
            })
            .unwrap_or_default();
        let mut request = vec![0; length];
        reader.read_exact(&mut request).unwrap();
        write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n", body.len()).unwrap();
        stream.write_all(&body).unwrap();
        headers
    })
}

#[test]
fn batch_credentials_are_not_forwarded_to_object_actions() {
    let root = tempfile::tempdir().unwrap();
    let repo = Repository::init(root.path().join("project")).unwrap();
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(b"private attachment").unwrap();
    std::io::Seek::rewind(&mut file).unwrap();
    let pointer = lfs::capture(&root.path().join("source"), &mut file).unwrap();
    let (oid, size) = pointer.fingerprint();
    let object_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let object_url = format!("http://{}/object", object_listener.local_addr().unwrap());
    let object = serve_once(object_listener, b"private attachment".to_vec(), "200 OK");
    let batch_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let batch_url = format!("http://{}/info/lfs", batch_listener.local_addr().unwrap());
    let batch = serve_once(batch_listener, serde_json::to_vec(&serde_json::json!({"objects":[{"oid":oid,"size":size,"actions":{"download":{"href":object_url}}}]})).unwrap(), "200 OK");
    http::transfer_endpoint(
        &repo,
        &Auth::default(),
        &http::Endpoint {
            href: batch_url,
            header: BTreeMap::from([("Authorization".into(), "Bearer batch-secret".into())]),
            expires_at: None,
            expires_in: None,
        },
        &[pointer],
        Direction::Download,
    )
    .unwrap();
    assert!(
        batch
            .join()
            .unwrap()
            .to_ascii_lowercase()
            .contains("authorization: bearer batch-secret")
    );
    assert!(
        !object
            .join()
            .unwrap()
            .to_ascii_lowercase()
            .contains("authorization:")
    );
}

#[test]
fn lfs_object_errors_use_structured_status_without_disclosing_provider_text() {
    let root = tempfile::tempdir().unwrap();
    let repo = Repository::init(root.path()).unwrap();
    let pointer=lfs::Pointer::parse(b"version https://git-lfs.github.com/spec/v1\noid sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nsize 0\n").unwrap();
    for (code, state) in [
        (401, "needs_sign_in"),
        (403, "no_access"),
        (507, "storage_full"),
        (404, "attachments_unavailable"),
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let href = format!("http://{}/info/lfs", listener.local_addr().unwrap());
        let (oid, size) = pointer.fingerprint();
        let body=serde_json::to_vec(&serde_json::json!({"objects":[{"oid":oid,"size":size,"error":{"code":code,"message":"example secret value"}}]})).unwrap();
        let server = serve_once(listener, body, "200 OK");
        let error = http::transfer_endpoint(
            &repo,
            &Auth::default(),
            &http::Endpoint {
                href,
                header: BTreeMap::new(),
                expires_at: None,
                expires_in: None,
            },
            std::slice::from_ref(&pointer),
            Direction::Download,
        )
        .unwrap_err();
        assert_eq!(error.state, state);
        assert!(!error.to_string().contains("example secret"));
        server.join().unwrap();
    }
}
