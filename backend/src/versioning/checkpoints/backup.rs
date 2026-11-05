// SPDX-License-Identifier: MPL-2.0
//! Copy immutable objects independently; refs are the snapshot's acknowledged refs,
//! never a newer live main tip observed after the SQLite snapshot was taken.
use git2::{Oid, Repository};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::Path,
};

use super::{DIRECTORY, initialize, open};
use crate::{
    store::io::{self, LockMode},
    versioning::{Result, failure, lfs, repository, transport},
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryCopy {
    pub(crate) git_objects: usize,
    pub(crate) lfs_objects: usize,
    pub(crate) unavailable_history: Vec<String>,
}

fn sql<T>(value: rusqlite::Result<T>) -> Result<T> {
    value.map_err(|error| failure(error.to_string()))
}

pub(crate) fn copy_pinned_history(
    source: &Path,
    destination: &Path,
    snapshot: &Connection,
) -> Result<HistoryCopy> {
    let protected = sql(snapshot.query_row(
        "SELECT EXISTS(SELECT 1 FROM maintenance_pin)",
        [],
        |row| row.get::<_, bool>(0),
    ))?;
    if !protected {
        return Err(failure("A full backup requires a durable maintenance pin"));
    }
    if destination.join(DIRECTORY).try_exists()? {
        return Err(failure("Backup history destination already exists"));
    }
    let from = open(source)?;
    let _lease = io::lock_file(&from.path().join("bluink.lock"), LockMode::Exclusive)
        .map_err(|error| failure(format!("{error:#}")))?;
    initialize(destination)?;
    let to = open(destination)?;
    let git_objects = crate::versioning::packing::copy_objects(&from, &to)?;
    let mut references = snapshot_refs(snapshot)?;
    // Retained divergent tips are write-once and live only as refs. The lease
    // keeps new ones from appearing after the objects were copied.
    for reference in from.references_glob("refs/bluink/recovery/*")? {
        let reference = reference?;
        if let Some(oid) = reference.target() {
            references.insert(reference.name()?.to_owned(), oid);
        }
    }
    let mut report = HistoryCopy {
        git_objects,
        lfs_objects: 0,
        unavailable_history: Vec::new(),
    };
    copy_attachments(&from, &to, snapshot, &references, &mut report)?;
    // Ref publication follows object verification; no destination is published to
    // the user until its caller verifies and atomically publishes the full backup.
    for (name, oid) in references {
        to.find_object(oid, None)?;
        to.reference(&name, oid, false, "Bluink full backup")?;
    }
    if let Ok(remote) = from.find_remote("origin") {
        to.remote("origin", remote.url()?)?;
    }
    verify_project(&to, snapshot)?;
    Ok(report)
}

fn snapshot_refs(snapshot: &Connection) -> Result<BTreeMap<String, Oid>> {
    let head: Option<String> = sql(snapshot.query_row(
        "SELECT expected_head FROM git_state WHERE singleton=1",
        [],
        |row| row.get(0),
    ))?;
    let mut references = BTreeMap::new();
    if let Some(head) = head {
        references.insert(repository::BRANCH.to_owned(), Oid::from_str(&head)?);
    }
    let mut names =
        sql(snapshot.prepare("SELECT ref_name,tag_oid FROM history_name ORDER BY ref_name"))?;
    for row in sql(names.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }))? {
        let (reference, oid) = sql(row)?;
        references.insert(reference, Oid::from_str(&oid)?);
    }
    let mut pins =
        sql(snapshot.prepare("SELECT object_id FROM object_pin WHERE object_kind='git'"))?;
    for row in sql(pins.query_map([], |row| row.get::<_, String>(0)))? {
        let oid = Oid::from_str(&sql(row)?)?;
        references.insert(format!("refs/bluink/backup-pins/{oid}"), oid);
    }
    Ok(references)
}

fn copy_attachments(
    from: &Repository,
    to: &Repository,
    snapshot: &Connection,
    references: &BTreeMap<String, Oid>,
    report: &mut HistoryCopy,
) -> Result<()> {
    let heads = references
        .values()
        .copied()
        .filter(|oid| {
            from.find_object(*oid, None)
                .and_then(|object| object.peel_to_commit())
                .is_ok()
        })
        .collect::<Vec<_>>();
    let available = match super::inventory::from_snapshot(from, snapshot, &heads)? {
        Some(pointers) => pointers,
        None => transport::local_inventory(from, &heads)?,
    };
    let mut pointers = available
        .into_iter()
        .map(|pointer| (pointer.identity().0.to_owned(), pointer))
        .collect::<BTreeMap<_, _>>();
    let mut required = BTreeSet::new();
    let mut assets = sql(snapshot.prepare(
        "SELECT a.oid,a.size,a.local_available,
        EXISTS(SELECT 1 FROM asset_path p WHERE p.oid=a.oid) OR
        EXISTS(SELECT 1 FROM capture_item c WHERE c.object_oid=a.oid) OR
        EXISTS(SELECT 1 FROM object_pin p WHERE p.object_kind='lfs' AND p.object_id=a.oid)
        FROM asset a",
    ))?;
    for row in sql(assets.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, bool>(2)?,
            row.get::<_, bool>(3)?,
        ))
    }))? {
        let (oid, size, available, pinned) = sql(row)?;
        if available || pinned {
            required.insert(oid.clone());
        }
        pointers.insert(
            oid.clone(),
            lfs::Pointer::from_identity(
                &oid,
                u64::try_from(size).map_err(|_| failure("Invalid backup attachment size"))?,
            )?,
        );
    }
    let mut pinned =
        sql(snapshot.prepare("SELECT object_id FROM object_pin WHERE object_kind='lfs'"))?;
    for oid in sql(pinned.query_map([], |row| row.get::<_, String>(0)))? {
        let oid = sql(oid)?;
        if !pointers.contains_key(&oid) {
            return Err(failure(format!(
                "Pinned attachment metadata is missing: {oid}"
            )));
        }
        required.insert(oid);
    }
    for (oid, pointer) in pointers {
        if let Some(path) = pointer.available_path(from.path())? {
            lfs::receive(to.path(), &pointer, &mut File::open(path)?)?;
            report.lfs_objects += 1;
        } else if required.contains(&oid) {
            return Err(failure(format!(
                "Required backup attachment is missing: {oid}"
            )));
        } else {
            report.unavailable_history.push(oid);
        }
    }
    Ok(())
}

fn verify_project(to: &Repository, snapshot: &Connection) -> Result<()> {
    let expected_project = sql(snapshot
        .query_row("SELECT project_uuid FROM project_state", [], |row| {
            row.get::<_, String>(0)
        })
        .optional())?
    .ok_or_else(|| failure("Snapshot project identity is missing"))?;
    if let Some(head) = repository::head(to)? {
        let entry = to
            .find_commit(head)?
            .tree()?
            .get_path(Path::new("bluink.json"))?;
        let config = crate::domain::project::config::parse_project_config(
            to.find_blob(entry.id())?.content(),
        )?;
        if config["id"].as_str() != Some(expected_project.as_str()) {
            return Err(failure("Backup history belongs to another project"));
        }
    }
    Ok(())
}
