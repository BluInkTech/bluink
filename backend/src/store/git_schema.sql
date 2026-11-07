-- SPDX-License-Identifier: MPL-2.0
-- Canonical portable mappings and durable cross-store checkpoint recovery.

CREATE TABLE project_record (
    path TEXT PRIMARY KEY,
    canonical_json TEXT NOT NULL CHECK (json_valid(canonical_json)),
    revision TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation >= 0)
) STRICT, WITHOUT ROWID;

CREATE TABLE asset (
    oid TEXT PRIMARY KEY CHECK (length(oid) = 64 AND oid NOT GLOB '*[^0-9a-f]*'),
    size INTEGER NOT NULL CHECK (size >= 0),
    local_available INTEGER NOT NULL CHECK (local_available IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE TABLE asset_path (
    path TEXT PRIMARY KEY,
    owner_id INTEGER REFERENCES entity(id),
    oid TEXT NOT NULL REFERENCES asset(oid),
    generation INTEGER NOT NULL CHECK (generation >= 0)
) STRICT, WITHOUT ROWID;
CREATE INDEX asset_path_by_owner ON asset_path(owner_id);
CREATE INDEX asset_path_by_object ON asset_path(oid);

CREATE TABLE dirty_path (
    path TEXT PRIMARY KEY,
    generation INTEGER NOT NULL CHECK (generation >= 0),
    operation TEXT NOT NULL CHECK (operation IN ('upsert', 'delete')),
    source_kind TEXT NOT NULL CHECK (source_kind IN ('item', 'record', 'asset')),
    item_id INTEGER REFERENCES entity(id),
    revision_uuid TEXT,
    CHECK ((source_kind = 'item' AND operation = 'upsert') =
           (item_id IS NOT NULL AND revision_uuid IS NOT NULL))
) STRICT, WITHOUT ROWID;
CREATE INDEX dirty_path_by_generation ON dirty_path(generation, path);
CREATE INDEX dirty_path_by_item ON dirty_path(item_id);

CREATE TABLE capture (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL DEFAULT 'checkpoint' CHECK (kind IN ('checkpoint', 'export', 'backup')),
    expected_head TEXT,
    generation INTEGER NOT NULL CHECK (generation >= 0),
    created_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE capture_item (
    capture_id TEXT NOT NULL REFERENCES capture(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    generation INTEGER NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('upsert', 'delete')),
    source_kind TEXT NOT NULL CHECK (source_kind IN ('item', 'record', 'asset')),
    -- An item is read back through its pinned exact revision. Project records
    -- keep no retained revisions, so their small JSON is frozen here.
    item_id INTEGER REFERENCES entity(id),
    revision_uuid TEXT,
    record_json TEXT CHECK (record_json IS NULL OR json_valid(record_json)),
    object_oid TEXT REFERENCES asset(oid),
    PRIMARY KEY (capture_id, path),
    CHECK (operation = 'delete' OR
       (source_kind = 'item' AND item_id IS NOT NULL AND revision_uuid IS NOT NULL) OR
       (source_kind = 'record' AND record_json IS NOT NULL) OR
       (source_kind = 'asset' AND object_oid IS NOT NULL))
) STRICT, WITHOUT ROWID;
CREATE INDEX capture_item_by_item ON capture_item(item_id);
CREATE INDEX capture_item_by_object ON capture_item(object_oid);

CREATE TABLE object_pin (
    owner_kind TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    object_kind TEXT NOT NULL CHECK (object_kind IN ('git', 'lfs')),
    object_id TEXT NOT NULL,
    PRIMARY KEY (owner_kind, owner_id, object_kind, object_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX object_pin_by_object ON object_pin(object_kind, object_id);

CREATE TRIGGER release_render_pins AFTER DELETE ON render_lease BEGIN
    DELETE FROM revision_pin WHERE owner_kind='render' AND owner_id=old.id;
    DELETE FROM object_pin WHERE owner_kind='render' AND owner_id=old.id;
END;

CREATE TABLE git_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    expected_head TEXT
) STRICT;
INSERT INTO git_state VALUES (1, NULL);

-- A row exists only while its ref publication still needs acknowledgement.
CREATE TABLE git_operation (
    id TEXT PRIMARY KEY,
    capture_id TEXT REFERENCES capture(id),
    expected_head TEXT,
    candidate_head TEXT NOT NULL,
    recovery_json TEXT NOT NULL CHECK (json_valid(recovery_json))
) STRICT, WITHOUT ROWID;
CREATE INDEX git_operation_by_capture ON git_operation(capture_id);

-- Rebuildable projections, covered by one exact visible head and named-ref digest.
CREATE TABLE history_commit (
    oid TEXT PRIMARY KEY,
    committed_seconds INTEGER NOT NULL,
    message TEXT NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE history_item (
    item_uuid TEXT NOT NULL,
    commit_oid TEXT NOT NULL REFERENCES history_commit(oid) ON DELETE CASCADE,
    PRIMARY KEY (item_uuid, commit_oid)
) STRICT, WITHOUT ROWID;
CREATE INDEX history_item_by_commit ON history_item(commit_oid, item_uuid);

CREATE TABLE history_name (
    ref_name TEXT PRIMARY KEY,
    tag_oid TEXT NOT NULL,
    version_id TEXT NOT NULL,
    title TEXT NOT NULL,
    item_uuid TEXT,
    commit_oid TEXT NOT NULL REFERENCES history_commit(oid),
    validator_version INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
CREATE INDEX history_name_by_item ON history_name(item_uuid, commit_oid);
CREATE INDEX history_name_by_commit ON history_name(commit_oid);

CREATE TABLE history_reachable (
    commit_oid TEXT PRIMARY KEY REFERENCES history_commit(oid),
    position INTEGER NOT NULL UNIQUE
) STRICT, WITHOUT ROWID;

CREATE TABLE history_coverage (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    head_oid TEXT,
    refs_digest TEXT NOT NULL,
    validator_version INTEGER NOT NULL
) STRICT;

-- Immutable Git pointer inventories. Coverage applies only after every parent
-- is covered. Empty deltas explicitly prove prose-only commits need no LFS batch.
CREATE TABLE git_inventory_commit (
    oid TEXT PRIMARY KEY,
    parents_json TEXT NOT NULL CHECK(json_valid(parents_json) AND json_type(parents_json)='array'),
    validator_version INTEGER NOT NULL,
    ancestry_covered INTEGER NOT NULL CHECK(ancestry_covered IN (0,1))
) STRICT, WITHOUT ROWID;
CREATE TABLE git_asset_delta (
    commit_oid TEXT NOT NULL REFERENCES git_inventory_commit(oid) ON DELETE CASCADE,
    object_oid TEXT NOT NULL CHECK(length(object_oid)=64),
    size INTEGER NOT NULL CHECK(size>=0),
    PRIMARY KEY(commit_oid,object_oid)
) STRICT, WITHOUT ROWID;
CREATE INDEX git_asset_delta_by_object ON git_asset_delta(object_oid,commit_oid);
CREATE TABLE git_tree_inventory (
    tree_oid TEXT NOT NULL,
    path_prefix TEXT NOT NULL,
    validator_version INTEGER NOT NULL,
    pointers_json TEXT NOT NULL CHECK(json_valid(pointers_json) AND json_type(pointers_json)='array'),
    PRIMARY KEY(tree_oid,path_prefix)
) STRICT, WITHOUT ROWID;
