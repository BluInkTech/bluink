// SPDX-License-Identifier: MPL-2.0
//! Clone through a validated immutable Git source, then publish a complete local database.
use super::{install_tags, model::*};
use crate::versioning::{History, Result, checkpoints, failure, repository, transport};
use std::path::Path;
impl History {
    pub(crate) fn clone_project(&self, request: &CloneProject) -> Result<ClonedProject> {
        let destination = Path::new(&request.destination);
        if !destination.is_absolute() || destination.try_exists()? {
            return Err(failure("Choose a new absolute destination folder"));
        }
        let parent = destination
            .parent()
            .ok_or_else(|| failure("Destination has no parent"))?;
        if !parent.is_dir() {
            return Err(failure("Destination parent must exist"));
        }
        let remote = transport::Remote::parse(&request.origin)?;
        let auth = self.auth(&remote)?;
        let stage = tempfile::Builder::new()
            .prefix(".bluink-fetch-")
            .tempdir_in(parent)?;
        checkpoints::initialize(stage.path())?;
        let repo = checkpoints::open(stage.path())?;
        let incoming = transport::fetch(&repo, &remote, &auth)?;
        let head = incoming
            .head
            .ok_or_else(|| failure("The destination has no Bluink project"))?;
        let files = repository::files(&repo, head)?;
        transport::transfer_lfs(
            &repo,
            &remote,
            &auth,
            &[head],
            transport::Direction::Download,
            false,
        )?;
        super::super::restore::validate_complete(&repo, &files)?;
        super::check_tags(&repo, &incoming.tags, head)?;
        install_tags(&repo, &incoming.tags)?;
        repository::publish_ref(&repo, None, head)?;
        repo.remote("origin", &request.origin)?;
        drop(repo);
        let source =
            checkpoints::portable::GitSource::open(&stage.path().join(checkpoints::DIRECTORY))?;
        crate::store::portable::import(&source, destination, |_, _, _| {})
            .map_err(|error| failure(error.to_string()))?;
        let repo = checkpoints::open(destination)?;
        super::state::write_receipt(&repo, request.origin.clone(), head, &incoming.tags)?;
        Ok(ClonedProject {
            path: destination.to_string_lossy().into_owned(),
        })
    }
}
