// SPDX-License-Identifier: MPL-2.0
use super::*;
use crate::store::{
    git,
    item::{self, ExpectedRevision, PreparedItem},
    receipts::{self, Submission},
};
use serde_json::{Value, json};
use std::io::{Seek, Write};
use uuid::Uuid;

fn seed(store: &Store, root: &Path) -> (Value, git2::Oid, String) {
    let value=crate::store::commands::assemble_item(&json!({
        "meta":{"type":"page","title":"Checkpointed page"},
        "content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"a"},"content":[{"type":"text","text":"Original text"}]}]}}
    }),None,"UTC").unwrap();
    let prepared = PreparedItem::new(&value).unwrap();
    let copy = value.clone();
    checkpoints::initialize(root).unwrap();
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(b"Independent backup attachment").unwrap();
    file.rewind().unwrap();
    let asset = checkpoints::install_asset(root, &mut file).unwrap();
    let oid = asset.oid().to_owned();
    store.write(move |tx| {
        let id=prepared.install(tx,ExpectedRevision::New)?;
        tx.execute("UPDATE project_state SET generation=1",[])?;
        git::mark_item_dirty(tx,copy["id"].as_str().unwrap(),copy["revision"].as_str().unwrap(),1)?;
        let project:String=tx.query_row("SELECT project_uuid FROM project_state",[],|row|row.get(0))?;
        git::put_record(tx,"bluink.json",&json!({"format":"bluink-project","formatVersion":20,"id":project,"defaultWorkspaceId":Uuid::now_v7().to_string(),"analytics":{"timezone":"UTC","eventRetentionDays":14}}),&Uuid::now_v7().to_string(),1)?;
        let identity=crate::domain::project::identity::ItemId::try_from(copy["id"].as_str().unwrap()).unwrap();
        let path=identity.asset_directory(Path::new("")).join("attachment.txt").to_string_lossy().replace('\\',"/");
        git::put_asset(tx,&path,Some(id),&asset,1)?;
        Ok(())
    }).unwrap();
    let head = crate::versioning::test_support::checkpoint(root, store, "device", "Backup test")
        .unwrap()
        .unwrap();
    (value, head, oid)
}

fn object(root: &Path, oid: &str) -> std::path::PathBuf {
    root.join(checkpoints::DIRECTORY)
        .join("lfs/objects")
        .join(&oid[..2])
        .join(&oid[2..4])
        .join(oid)
}

#[test]
fn full_backup_restores_uncheckpointed_save_receipt_capture_and_assets_without_source() {
    let source = tempfile::tempdir().unwrap();
    let outputs = tempfile::tempdir().unwrap();
    let store = Store::create(source.path(), Uuid::now_v7()).unwrap();
    let (value, head, oid) = seed(&store, source.path());
    let uuid = value["id"].as_str().unwrap().to_owned();
    let expected=crate::store::commands::assemble_item(&json!({"id":uuid,"meta":{"type":"page","title":"Unsynced work"},"content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"a"},"content":[{"type":"text","text":"Private new text"}]}]}}}),Some(&value),"UTC").unwrap();
    let prepared = PreparedItem::new(&expected).unwrap();
    let base = value["revision"].as_str().unwrap().to_owned();
    let accepted = expected["revision"].as_str().unwrap().to_owned();
    let saved_uuid = uuid.clone();
    store
        .write(move |tx| {
            let id = prepared.install(tx, ExpectedRevision::Exact(&base))?;
            tx.execute("UPDATE project_state SET generation=2", [])?;
            git::mark_item_dirty(tx, &saved_uuid, &accepted, 2)?;
            receipts::complete(
                tx,
                &Submission {
                    item: id,
                    device: "offline",
                    id: "lost-ack",
                    hash: "request-hash",
                    base: &base,
                },
                &accepted,
                &json!({"accepted":accepted}),
            )?;
            git::capture(tx, &Uuid::now_v7().to_string(), Some(&head.to_string()), 2)?;
            Ok(())
        })
        .unwrap();
    let recovery = format!("refs/bluink/recovery/{head}");
    checkpoints::open(source.path())
        .unwrap()
        .reference(&recovery, head, false, "Retained tip")
        .unwrap();
    let destination = outputs.path().join("backup");
    let backup = create(&store, &destination).unwrap();
    assert_eq!(backup.snapshot.generation, 2);
    assert_eq!(backup.history.lfs_objects, 1);
    assert!(!store.read(maintenance::protected).unwrap());
    drop(store);
    source.close().unwrap();
    let restored = outputs.path().join("restored");
    restore(&destination, &restored).unwrap();
    let store = Store::open(&restored).unwrap();
    store
        .read(|tx| {
            assert_eq!(item::read(tx, &uuid, true)?.unwrap(), expected);
            assert_eq!(
                tx.query_row::<i64, _, _>("SELECT count(*) FROM submission", [], |row| row.get(0))?,
                1
            );
            assert_eq!(
                tx.query_row::<i64, _, _>(
                    "SELECT count(*) FROM capture WHERE generation=2",
                    [],
                    |row| row.get(0)
                )?,
                1
            );
            assert!(!maintenance::protected(tx)?);
            Ok(())
        })
        .unwrap();
    let repo = checkpoints::open(&restored).unwrap();
    assert_eq!(repo.head().unwrap().target(), Some(head));
    assert_eq!(repo.find_reference(&recovery).unwrap().target(), Some(head));
    assert_eq!(
        fs::read(object(&restored, &oid)).unwrap(),
        b"Independent backup attachment"
    );
    assert!(!repo.path().join("objects/info/alternates").exists());
    let sentinel = restored.join("keep.txt");
    fs::write(&sentinel, "keep").unwrap();
    assert!(restore(&destination, &restored).is_err());
    assert_eq!(fs::read_to_string(sentinel).unwrap(), "keep");
}

#[test]
fn missing_required_attachment_releases_pin_and_never_publishes_backup_manifest() {
    let root = tempfile::tempdir().unwrap();
    let outputs = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let (_, _, oid) = seed(&store, root.path());
    fs::remove_file(object(root.path(), &oid)).unwrap();
    let destination = outputs.path().join("incomplete");
    assert!(create(&store, &destination).is_err());
    assert!(!destination.join(MANIFEST).exists());
    // A leftover pin would stop all retention and packing indefinitely.
    assert!(!store.read(maintenance::protected).unwrap());
}

#[test]
fn altered_snapshot_is_rejected_before_restore_creates_destination() {
    let root = tempfile::tempdir().unwrap();
    let outputs = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    seed(&store, root.path());
    let destination = outputs.path().join("backup");
    create(&store, &destination).unwrap();
    let mut database = OpenOptions::new()
        .append(true)
        .open(destination.join(DATABASE_NAME))
        .unwrap();
    database.write_all(b"altered").unwrap();
    drop(database);
    let restored = outputs.path().join("restored");
    assert!(
        restore(&destination, &restored)
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
    assert!(!restored.exists());
}
