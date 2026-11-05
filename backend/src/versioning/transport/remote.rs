// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::super::{Result, failure};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Remote {
    Local(PathBuf),
    Https(url::Url),
    Ssh(url::Url),
}

impl Remote {
    pub(crate) fn initialize(&self) -> Result<()> {
        let Self::Local(path) = self else {
            return Err(failure("Only local destinations can be created by Bluink"));
        };
        Self::initialize_local(path)?;
        Ok(())
    }
    pub(crate) fn parse(input: &str) -> Result<Self> {
        if input.is_empty() || input.chars().any(char::is_control) {
            return Err(failure("Choose a repository URL or absolute local path"));
        }
        if Path::new(input).is_absolute() {
            return Ok(Self::Local(PathBuf::from(input)));
        }
        let normalized;
        let input = if !input.contains("://") && input.contains('@') {
            let (host, path) = input
                .split_once(':')
                .ok_or_else(|| failure("Invalid SSH repository URL"))?;
            normalized = format!("ssh://{host}/{path}");
            normalized.as_str()
        } else {
            input
        };
        let url = url::Url::parse(input).map_err(|_| failure("Invalid repository URL"))?;
        if url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
            return Err(failure(
                "Repository URLs cannot contain secrets, queries or fragments",
            ));
        }
        match url.scheme() {
            "file" if url.username().is_empty() => Ok(Self::Local(
                url.to_file_path()
                    .map_err(|()| failure("Invalid local repository URL"))?,
            )),
            "https"
                if url.username().is_empty() && url.host_str().is_some() && url.path() != "/" =>
            {
                Ok(Self::Https(url))
            }
            "ssh"
                if url.host_str().is_some() && !url.username().is_empty() && url.path() != "/" =>
            {
                Ok(Self::Ssh(url))
            }
            _ => Err(failure(
                "Use an absolute local path, file://, https:// or SSH URL with a username",
            )),
        }
    }

    pub(crate) fn url(&self) -> Result<String> {
        Ok(match self {
            Self::Local(path) => path
                .to_str()
                .ok_or_else(|| failure("Repository path must be UTF-8"))?
                .to_owned(),
            Self::Https(url) | Self::Ssh(url) => url.to_string(),
        })
    }

    pub(crate) fn host_key(&self) -> Option<String> {
        match self {
            Self::Ssh(url) => Some(format!("{}:{}", url.host_str()?, url.port().unwrap_or(22))),
            _ => None,
        }
    }

    pub(crate) fn initialize_local(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(failure("Choose an absolute repository path"));
        }
        if path.try_exists()? && std::fs::read_dir(path)?.next().is_some() {
            return Err(failure("Create a destination in a new or empty folder"));
        }
        let mut options = git2::RepositoryInitOptions::new();
        options
            .bare(true)
            .initial_head("main")
            .external_template(false);
        git2::Repository::init_opts(path, &options)?;
        Ok(Self::Local(path.canonicalize()?))
    }

    pub(super) fn local_repository(&self) -> Result<git2::Repository> {
        let Self::Local(path) = self else {
            return Err(failure("Not a local destination"));
        };
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(failure(
                "Local destination must be a real bare repository directory",
            ));
        }
        let repo = git2::Repository::open_ext(
            path,
            git2::RepositoryOpenFlags::NO_SEARCH | git2::RepositoryOpenFlags::BARE,
            std::iter::empty::<&Path>(),
        )?;
        if !repo.is_bare() {
            return Err(failure("Local synchronization requires a bare repository"));
        }
        Ok(repo)
    }
}
