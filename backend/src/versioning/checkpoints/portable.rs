// SPDX-License-Identifier: MPL-2.0
//! An explicit Git import binds to immutable objects and holds the pruning lease.
use crate::versioning::{self, failure, lfs, repository, transport};
use crate::{
    store::io::{self, LockMode},
    store::{self, portable::Source},
};
use git2::{Oid, Repository};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Seek, Write},
    path::Path,
};

pub(crate) struct GitSource {
    repo: Repository,
    _lease: File,
    head: Oid,
    files: BTreeMap<String, Oid>,
    refs: BTreeMap<String, Oid>,
}

fn storage<T>(result: versioning::Result<T>) -> store::Result<T> {
    result.map_err(|error| store::Error::Invalid(error.to_string()))
}

impl GitSource {
    /// `directory` is an existing bare repository, for example a verified clone
    /// or another project's history.git. This never imports uncheckpointed saves.
    pub(crate) fn open(directory: &Path) -> versioning::Result<Self> {
        let repo = Repository::open_bare(directory)?;
        let lease = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        let head = repo.refname_to_id(repository::BRANCH)?;
        let files = repository::files(&repo, head)?;
        let refs = repo
            .references()?
            .map(|reference| {
                let reference = reference?;
                Ok((
                    reference.name()?.to_owned(),
                    reference
                        .resolve()?
                        .target()
                        .ok_or_else(|| failure("Import reference has no target"))?,
                ))
            })
            .collect::<versioning::Result<BTreeMap<_, _>>>()?;
        let config = files
            .get("bluink.json")
            .ok_or_else(|| failure("Git project has no settings"))?;
        let config = crate::domain::project::config::parse_project_config(
            repo.find_blob(*config)?.content(),
        )?;
        let project = config["id"]
            .as_str()
            .ok_or_else(|| failure("Git project has no identity"))?;
        for (name, oid) in &refs {
            if name.starts_with("refs/tags/bluink/versions/") {
                crate::versioning::history::named_tag(
                    &repo,
                    name,
                    &repo.find_tag(*oid)?,
                    Some(project),
                )?;
            }
        }
        Ok(Self {
            repo,
            _lease: lease,
            head,
            files,
            refs,
        })
    }

    fn bytes(&self, path: &str) -> versioning::Result<Vec<u8>> {
        let oid = self
            .files
            .get(path)
            .ok_or_else(|| failure("Path is absent from imported tree"))?;
        Ok(self.repo.find_blob(*oid)?.content().to_vec())
    }

    fn seed(&self, destination: &Path) -> versioning::Result<Option<String>> {
        if destination.join(super::DIRECTORY).try_exists()? {
            return Err(failure("Import history destination already exists"));
        }
        super::initialize(destination)?;
        let to = super::open(destination)?;
        crate::versioning::packing::copy_objects(&self.repo, &to)?;
        let heads = self
            .refs
            .values()
            .filter_map(|oid| {
                self.repo
                    .find_object(*oid, None)
                    .ok()?
                    .peel_to_commit()
                    .ok()
                    .map(|commit| commit.id())
            })
            .collect::<Vec<_>>();
        for pointer in transport::local_inventory(&self.repo, &heads)? {
            if let Some(path) = pointer.available_path(self.repo.path())? {
                lfs::receive(to.path(), &pointer, &mut File::open(path)?)?;
            }
        }
        // Missing historical objects remain explicitly unavailable. Every binary
        // referenced by the imported current tree is required before publishing.
        for path in self
            .files
            .keys()
            .filter(|path| repository::tracked(path) == Some(repository::Kind::Binary))
        {
            let pointer = lfs::Pointer::parse(&self.bytes(path)?)?;
            pointer.verified_path(to.path())?;
        }
        for (name, oid) in &self.refs {
            to.find_object(*oid, None)?;
            to.reference(name, *oid, false, "Bluink Git import")?;
        }
        if let Ok(remote) = self.repo.find_remote("origin") {
            to.remote("origin", remote.url()?)?;
        }
        Ok(Some(self.head.to_string()))
    }
}

impl Source for GitSource {
    fn paths(&self) -> store::Result<Vec<String>> {
        Ok(self
            .files
            .keys()
            .filter(|path| !matches!(path.as_str(), ".gitattributes" | ".gitignore"))
            .cloned()
            .collect())
    }
    fn read_json(&self, path: &str) -> store::Result<Vec<u8>> {
        storage(self.bytes(path))
    }
    fn open_asset(&self, path: &str) -> store::Result<File> {
        storage((|| {
            let bytes = self.bytes(path)?;
            if repository::tracked(path) == Some(repository::Kind::Binary) {
                let pointer = lfs::Pointer::parse(&bytes)?;
                Ok(File::open(pointer.verified_path(self.repo.path())?)?)
            } else {
                let mut file = tempfile::tempfile()?;
                file.write_all(&bytes)?;
                file.rewind()?;
                Ok(file)
            }
        })())
    }
    fn seed_history(&self, destination: &Path) -> store::Result<Option<String>> {
        storage(self.seed(destination))
    }
}
