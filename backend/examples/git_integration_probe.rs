// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Phase 0: exercise embedded Git without system Git, credentials or real projects.
//! Run: `cargo run --locked --example git_integration_probe`
#![allow(
    unused_crate_dependencies,
    reason = "This standalone probe uses only the dependencies needed for Git verification."
)]

use std::{error::Error, io, path::Path, time::Instant};

use git2::{Oid, Repository, RepositoryInitOptions, RepositoryOpenFlags, Signature};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const BRANCH: &str = "refs/heads/main";
const TAG: &str = "refs/tags/bluink/versions/probe";

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message).into())
    }
}

fn init(path: &Path, bare: bool) -> Result<Repository> {
    let mut options = RepositoryInitOptions::new();
    options
        .bare(bare)
        .initial_head("main")
        .external_template(false);
    Ok(Repository::init_opts(path, &options)?)
}

fn commit(repo: &Repository, content: &[u8], parents: &[Oid]) -> Result<Oid> {
    // Build a tree directly: neither the worktree nor Git's staging index is read.
    let blob = repo.blob(content)?;
    let mut builder = repo.treebuilder(None)?;
    builder.insert("fixture.json", blob, 0o100_644)?;
    let tree = repo.find_tree(builder.write()?)?;
    let signature = Signature::now("Bluink probe", "probe@localhost")?;
    let parents = parents
        .iter()
        .map(|id| repo.find_commit(*id))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(repo.commit(
        None,
        &signature,
        &signature,
        "Probe checkpoint",
        &tree,
        &parents.iter().collect::<Vec<_>>(),
    )?)
}

fn push(repo: &Repository, refs: &[&str]) -> Result<()> {
    let mut rejected = None;
    {
        let mut callbacks = git2::RemoteCallbacks::new();
        callbacks.push_update_reference(|name, status| {
            if let Some(status) = status {
                rejected = Some(format!("Rejected {name}: {status}"));
            }
            Ok(())
        });
        let mut options = git2::PushOptions::new();
        options.remote_callbacks(callbacks);
        repo.find_remote("origin")?.push(refs, Some(&mut options))?;
    }
    rejected.map_or_else(|| Ok(()), |message| Err(io::Error::other(message).into()))
}

fn fetch(repo: &Repository) -> Result<()> {
    repo.find_remote("origin")?.fetch(
        &[
            "refs/heads/main:refs/remotes/origin/main",
            "refs/tags/bluink/versions/*:refs/tags/bluink/versions/*",
        ],
        None,
        None,
    )?;
    Ok(())
}

fn verify_pack(repo: &Repository, head: Oid, tag: Oid) -> Result<()> {
    let packed = tempfile::tempdir()?;
    let mut pack = repo.packbuilder()?;
    let mut history = repo.revwalk()?;
    history.push(head)?;
    pack.insert_walk(&mut history)?;
    pack.insert_recursive(tag, None)?;
    pack.write(packed.path(), 0)?;
    // An independent object database proves the pack works without loose originals.
    let destination = tempfile::tempdir()?;
    let reader = init(destination.path(), true)?;
    let pack_dir = reader.path().join("objects/pack");
    std::fs::create_dir_all(&pack_dir)?;
    for entry in std::fs::read_dir(packed.path())? {
        let entry = entry?;
        std::fs::copy(entry.path(), pack_dir.join(entry.file_name()))?;
    }
    let reader = Repository::open_bare(destination.path())?;
    let tree = reader.find_commit(head)?.tree()?;
    let entry = tree
        .get_name("fixture.json")
        .ok_or_else(|| io::Error::other("Packed tree lost its fixture"))?;
    require(
        reader.find_blob(entry.id())?.content() == b"merged",
        "Packed blob differs",
    )?;
    require(
        reader.find_tag(tag)?.target_id() == head,
        "Packed tag lost its target",
    )?;
    let mut history = reader.revwalk()?;
    history.push(head)?;
    let mut count = 0;
    for id in history {
        let tree = reader.find_commit(id?)?.tree()?;
        let entry = tree
            .get_name("fixture.json")
            .ok_or_else(|| io::Error::other("Packed history lost its fixture"))?;
        reader.find_blob(entry.id())?;
        count += 1;
    }
    require(count == 4, "Packed history lost an ancestor")
}

