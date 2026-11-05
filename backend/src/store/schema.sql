-- SPDX-License-Identifier: MPL-2.0
-- Copyright (c) 2023–2026 BluInk

PRAGMA foreign_keys = ON;

CREATE TABLE project_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    project_uuid TEXT NOT NULL UNIQUE,
    generation INTEGER NOT NULL CHECK (generation >= 0),
    change_epoch TEXT NOT NULL,
    change_floor INTEGER NOT NULL CHECK (change_floor >= 0),
    manifest_generation INTEGER NOT NULL CHECK (manifest_generation >= 0),
    projector_version INTEGER NOT NULL,
    CHECK (change_floor <= generation)
) STRICT;

-- Survives process death. Backup/migration recovery explicitly releases protection.
CREATE TABLE maintenance_pin (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('backup', 'migration', 'export')),
    destination TEXT NOT NULL,
    created_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE maintenance_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    last_block_sweep_ms INTEGER NOT NULL
) STRICT;
INSERT INTO maintenance_state VALUES (1, CAST(unixepoch('subsec') * 1000 AS INTEGER));

CREATE TABLE render_lease (id TEXT PRIMARY KEY, expires_ms INTEGER NOT NULL) STRICT, WITHOUT ROWID;
CREATE INDEX render_lease_expiry ON render_lease(expires_ms);

CREATE TABLE entity (
    id INTEGER PRIMARY KEY,
    uuid TEXT NOT NULL UNIQUE,
    named_kind TEXT,
    named_key TEXT,
    CHECK ((named_kind IS NULL) = (named_key IS NULL)),
    UNIQUE (named_kind, named_key)
) STRICT;

CREATE TABLE block (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    digest BLOB NOT NULL UNIQUE CHECK (length(digest) = 32),
    document_json TEXT NOT NULL
        CHECK (json_valid(document_json) AND json_type(document_json) = 'object'),
    stable_id TEXT GENERATED ALWAYS AS (json_extract(document_json, '$.attrs.id')) STORED NOT NULL
) STRICT;

-- Resolve patch references without fetching unchanged subtree payloads.
CREATE INDEX block_identity ON block(id, stable_id);

-- Derived from an immutable subtree once. Subsequent sparse saves can read
-- its facts without fetching or decoding the author's document_json.
CREATE TABLE block_fact (
    block_id INTEGER PRIMARY KEY REFERENCES block(id) ON DELETE CASCADE,
    facts_json TEXT NOT NULL CHECK (json_valid(facts_json))
) STRICT;

CREATE TABLE body (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    content_json TEXT NOT NULL CHECK (json_valid(content_json)),
    block_ids BLOB,
    installed_generation INTEGER,
    CHECK (
        (block_ids IS NULL AND installed_generation IS NULL)
        OR
        (block_ids IS NOT NULL AND installed_generation IS NOT NULL
         AND installed_generation > 0)
    )
) STRICT;

CREATE INDEX body_by_installation
    ON body(installed_generation, id)
    WHERE installed_generation IS NOT NULL;

CREATE TABLE item (
    id INTEGER PRIMARY KEY REFERENCES entity(id),
    header_json TEXT NOT NULL
        CHECK (json_valid(header_json) AND json_type(header_json) = 'object'),
    body_id INTEGER NOT NULL REFERENCES body(id),
    uuid TEXT COLLATE BINARY GENERATED ALWAYS AS
        (json_extract(header_json, '$.id')) STORED NOT NULL UNIQUE,
    revision TEXT GENERATED ALWAYS AS
        (json_extract(header_json, '$.revision')) STORED NOT NULL UNIQUE,
    type TEXT GENERATED ALWAYS AS
        (json_extract(header_json, '$.meta.type')) STORED NOT NULL,
    state TEXT GENERATED ALWAYS AS
        (json_extract(header_json, '$.meta.state')) STORED NOT NULL,
    title TEXT GENERATED ALWAYS AS
        (coalesce(json_extract(header_json, '$.meta.title'), '')) STORED NOT NULL,
    title_sort TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    modified_ms INTEGER NOT NULL,
    CHECK (json_type(header_json, '$.id') IS 'text'),
    CHECK (json_type(header_json, '$.revision') IS 'text'),
    CHECK (json_type(header_json, '$.meta.type') IS 'text'),
    CHECK (json_type(header_json, '$.meta.state') IS 'text')
) STRICT;

