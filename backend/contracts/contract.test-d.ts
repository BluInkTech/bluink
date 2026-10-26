// Compile-only regressions. An unused @ts-expect-error makes the contract check fail.
import type { Api, PublicItem, TaskRecord, PublicationThemePatch, PublicationThemeToken, PublicationThemeWebControl, QueryDocument } from './generated/types.js'
import { rpcMethods, ItemType, ItemState, FacetKind } from './generated/runtime.js'
import { item, task, create_export, portableTheme } from './generated/wire.js'

const methods: { [K in keyof Api]: Api[K]['method'] } = rpcMethods
// @ts-expect-error Activity is a destination; Dashboard is not a collection presentation.
const obsoleteCollection: import('./generated/types.js').ViewPresentationMode = 'dashboard'
const scopedPassages: Api['query_label_passages']['request'] = { label: 'source', pinScopeId: item.id }
// @ts-expect-error Scope is a workspace ID string, never a nested workspace object.
const invalidPassageScope: Api['query_label_passages']['request'] = { label: 'source', pinScopeId: { id: item.id } }
const page: PublicItem['meta']['type'] = ItemType.PAGE
const state: PublicItem['meta']['state'] = ItemState.ACTIVE
const facet: Api['create_facet']['request']['kind'] = FacetKind.TAG

// @ts-expect-error The query is required.
const missingQuery: Api['query_items']['request'] = {}
const structuredQuery: QueryDocument = { version: 1, clauses: [{ polarity: 'excluded', predicate: { kind: 'field', field: 'priority', operator: 'gte', value: '3' } }], sort: [] }
// @ts-expect-error Structured validation does not accept raw query text.
const rawQuery: Api['validate_query']['request'] = { document: '#fiction' }
// @ts-expect-error Operators are typed choices, not query-language punctuation.
const invalidOperator: QueryDocument['clauses'][number]['predicate'] = { kind: 'field', field: 'priority', operator: '>=', value: '3' }
// @ts-expect-error UI identities do not belong in the executable condition.
const editorIdentity: QueryDocument['clauses'][number] = { polarity: 'required', predicate: { kind: 'tag', key: 'fiction' }, id: 'editor-1' }
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
const word: Api['create_export']['request'] = { source: { kind: 'publication', id: item.id }, format: 'docx', start: false }
// @ts-expect-error Unsupported formats are rejected.
const unsupported: Api['create_export']['request'] = { source: { kind: 'publication', id: item.id }, format: 'mobi', start: false }

const sparseTheme: PublicationThemePatch = { page: { marginTop: '1in', header: { showOnFirstPage: false } }, styles: { heading1: { fontSize: { ref: 'heading-size' } } } }
// @ts-expect-error Theme page overrides have known property names.
const misspelledMargin: PublicationThemePatch = { page: { margnTop: '1in' } }
// @ts-expect-error Named token types constrain literal values.
const invalidToken: PublicationThemeToken = { type: 'number', value: 'twelve' }
// @ts-expect-error Choice controls require choices.
const incompleteControl: PublicationThemeWebControl = { kind: 'choice', id: 'layout', label: 'Layout', default: 'wide' }
// @ts-expect-error Theme updates must carry the captured package revision.
const missingThemeRevision: Api['update_publication_theme']['request'] = { themeId: 'custom', definition: portableTheme }
// @ts-expect-error Cloning a theme requires a fresh display name and a revision.
const incompleteThemeClone: Api['duplicate_publication_theme']['request'] = { themeId: 'classic' }
// @ts-expect-error Draft previews must choose a supported production target.
const unsupportedThemePreview: Api['preview_publication_theme']['request'] = { themeId: 'classic', revision: 'r', definition: portableTheme, format: 'mobi', start: true }

void [methods, page, state, facet, missingQuery, nullTags, missingTimestamps, missingTaskDates, revision, missingVersion, noteSite, taskChange, invalidFacet, word, unsupported, sparseTheme, misspelledMargin, invalidToken, incompleteControl, missingThemeRevision, incompleteThemeClone, unsupportedThemePreview]
void [structuredQuery, rawQuery, invalidOperator, editorIdentity]
void [obsoleteCollection, scopedPassages, invalidPassageScope]
