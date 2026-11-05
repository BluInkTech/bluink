// SPDX-License-Identifier: MPL-2.0
//! An acknowledged page restore changes only its item, direct attachments and
//! current owned tasks when archiving. Check the page's dependencies in SQLite.
use super::{
    BTreeMap, History, ItemId, Kind, Oid, Path, Repository, Result, Value, failure, json, project,
    repository,
};
use crate::store::{self, item, mutations, relations};

type Files = BTreeMap<String, Oid>;
type Destination = (Files, Files, Vec<String>);

fn item_path(id: &ItemId) -> String {
    id.path(Path::new("")).to_string_lossy().replace('\\', "/")
}

fn entry(tree: &git2::Tree<'_>, path: &str) -> Result<Option<git2::TreeEntry<'static>>> {
    match tree.get_path(Path::new(path)) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn page_files(repo: &Repository, root: &git2::Tree<'_>, id: &ItemId) -> Result<Files> {
    let mut files = Files::new();
    let path = item_path(id);
    if let Some(value) = entry(root, &path)? {
        if value.filemode() != 0o100_644 {
            return Err(failure("Page history must contain a regular file"));
        }
        files.insert(path, value.id());
    }
    let prefix = id
        .asset_directory(Path::new(""))
        .to_string_lossy()
        .replace('\\', "/");
    if let Some(directory) = entry(root, &prefix)? {
        for file in &repo.find_tree(directory.id())? {
            let path = format!("{prefix}/{}", file.name()?);
            if file.filemode() != 0o100_644 || repository::tracked(&path) != Some(Kind::Binary) {
                return Err(failure("Invalid page attachment in history"));
            }
            files.insert(path, file.id());
        }
    }
    Ok(files)
}

pub(super) fn destination(
    store: &History,
    repo: &Repository,
    head: Oid,
    selected: Oid,
    generation: i64,
    id: &str,
) -> Result<Destination> {
    let identity = ItemId::try_from(id)?;
    let path = item_path(&identity);
    let current_tree = repo.find_commit(head)?.tree()?;
    let mut current = page_files(repo, &current_tree, &identity)?;
    let mut target = page_files(repo, &repo.find_commit(selected)?.tree()?, &identity)?;
    let selected = target
        .get(&path)
        .ok_or_else(|| failure("This checkpoint does not contain the page"))?;
    let page: Value = serde_json::from_slice(repo.find_blob(*selected)?.content())?;
    project::item::validate_item(&page, id)?;
    if page["meta"]["type"] != "page" {
        return Err(failure("Choose a page version to restore"));
    }
    let dependencies = project::candidate::dependencies(&page)?;
    let tasks = store
        .canonical()?
        .read(|tx| {
            let observed: i64 = tx.query_row(
                "SELECT generation FROM project_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            if observed != generation {
                return Err(store::Error::RevisionConflict);
            }
            if let Some(existing) = item::read_header(tx, id)?
                && existing.value["meta"]["type"] != "page"
            {
                return Err(store::Error::Invalid(
                    "An item's type cannot be changed.".into(),
                ));
            }
            relations::validate_dependencies(tx, &page, &dependencies)?;
            if page["meta"]["state"] == "active" {
                Ok(Vec::new())
            } else {
                mutations::active_tasks(tx, id)
            }
        })
        .map_err(|error| failure(error.to_string()))?;
    let mut archived = Vec::with_capacity(tasks.len());
    for (task, revision) in tasks {
        let path = item_path(&ItemId::try_from(task.as_str())?);
        let oid = entry(&current_tree, &path)?
            .ok_or_else(|| failure("Owned task is missing from the checkpoint"))?
            .id();
        let mut value: Value = serde_json::from_slice(repo.find_blob(oid)?.content())?;
        if value["revision"] != revision
            || value["content"]["noteId"] != id
            || value["meta"]["type"] != "task"
        {
            return Err(failure("Owned task changed since the checkpoint"));
        }
        value["meta"]["state"] = json!("archived");
        current.insert(path.clone(), oid);
        target.insert(path, repo.blob(&project::canonical_json::to_vec(&value)?)?);
        archived.push(task);
    }
    Ok((current, target, archived))
}