CREATE INDEX item_by_modified ON item(type, state, modified_ms DESC, uuid ASC);
CREATE INDEX item_by_title ON item(type, state, title_sort ASC, uuid ASC);
CREATE INDEX item_by_body ON item(body_id);
-- Query membership joins by local ID before page-sized headers are hydrated.
CREATE INDEX item_query_header
    ON item(id, type, state, uuid, modified_ms, created_ms, title_sort);

CREATE TABLE revision (
    item_id INTEGER NOT NULL REFERENCES entity(id),
    revision_uuid TEXT NOT NULL,
    header_json TEXT NOT NULL CHECK (json_valid(header_json)),
    body_id INTEGER NOT NULL REFERENCES body(id),
    memberships_json TEXT NOT NULL CHECK (json_valid(memberships_json)),
    superseded_ms INTEGER NOT NULL,
    PRIMARY KEY (item_id, revision_uuid)
) STRICT, WITHOUT ROWID;

CREATE INDEX revision_by_expiry ON revision(superseded_ms);
CREATE INDEX revision_by_body ON revision(body_id);

CREATE TABLE revision_pin (
    owner_kind TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    item_id INTEGER NOT NULL REFERENCES entity(id),
    revision_uuid TEXT NOT NULL,
    expires_ms INTEGER,
    PRIMARY KEY (owner_kind, owner_id, item_id, revision_uuid)
) STRICT, WITHOUT ROWID;
CREATE INDEX revision_pin_by_revision ON revision_pin(item_id, revision_uuid);
CREATE INDEX revision_pin_by_expiry ON revision_pin(expires_ms) WHERE expires_ms IS NOT NULL;

CREATE TABLE submission (
    item_id INTEGER NOT NULL REFERENCES entity(id),
    device_id TEXT NOT NULL,
    submission_uuid TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    base_revision TEXT NOT NULL,
    accepted_revision TEXT NOT NULL,
    outcome_json TEXT NOT NULL CHECK (json_valid(outcome_json)),
    completed_ms INTEGER NOT NULL,
    PRIMARY KEY (item_id, device_id, submission_uuid)
) STRICT, WITHOUT ROWID;
CREATE INDEX submission_by_base ON submission(item_id, base_revision);
CREATE INDEX submission_by_accepted ON submission(item_id, accepted_revision);
CREATE INDEX submission_by_expiry ON submission(completed_ms);

CREATE TABLE activity_event (
    source_id INTEGER NOT NULL REFERENCES entity(id),
    revision_uuid TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('content', 'task_completed')),
    page_id INTEGER NOT NULL REFERENCES entity(id),
    occurred_ms INTEGER NOT NULL,
    day TEXT NOT NULL,
    words_added INTEGER NOT NULL CHECK (words_added >= 0),
    words_deleted INTEGER NOT NULL CHECK (words_deleted >= 0),
    PRIMARY KEY (source_id, revision_uuid, kind)
) STRICT, WITHOUT ROWID;
CREATE INDEX activity_event_by_expiry ON activity_event(occurred_ms);
CREATE INDEX activity_event_by_page ON activity_event(page_id, day);

