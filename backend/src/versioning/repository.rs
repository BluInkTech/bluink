// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023â€“2026 BluInk
use super::{Result, failure};
use crate::domain::project::identity::ItemId;
use crate::domain::publication::theme;
use git2::{Oid, Repository, Tree};
use std::collections::BTreeMap;

pub(super) const BRANCH: &str = "refs/heads/main";
pub(super) const IGNORE: &[u8] = b"/.cache/\n";
pub(super) const ATTRIBUTES: &[u8] = b"* text=auto eol=lf\nassets/** filter=lfs diff=lfs merge=lfs -text\nthemes/** filter=lfs diff=lfs merge=lfs -text\nthemes/**/*.json -filter diff merge text eol=lf\nthemes/**/*.html -filter diff merge text eol=lf\nthemes/**/*.css -filter diff merge text eol=lf\nthemes/**/*.js -filter diff merge text eol=lf\nthemes/**/*.typ -filter diff merge text eol=lf\nthemes/**/*.svg -filter diff merge text eol=lf\nthemes/**/*.txt -filter diff merge text eol=lf\nthemes/**/*.md -filter diff merge text eol=lf\nthemes/**/*.j2 -filter diff merge text eol=lf\nthemes/**/*.jinja -filter diff merge text eol=lf\nthemes/**/*.minijinja -filter diff merge text eol=lf\n";

pub(super) fn head(repo: &Repository) -> Result<Option<Oid>> {
    match repo.refname_to_id(BRANCH) {
        Ok(id) => Ok(Some(id)),
        Err(error)
            if matches!(
                error.code(),
                git2::ErrorCode::NotFound | git2::ErrorCode::UnbornBranch
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) fn publish_ref(repo: &Repository, previous: Option<Oid>, next: Oid) -> Result<()> {
    if let Some(previous) = previous {
        repo.reference_matching(BRANCH, next, true, previous, "Bluink checkpoint")?;
    } else {
        repo.reference(BRANCH, next, false, "Initial Bluink checkpoint")?;
    }
    Ok(())
}

pub(super) fn timestamp(commit: &git2::Commit<'_>) -> Result<String> {
    Ok(chrono::DateTime::from_timestamp(commit.time().seconds(), 0)
        .ok_or_else(|| failure("Invalid checkpoint timestamp"))?
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

#[derive(Default)]
struct Changes {
    files: BTreeMap<String, Option<Oid>>,
    directories: BTreeMap<String, Self>,
}

impl Changes {
    fn add(&mut self, path: &str, oid: Option<Oid>) {
        if let Some((directory, rest)) = path.split_once('/') {
            self.directories
                .entry(directory.to_owned())
                .or_default()
                .add(rest, oid);
        } else {
            self.files.insert(path.to_owned(), oid);
        }
    }

    fn write(
        &self,
        repo: &Repository,
        previous: Option<&Tree<'_>>,
        record: &mut impl FnMut(Oid) -> Result<()>,
    ) -> Result<Oid> {
        let mut builder = repo.treebuilder(previous)?;
        for (name, oid) in &self.files {
            if let Some(oid) = oid {
                builder.insert(name, *oid, 0o100_644)?;
            } else if builder.get(name)?.is_some() {
                builder.remove(name)?;
            }
        }
        for (name, changes) in &self.directories {
            let old = previous
                .and_then(|tree| tree.get_name(name))
                .map(|entry| repo.find_tree(entry.id()))
                .transpose()?;
            let oid = changes.write(repo, old.as_ref(), record)?;
            if repo.find_tree(oid)?.is_empty() {
                if builder.get(name)?.is_some() {
                    builder.remove(name)?;
                }
            } else {
                builder.insert(name, oid, 0o040_000)?;
            }
        }
        let oid = builder.write()?;
        record(oid)?;
        Ok(oid)
    }
}

pub(super) fn changed_tree(
    repo: &Repository,
    previous: Option<Oid>,
    files: &BTreeMap<String, Option<Oid>>,
) -> Result<Oid> {
    changed_tree_recording(repo, previous, files, &mut |_| Ok(()))
}

pub(super) fn changed_tree_recording(
    repo: &Repository,
    previous: Option<Oid>,
    files: &BTreeMap<String, Option<Oid>>,
    record: &mut impl FnMut(Oid) -> Result<()>,
) -> Result<Oid> {
    let mut changes = Changes::default();
    for (path, oid) in files {
        changes.add(path, *oid);
    }
    let previous = previous
        .map(|id| repo.find_commit(id).and_then(|commit| commit.tree()))
        .transpose()?;
    changes.write(repo, previous.as_ref(), record)
}

pub(super) fn files(repo: &Repository, commit: Oid) -> Result<BTreeMap<String, Oid>> {
    fn visit(
        repo: &Repository,
        tree: &Tree<'_>,
        prefix: &str,
        output: &mut BTreeMap<String, Oid>,
        depth: usize,
    ) -> Result<()> {
        if depth > 16 {
            return Err(failure("History tree is too deeply nested"));
        }
        for entry in tree {
            let name = entry.name()?;
            let path = format!("{prefix}{name}");
            if entry.kind() == Some(git2::ObjectType::Tree) {
                visit(
                    repo,
                    &repo.find_tree(entry.id())?,
                    &format!("{path}/"),
                    output,
                    depth + 1,
                )?;
            } else {
                if tracked(&path).is_none() || entry.filemode() != 0o100_644 {
                    return Err(failure(format!(
                        "History contains an unsupported path: {path}"
                    )));
                }
                output.insert(path, entry.id());
            }
        }
        Ok(())
    }
    let mut output = BTreeMap::new();
    visit(repo, &repo.find_commit(commit)?.tree()?, "", &mut output, 0)?;
    Ok(output)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Json,
    Text,
    Binary,
}

pub(super) fn tracked(path: &str) -> Option<Kind> {
    if path.split('/').any(|part| {
        part.is_empty()
            || part == ".."
            || part == "."
            || part.contains([':', '\\'])
            || part.chars().any(char::is_control)
    }) {
        return None;
    }
    let parts: Vec<_> = path.split('/').collect();
    match parts.as_slice() {
        ["bluink.json"] => Some(Kind::Json),
        [".gitignore" | ".gitattributes"] => Some(Kind::Text),
        ["items", bucket, filename] => {
            let id = ItemId::try_from(filename.strip_suffix(".json")?).ok()?;
            (id.bucket() == *bucket).then_some(Kind::Json)
        }
        ["assets", bucket, id, filename] => {
            let id = ItemId::try_from(*id).ok()?;
            (id.bucket() == *bucket && crate::domain::project::asset::valid_filename(filename))
                .then_some(Kind::Binary)
        }
        ["themes", id, files @ ..]
            if theme::is_theme_id(id)
                && !theme::is_builtin(id)
                && theme::safe_theme_path(&files.join("/")) =>
        {
            let text = [
                "json",
                "html",
                "css",
                "js",
                "txt",
                "md",
                "typ",
                "svg",
                "j2",
                "jinja",
                "minijinja",
            ]
            .iter()
            .any(|extension| path.ends_with(&format!(".{extension}")));
            Some(if text { Kind::Text } else { Kind::Binary })
        }
        _ => None,
    }
}
