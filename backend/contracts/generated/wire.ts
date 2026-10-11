// SPDX-License-Identifier: MIT
// See backend/contracts/LICENSE.
// Generated from Rust; do not edit.
// Run npm run contracts:generate from the repository root.

import type { Api, PublicItem, OutlineNode, TaskRecord, ThemeSnapshot } from './types.js'

export const query_items = {"limit":null,"query":"#fiction"} satisfies Api['query_items']['request']
export const read_item = {"id":"00000000-0000-7000-8000-000000000501"} satisfies Api['read_item']['request']
export const read_items = {"ids":["00000000-0000-7000-8000-000000000501"]} satisfies Api['read_items']['request']
export const create_item = {} satisfies Api['create_item']['request']
export const read_export = {"jobId":"job"} satisfies Api['read_export']['request']
export const mutate_note_task = {"change":{"kind":"complete","taskId":"task-1"},"noteId":"00000000-0000-7000-8000-000000000501","version":"revision-1"} satisfies Api['mutate_note_task']['request']
export const create_export = {"format":"pdf","outsideLinks":"error","profile":"default","source":{"id":"00000000-0000-7000-8000-000000000501","kind":"note"},"start":false,"themeId":"classic"} satisfies Api['create_export']['request']
export const item = {"content":{"content":{"content":[{"type":"paragraph"}],"type":"doc"}},"createdAt":"2026-10-10T12:00:00Z","id":"00000000-0000-7000-8000-000000000501","meta":{"custom":42,"title":"Example","type":"page"},"modifiedAt":"2026-10-10T12:00:00Z","version":"revision-1"} satisfies PublicItem
export const task = {"completedAt":null,"createdAt":"2026-10-10T12:00:00Z","dueDate":null,"modifiedAt":"2026-10-10T12:00:00Z","noteId":"00000000-0000-7000-8000-000000000501","noteTitle":"Example","revision":"revision-1","taskId":"task-1","text":"Read the chapter"} satisfies TaskRecord
export const outline = {"children":[],"kind":"page","pageId":"00000000-0000-7000-8000-000000000501"} satisfies OutlineNode
export const theme = {"controls":[{"choices":[],"default":true,"id":"sidebar","kind":"toggle","label":"Show site navigation"}],"id":"classic","name":"Classic","options":{"sidebar":true},"origin":"builtin","settings":{"colors":{"accent":"#174aaf","ink":"#222222","link":"#174aaf","muted":"#555b66","paper":"#ffffff","surface":"#f2f4f8"},"layout":{"measureCh":72,"spacing":1.0},"typography":{"bodyFamily":"source-serif-4","bodyScale":1.0,"headingFamily":"source-serif-4","leading":1.6}},"sha256":"92e04974f31ccecc613f91b593532e9dc6d535def3937de897e3d26dd31ae4cb","version":"1.0.0"} satisfies ThemeSnapshot
