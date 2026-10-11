// Compile-only regressions. An unused @ts-expect-error makes the contract check fail.
import type { Api, PublicItem, TaskRecord } from './generated/types.js'
import { rpcMethods, ItemType, ItemState, FacetKind } from './generated/runtime.js'
import { item, task, create_export } from './generated/wire.js'

const methods: { [K in keyof Api]: Api[K]['method'] } = rpcMethods
const page: PublicItem['meta']['type'] = ItemType.PAGE
const state: PublicItem['meta']['state'] = ItemState.ACTIVE
const facet: Api['create_facet']['request']['kind'] = FacetKind.TAG

// @ts-expect-error The query is required.
const missingQuery: Api['query_items']['request'] = {}
// @ts-expect-error Optional response tags are omitted, never null.
const nullTags: PublicItem = { ...item, tags: null }
// @ts-expect-error Timestamps are present on every saved item.
const missingTimestamps: PublicItem = { id: item.id, version: item.version, meta: {}, content: {} }
// @ts-expect-error Task dates are present and nullable, not optional.
const missingTaskDates: TaskRecord = { ...task, completedAt: undefined }
// @ts-expect-error Canonical revision is not the public optimistic-lock token.
const revision: Api['update_item_meta']['request'] = { id: item.id, revision: 'r', meta: {} }
// @ts-expect-error Version is mandatory even though it is removed before canonical persistence.
const missingVersion: Api['update_item']['request'] = { id: item.id, meta: {}, content: {} }
// @ts-expect-error Single notes cannot generate a multipage site.
const noteSite: Api['create_export']['request'] = { ...create_export, format: 'site' }
// @ts-expect-error Task operations are a discriminated union with required task IDs.
const taskChange: Api['mutate_note_task']['request'] = { noteId: item.id, version: item.version, change: { kind: 'complete' } }
// @ts-expect-error A facet must be a tag or label.
const invalidFacet: Api['create_facet']['request'] = { kind: 'snippet', key: 'example' }
// @ts-expect-error Unsupported formats are rejected.
const word: Api['create_export']['request'] = { source: { kind: 'publication', id: item.id }, format: 'docx', start: false }

void [methods, page, state, facet, missingQuery, nullTags, missingTimestamps, missingTaskDates, revision, missingVersion, noteSite, taskChange, invalidFacet, word]