CREATE TABLE activity_daily (
    day TEXT NOT NULL,
    page_id INTEGER NOT NULL REFERENCES entity(id),
    words_added INTEGER NOT NULL CHECK (words_added >= 0),
    words_deleted INTEGER NOT NULL CHECK (words_deleted >= 0),
    editing_events INTEGER NOT NULL CHECK (editing_events >= 0),
    tasks_completed INTEGER NOT NULL CHECK (tasks_completed >= 0),
    PRIMARY KEY (day, page_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX activity_daily_by_page ON activity_daily(page_id, day);

CREATE TABLE relation (
    kind TEXT NOT NULL,
    source_id INTEGER NOT NULL REFERENCES entity(id),
    target_id INTEGER NOT NULL REFERENCES entity(id),
    PRIMARY KEY (source_id, kind, target_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX relation_reverse ON relation(target_id, kind, source_id);

CREATE TABLE field_definition (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    value_type TEXT,
    declaration_id INTEGER REFERENCES entity(id),
    normalization_version INTEGER NOT NULL,
    CHECK ((value_type IS NULL) = (declaration_id IS NULL))
) STRICT;

CREATE INDEX field_definition_by_declaration ON field_definition(declaration_id);

CREATE TABLE field_option (
    field_id INTEGER NOT NULL REFERENCES field_definition(id),
    option_key TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    PRIMARY KEY (field_id, option_key),
    UNIQUE (field_id, position)
) STRICT, WITHOUT ROWID;

CREATE TABLE field_presence (
    item_id INTEGER NOT NULL REFERENCES item(id) ON DELETE CASCADE,
    field_id INTEGER NOT NULL REFERENCES field_definition(id),
    value_state TEXT NOT NULL CHECK (value_state IN ('null', 'empty', 'value')),
    observed_type TEXT CHECK (observed_type IN ('text','number','checkbox','multiSelect')),
    CHECK ((value_state = 'null') = (observed_type IS NULL)),
    PRIMARY KEY (item_id, field_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX field_presence_by_field
    ON field_presence(field_id, value_state, item_id);

CREATE INDEX field_presence_by_observation ON field_presence(field_id, observed_type) WHERE observed_type IS NOT NULL;

CREATE TABLE field_value (
    id INTEGER PRIMARY KEY,
    item_id INTEGER NOT NULL,
    field_id INTEGER NOT NULL,
    text_value TEXT,
    choice_sort TEXT CHECK (choice_sort IS NULL OR text_value IS NOT NULL),
    number_value REAL,
    time_value INTEGER,
    FOREIGN KEY (item_id, field_id)
        REFERENCES field_presence(item_id, field_id) ON DELETE CASCADE,
    CHECK (
        (text_value IS NOT NULL) + (number_value IS NOT NULL) = 1
        AND (time_value IS NULL OR text_value IS NOT NULL)
    )
) STRICT;

CREATE INDEX field_value_by_owner ON field_value(item_id, field_id);

CREATE UNIQUE INDEX field_value_text
    ON field_value(field_id, text_value, item_id)
    WHERE text_value IS NOT NULL;

CREATE UNIQUE INDEX field_value_number
    ON field_value(field_id, number_value, item_id)
    WHERE number_value IS NOT NULL;

CREATE INDEX field_value_time
    ON field_value(field_id, time_value, item_id)
    WHERE time_value IS NOT NULL;

CREATE TABLE field_token (
    field_id INTEGER NOT NULL REFERENCES field_definition(id),
    item_id INTEGER NOT NULL REFERENCES item(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    term TEXT NOT NULL,
    PRIMARY KEY (item_id, field_id, position, term),
    FOREIGN KEY (item_id, field_id)
        REFERENCES field_presence(item_id, field_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX field_token_lookup
    ON field_token(field_id, term, item_id, position);

CREATE TABLE item_projection (
    item_id INTEGER PRIMARY KEY REFERENCES item(id) ON DELETE CASCADE,
    preview TEXT NOT NULL,
    preview_image TEXT,
    analyzed_text_hash TEXT NOT NULL,
    page_links_json TEXT NOT NULL DEFAULT '[]' CHECK (json_type(page_links_json)='array'),
    word_count INTEGER NOT NULL CHECK (word_count >= 0),
    export_facts_json TEXT CHECK (
        export_facts_json IS NULL OR json_valid(export_facts_json)
    )
) STRICT;

CREATE TABLE passage (
    item_id INTEGER NOT NULL REFERENCES item(id) ON DELETE CASCADE,
    block_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    text TEXT NOT NULL,
    fragment_json TEXT NOT NULL CHECK (json_valid(fragment_json)),
    truncated INTEGER NOT NULL CHECK (truncated IN (0, 1)),
    PRIMARY KEY (item_id, block_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX passage_by_order ON passage(item_id, position, block_id);

CREATE TABLE passage_label (
    item_id INTEGER NOT NULL,
    block_id TEXT NOT NULL,
    label_id INTEGER NOT NULL REFERENCES entity(id),
    PRIMARY KEY (item_id, block_id, label_id),
    FOREIGN KEY (item_id, block_id)
        REFERENCES passage(item_id, block_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX passage_label_reverse ON passage_label(label_id, item_id, block_id);

CREATE TABLE snippet_projection (
    item_id INTEGER PRIMARY KEY REFERENCES item(id) ON DELETE CASCADE,
    search_text TEXT NOT NULL
) STRICT;

CREATE TABLE task_projection (
    item_id INTEGER PRIMARY KEY REFERENCES item(id) ON DELETE CASCADE,
    note_id INTEGER NOT NULL REFERENCES entity(id),
    text TEXT NOT NULL,
    completed_ms INTEGER,
    due_day INTEGER
) STRICT;

CREATE INDEX task_by_owner ON task_projection(note_id, completed_ms, item_id);
CREATE INDEX task_by_due ON task_projection(due_day, item_id);

CREATE TABLE decision_projection (
    item_id INTEGER PRIMARY KEY REFERENCES item(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    target_key TEXT NOT NULL,
    resolved INTEGER NOT NULL CHECK (resolved IN (0, 1)),
    created_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX decision_open
    ON decision_projection(created_ms, item_id) WHERE resolved = 0;

CREATE TABLE decision_target (
    decision_id INTEGER NOT NULL
        REFERENCES decision_projection(item_id) ON DELETE CASCADE,
    target_id INTEGER NOT NULL REFERENCES entity(id),
    PRIMARY KEY (decision_id, target_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX decision_target_reverse ON decision_target(target_id, decision_id);

CREATE VIRTUAL TABLE item_fts USING fts5(
    title,
    body,
    content = '',
    contentless_delete = 1,
    detail = full,
    columnsize = 1
);

-- Read-only positions over the same FTS postings for rare phrases with gaps.
CREATE VIRTUAL TABLE item_fts_terms USING fts5vocab(item_fts, 'instance');

-- Replacing a complete page makes its preceding segment mostly tombstones.
-- Batch that cleanup with normal merges to bound ordinary autosave WAL.
INSERT INTO item_fts(item_fts, rank) VALUES ('deletemerge', 0);
INSERT INTO item_fts(item_fts, rank) VALUES ('automerge', 24);
INSERT INTO item_fts(item_fts, rank) VALUES ('crisismerge', 48);

CREATE TABLE change_batch (
    generation INTEGER PRIMARY KEY,
    created_ms INTEGER NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('items', 'reset')),
    domains_json TEXT NOT NULL CHECK (
        json_valid(domains_json) AND json_type(domains_json) = 'array'
    )
) STRICT;

CREATE INDEX change_batch_by_age ON change_batch(created_ms, generation);

CREATE TABLE change_item (
    generation INTEGER NOT NULL
        REFERENCES change_batch(generation) ON DELETE CASCADE,
    item_uuid TEXT NOT NULL,
    item_type TEXT NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('upsert', 'delete')),
    revision_uuid TEXT,
    PRIMARY KEY (generation, item_uuid)
) STRICT, WITHOUT ROWID;