fn main() -> Result<()> {
    let start = Instant::now();
    let version = git2::Version::get();
    println!(
        "libgit2={:?} vendored={} https={} ssh={}",
        version.libgit2_version(),
        version.vendored(),
        version.https(),
        version.ssh()
    );
    require(
        version.vendored() && version.https() && version.ssh(),
        "Vendored Git with HTTPS and SSH is required",
    )?;
    let fixture = tempfile::tempdir()?;
    let parent = init(fixture.path(), false)?;
    let project = fixture.path().join("project");
    std::fs::create_dir(&project)?;
    require(
        Repository::open_ext(
            &project,
            RepositoryOpenFlags::NO_SEARCH,
            std::iter::empty::<&Path>(),
        )
        .is_err(),
        "Opening a project discovered an ancestor repository",
    )?;
    drop(parent);
    let first = init(&project, false)?;
    let root = commit(&first, b"base", &[])?;
    first.reference(BRANCH, root, false, "Initial checkpoint")?;
    let remote_path = fixture.path().join("remote.git");
    let remote = init(&remote_path, true)?;
    let remote_url = url::Url::from_directory_path(&remote_path)
        .map_err(|()| io::Error::other("Invalid fixture URL"))?;
    first.remote("origin", remote_url.as_str())?;
    push(&first, &["refs/heads/main:refs/heads/main"])?;
    let second = Repository::clone(remote_url.as_str(), fixture.path().join("second"))?;
    let left = commit(&first, b"left", &[root])?;
    first.reference_matching(BRANCH, left, true, root, "Local edit")?;
    let right = commit(&second, b"right", &[root])?;
    second.reference_matching(BRANCH, right, true, root, "Other edit")?;
    push(&first, &["refs/heads/main:refs/heads/main"])?;
    require(
        push(&second, &["refs/heads/main:refs/heads/main"]).is_err(),
        "Diverged push must be rejected",
    )?;
    fetch(&second)?;
    require(second.merge_base(left, right)? == root, "Wrong merge base")?;
    let merged = commit(&second, b"merged", &[right, left])?;
    require(
        second
            .reference_matching(BRANCH, merged, true, root, "Stale publication")
            .is_err(),
        "Stale expected head must be rejected",
    )?;
    require(
        second.refname_to_id(BRANCH)? == right,
        "Rejected publication changed the branch",
    )?;
    second.reference_matching(BRANCH, merged, true, right, "Publish merge")?;
    let signature = Signature::now("Bluink probe", "probe@localhost")?;
    let tag = second.tag(
        "bluink/versions/probe",
        &second.find_object(merged, None)?,
        &signature,
        "Named version",
        false,
    )?;
    push(&second, &["refs/heads/main:refs/heads/main", TAG])?;
    fetch(&first)?;
    require(
        first.refname_to_id("refs/remotes/origin/main")? == merged,
        "Remote merge missing",
    )?;
    require(first.refname_to_id(TAG)? == tag, "Named version missing")?;
    require(
        remote.refname_to_id(BRANCH)? == merged,
        "Bare remote did not advance",
    )?;
    verify_pack(&second, merged, tag)?;
    println!(
        "PASS: root isolation, local checkpoints, clone, fetch, push rejection, ancestry, expected-head publication, named tags and independent pack reads ({:.2}s)",
        start.elapsed().as_secs_f64()
    );
    println!(
        "NOT TESTED: provider HTTPS/PAT/LFS, other release targets, application recovery or UI."
    );
    Ok(())
}
