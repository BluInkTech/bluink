// SPDX-License-Identifier: MPL-2.0
//! Opt-in interoperability check. The application itself never invokes these executables.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    process::{Command, Output},
};

fn git(cwd: &Path, isolated_home: &Path, args: &[&str], skip_smudge: bool) -> Output {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", isolated_home.join("gitconfig"))
        .env("HOME", isolated_home)
        .env("USERPROFILE", isolated_home)
        .env("GIT_TERMINAL_PROMPT", "0");
    if skip_smudge {
        command.env("GIT_LFS_SKIP_SMUDGE", "1");
    } else {
        command.env_remove("GIT_LFS_SKIP_SMUDGE");
    }
    command.output().unwrap()
}
fn success(output: Output) -> Vec<u8> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
#[ignore = "Requires a stock Git client and git-lfs with file:// transport support"]
fn stock_git_lfs_reads_current_and_older_bytes_and_reports_missing_history() {
    let fixture = Fixture::new();
    let page = fixture.page("Stock Git LFS");
    let id = page["id"].as_str().unwrap();
    let identity = ItemId::try_from(id).unwrap();
    let relative = identity.asset_directory(Path::new("")).join("cover.png");
    let mut versions = Vec::new();
    for bytes in [
        b"unavailable version".as_slice(),
        b"available old version",
        b"current version",
    ] {
        crate::versioning::test_support::asset(&fixture.root, id, "cover.png", bytes).unwrap();
        versions.push((fixture.store.checkpoint().unwrap().head.unwrap(), bytes));
    }
    let missing = checkpoints::digest_hex(&Sha256::digest(versions[0].1));
    let missing_file = checkpoints::verified_asset_path(
        &fixture.root,
        &missing,
        u64::try_from(versions[0].1.len()).unwrap(),
    )
    .unwrap();
    std::fs::remove_file(missing_file).unwrap();
    let clients = tempfile::tempdir().unwrap();
    let home = clients.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let url = url::Url::from_directory_path(fixture.root.join(checkpoints::DIRECTORY)).unwrap();
    success(git(
        clients.path(),
        &home,
        &["clone", url.as_str(), "checkout"],
        true,
    ));
    let checkout = clients.path().join("checkout");
    success(git(&checkout, &home, &["lfs", "install", "--local"], true));
    for (commit, expected) in versions.iter().rev().take(2) {
        success(git(
            &checkout,
            &home,
            &["checkout", "--detach", commit],
            true,
        ));
        let spec = format!("HEAD:{}", relative.to_string_lossy().replace('\\', "/"));
        let pointer = String::from_utf8(success(git(
            &checkout,
            &home,
            &["cat-file", "blob", &spec],
            true,
        )))
        .unwrap();
        assert!(pointer.contains(&format!(
            "oid sha256:{}",
            checkpoints::digest_hex(&Sha256::digest(expected))
        )));
        assert!(pointer.contains(&format!("size {}", expected.len())));
        success(git(&checkout, &home, &["lfs", "pull"], false));
        assert_eq!(std::fs::read(checkout.join(&relative)).unwrap(), *expected);
    }
    success(git(
        &checkout,
        &home,
        &["checkout", "--detach", &versions[0].0],
        true,
    ));
    assert!(
        !git(&checkout, &home, &["lfs", "pull"], false)
            .status
            .success()
    );
    fixture.close();
}
