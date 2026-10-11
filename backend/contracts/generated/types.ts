// SPDX-License-Identifier: MIT
// See backend/contracts/LICENSE.
// Generated from Rust; do not edit.
// Run npm run contracts:generate from the repository root.

export type ActivityDay = { date: string, notesTouched: number, wordsAdded: number, wordsDeleted: number, netWordChange: number, editingEvents: number, tasksCompleted: number, };
export type AddPropertyOptionsRequest = { key: string, valueType: MetadataFieldValueType, options: Array<string>, };
export type AnalyticsRequest = { startDate?: string | null, endDate?: string | null, query?: string | null, };
export type AnalyzeQueryRequest = { query: string, };
export type ApproveRequest = { jobId: string, hash: string, };
export type Approved = { trusted: boolean, };
export type ArchiveSnippetRequest = { id: string, revision: string, };
export type ArticlePlan = { pageId: string, pageRevision: string, title: string, outsideLinks: OutsideLinks, theme: ThemeSnapshot, assets: Array<PlannedAsset>, links: Array<PlannedLink>, snippets: Array<PlannedSnippet>, };
export type CacheStatus = { structuralState: string, pending: number, documents: number, schemaVersion: number, schemaFingerprint: string, reconciliationTimings?: ReconciliationTimings, lastReconciledAt?: number, error?: string, };
export type CalendarBucket = { date: string, count: number, };
export type CalendarGranularity = "day" | "month" | "year";
export type CalendarRequest = { query?: string | null, field: string, startDate: string, endDate: string, granularity: CalendarGranularity, pinScopeId?: string | null, };
export type CalendarResults = { buckets: Array<CalendarBucket>, };
export type CalendarZoom = "month" | "week";
export type Cancelled = { cancelled: boolean, };
export type CardFieldLayout = "inline" | "stacked" | "body";
export type CreateItemRequest = { meta?: { [key in string]: OpaqueJson }, content?: { [key in string]: OpaqueJson }, tags?: Array<string>, pins?: Array<string>, tasks?: Array<NoteTask>, };
export type CreatePropertyRequest = { key: string, valueType: MetadataFieldValueType, options?: Array<string>, };
export type CreateSnippetRequest = { id: string, paragraph: ReferenceParagraph, };
export type CreateWritingGoalRequest = { scope: GoalScope, startDate: string, endDate: string, dailyWordTarget: number, totalWordTarget?: number, viewId?: string, title?: string, };
export type DashboardAnalytics = { days: Array<ActivityDay>, trendStart: string, summary: DashboardSummary, };
export type DashboardDayDetails = { date: string, items: Array<DashboardDayItem>, };
export type DashboardDayItem = { id: string, title: string, wordsAdded: number, wordsDeleted: number, netWordChange: number, editingEvents: number, tasksCompleted: number, };
export type DashboardSummary = { activeDays: number, wordsAdded: number, wordsDeleted: number, netWordChange: number, editingEvents: number, tasksCompleted: number, currentStreak: number, bestStreak: number, };
export type DayDetailsRequest = { date: string, query?: string | null, };
export type DeleteItemRequest = { id: string, };
export type DeleteResult = { deleted: boolean, };
export type EmptyRequest = Record<symbol, never>;
export type ExportRequest = PageExportInput<NoteSource, NoteFormat> | PageExportInput<LinkedSource, LinkedFormat> | PublicationExportInput;
export type FacetCount = { key: string, count: number, };
export type FacetCountsRequest = { kind: FacetKind, keys?: Array<string> | null, sourceType?: string | null, };
export type FacetEntry = { key: string, kind: FacetKind, usageCount: number, inferred: boolean, };
export type FacetKind = "tag" | "label";
export type FacetRequest = { kind: FacetKind, key: string, };
export type FieldSource = "registered" | "inferred" | "builtin";
export type GoalScope = "all_notes" | "view";
export type HtmlTarget = { outsideLinks: OutsideLinks, };
export type ImageFit = "cover" | "fit";
export type ItemState = "active" | "archived";
export type ItemType = "page" | "workspace" | "goal" | "widget" | "query" | "setting" | "publication";
export type JobId = { jobId: string, };
export type JobRequest = { jobId: string, };
export type JsonValue = number | string | boolean | Array<JsonValue> | { [key in string]: JsonValue } | null;
export type LinkAttributes = { href: string, };
export type LinkResolution = "included" | "text_only" | "external" | "anchor";
export type LinkedFormat = "pdf" | "site";
export type LinkedSource = { "kind": "linked-pages", id: string, };
export type ListFacetsRequest = { kind?: FacetKind | null, };
export type Metadata = { state?: ItemState, type?: ItemType, title?: string, summary?: string, image?: string, color?: string, propertyKeys?: Array<string>, lang?: string, } & ({ [key in string]: unknown });
export type MetadataField = { key: string, label: string, valueType: MetadataFieldValueType, source: FieldSource, system?: boolean, options?: Array<string>, };
export type MetadataFieldValueType = "text" | "url" | "email" | "tel" | "number" | "checkbox" | "date" | "dateTime" | "timestamp" | "select" | "multiSelect" | "language" | "color" | "image" | "rating";
export type MutateNoteTaskRequest = { noteId: string, version: string, change: TaskChange, };
export type NoteFormat = "pdf" | "html";
export type NotePdfPlan = { pageId: string, pageRevision: string, title: string, profile: PdfProfile, outsideLinks: OutsideLinks, theme: ThemeSnapshot, assets: Array<PlannedAsset>, links: Array<PlannedLink>, snippets: Array<PlannedSnippet>, };
export type NoteSource = { "kind": "note", id: string, };
export type NoteTask = { id: string, text: string, createdAt: string, completedAt?: string, };
export type OpaqueJson = unknown;
export type OutlineNode = { "kind": "page", pageId: string, children: Array<OutlineNode>, showInNavigation?: boolean, slug?: string | null, } | { "kind": "section", id: string, title: string, introPageId?: string | null, children: Array<OutlineNode>, } | { "kind": "collection", id: string, title: string, query: string, showPagesInNavigation?: boolean, listingSlug?: string | null, pageSize?: number, feed?: boolean, archives?: boolean, };
export type OutsideLinks = "error" | "text-only";
export type PageExportInput<S, F> = { source: S, format: F, start: boolean, themeId: string, outsideLinks: OutsideLinks, profile: PdfProfile, };
export type ParagraphKind = "paragraph";
export type PassageRecord = { itemId: string, blockId: string, revision: string, noteTitle: string, labels: Array<string>, text: string, fragment: { [key in string]: OpaqueJson }, truncated: boolean, };
export type PdfProfile = "default" | "accessible";
export type PdfTarget = { profile: PdfProfile, outsideLinks: OutsideLinks, };
export type PinResult = { pinned: boolean, version: string, };
export type Plan = { "kind": "article", "value": ArticlePlan } | { "kind": "note_pdf", "value": NotePdfPlan } | { "kind": "publication", "value": PublicationPlan };
export type PlannedAsset = { src: string, ownerId: string, filename: string, bytes: number, sha256: string, };
export type PlannedCollection = { id: string, title: string, query: string, 
/**
 * Raw query membership before selection rules. Used to detect a stale
 * dynamic collection without repeating the complete Publication plan.
 */
resolvedIds: Array<string>, pageIds: Array<string>, listingSlug?: string, pageSize: number, feed: boolean, archives: boolean, };
export type PlannedLink = { pageId: string, blockId?: string, href: string, resolution: LinkResolution, targetId?: string, destination?: string, };
export type PlannedListing = { route: string, basePath: string, title: string, collectionId: string, pageIds: Array<string>, pageNumber: number, totalPages: number, };
export type PlannedPage = { id: string, revision: string, title: string, route: string, publishedOn?: string, tags: Array<string>, depth: number, excludedBlockIds: Array<string>, parentId?: string, };
export type PlannedSnippet = { id: string, revision: string, paragraph: JsonValue, };
export type PlannedStructureNode = { "kind": "page", pageId: string, showInNavigation: boolean, slug?: string, children: Array<PlannedStructureNode>, } | { "kind": "section", id: string, title: string, introPageId?: string, children: Array<PlannedStructureNode>, } | { "kind": "collection", id: string, title: string, pageIds: Array<string>, showPagesInNavigation: boolean, listingSlug?: string, pageSize: number, feed: boolean, archives: boolean, };
export type PreviewUrl = { url: string, };
export type PublicItem = { id: string, version: string, createdAt: string, modifiedAt: string, meta: Metadata, content: { [key in string]: OpaqueJson }, tags?: Array<string>, pins?: Array<string>, tasks?: Array<NoteTask>, preview?: string, previewImage?: string, };
export type PublicationContent = { kind: PublicationKind, preset: PublicationPreset, selectedOutput: PublicationOutput, rootPageId: string, outline: Array<OutlineNode>, selectionRules: Array<SelectionRule>, themeId: string, themeSettings?: ThemeSettingsOverride, themeOptions?: { [key in string]: ThemeOptionValue }, targets: PublicationTargets, };
export type PublicationExportInput = { source: PublicationSource, format: PublicationFormat, start: boolean, };
export type PublicationFormat = "pdf" | "html" | "site";
export type PublicationKind = "publication";
export type PublicationOutput = "pdf" | "html" | "site";
export type PublicationPlan = { sourceKind: PublicationSourceKind, publicationId: string, publicationRevision: string, title: string, format: PublicationFormat, themeId: string, theme: ThemeSnapshot, rootPageId: string, structure: Array<PlannedStructureNode>, pages: Array<PlannedPage>, collections: Array<PlannedCollection>, listings: Array<PlannedListing>, assets: Array<PlannedAsset>, links: Array<PlannedLink>, snippets: Array<PlannedSnippet>, selectionRules: Array<SelectionRule>, targets: PublicationTargets, };
export type PublicationPreset = "book" | "documentation" | "blog" | "website";
export type PublicationSource = { "kind": "publication", id: string, };
export type PublicationSourceKind = "publication" | "linked_pages";
export type PublicationTargets = { pdf: PdfTarget, html: HtmlTarget, site: SiteTarget, };
export type QueryAnalysis = { languageVersion: number, valid: boolean, diagnostics: Array<QueryDiagnostic>, };
export type QueryDiagnostic = { code: string, message: string, severity: string, start: number, end: number, };
export type QueryItemsRequest = { query: string, options?: ReadOptions, limit?: number | null, offset?: number | null, pinScopeId?: string | null, };
export type QueryItemsResult = { items: Array<PublicItem>, count: number, pinnedCount?: number, };
export type QueryPassagesRequest = { label: string, limit?: number | null, offset?: number | null, };
export type QuerySnippetsRequest = { query?: string | null, limit?: number | null, offset?: number | null, };
export type QueryTasksRequest = { query?: string | null, noteId?: string | null, status: TaskStatus, limit?: number | null, offset?: number | null, };
export type ReadItemRequest = { id: string, options?: ReadOptions, };
export type ReadItemsRequest = { ids?: Array<string> | null, query?: string | null, options?: ReadOptions, limit?: number | null, offset?: number | null, };
export type ReadOptions = { content?: boolean | null, };
export type ReadRequest = { jobId: string, includePlan?: boolean, };
export type ReadSnippetRequest = { id: string, };
export type ReadThemeRequest = { themeId: string, format: string, };
export type ReconciliationTimings = { walkMs: number, pinScopeMs: number, fileStampMs: number, indexedStampMs: number, diffMs: number, };
export type ReferenceMark = { type: ReferenceMarkKind, attrs?: LinkAttributes, };
export type ReferenceMarkKind = "strong" | "emphasis" | "underline" | "strike" | "code" | "link" | "superscript" | "subscript";
export type ReferenceParagraph = { type: ParagraphKind, content?: Array<ReferenceText>, };
export type ReferenceText = { type: TextKind, text: string, marks?: Array<ReferenceMark>, };
export type RemovePropertyRequest = { key: string, };
export type Removed = { removed: boolean, };
export type ResultPage<T> = { items: Array<T>, count: number, };
export type SelectionEffect = "include" | "exclude";
export type SelectionMatcher = { kind: SelectionMatcherKind, value: string, };
export type SelectionMatcherKind = "block_type" | "block_label" | "page_tag";
export type SelectionRule = { id: string, effect: SelectionEffect, target: SelectionTarget, matcher: SelectionMatcher, };
export type SelectionTarget = "page" | "block" | "section";
export type SetItemTagsRequest = { id: string, version: string, tags: Array<string>, };
export type SetNotePinRequest = { noteId: string, version: string, pinned: boolean, scopeId?: string | null, };
export type SiteLayout = "documentation" | "website";
export type SiteTarget = { layout: SiteLayout, outsideLinks: OutsideLinks, baseUrl?: string, };
export type SnippetHit = { usageCount: number, format: string, formatVersion: number, id: string, state: SnippetState, revision: string, createdAt: string, modifiedAt: string, paragraph: ReferenceParagraph, };
export type SnippetOccurrence = { kind: SnippetOccurrenceKind, blockId?: string, };
export type SnippetOccurrenceKind = "citation" | "embed";
export type SnippetRecord = { format: string, formatVersion: number, id: string, state: SnippetState, revision: string, createdAt: string, modifiedAt: string, paragraph: ReferenceParagraph, };
export type SnippetResults = { items: Array<SnippetHit>, count: number, };
export type SnippetState = "active" | "archived";
export type SnippetUsage = { noteId: string, noteTitle: string, revision: string, occurrences: Array<SnippetOccurrence>, };
export type State = "planning" | "prepared" | "awaiting_theme_approval" | "queued" | "rendering" | "ready" | "failed" | "cancelled";
export type Status = { state: State, phase: string, format: PublicationFormat, plan?: Plan, summary?: Summary, themeTrust?: ThemeTrust, artifactUrl?: string, bundleUrl?: string, previewUrl?: string, error?: string, };
export type Summary = { title: string, pages: number, collections: number, assets: number, links: number, };
export type TaskChange = { "kind": "create", text: string, } | { "kind": "edit", taskId: string, text: string, } | { "kind": "complete", taskId: string, } | { "kind": "reopen", taskId: string, } | { "kind": "delete", taskId: string, };
export type TaskRecord = { noteId: string, taskId: string, revision: string, noteTitle: string, text: string, createdAt: string, completedAt: string | null, dueDate: string | null, modifiedAt: string, };
export type TaskStatus = "open" | "completed";
export type TextKind = "text";
export type ThemeColors = { ink: string, muted: string, link: string, accent: string, surface: string, paper: string, };
export type ThemeColorsOverride = { ink?: string, muted?: string, link?: string, accent?: string, surface?: string, paper?: string, };
export type ThemeControl = { id: string, label: string, kind: ThemeControlKind, default: ThemeOptionValue, choices: Array<string>, };
export type ThemeControlKind = "toggle" | "choice";
export type ThemeLayout = { measureCh: number, spacing: number, };
export type ThemeLayoutOverride = { measureCh?: number, spacing?: number, };
export type ThemeOptionValue = boolean | string;
export type ThemeOrigin = "builtin" | "project";
export type ThemeSettings = { colors: ThemeColors, typography: ThemeTypography, layout: ThemeLayout, };
export type ThemeSettingsOverride = { colors?: ThemeColorsOverride, typography?: ThemeTypographyOverride, layout?: ThemeLayoutOverride, };
export type ThemeSnapshot = { id: string, version: string, name: string, origin: ThemeOrigin, sha256: string, settings: ThemeSettings, controls: Array<ThemeControl>, options: { [key in string]: ThemeOptionValue }, };
export type ThemeTrust = { hash: string, trusted: boolean, root: string, };
export type ThemeTypography = { bodyFamily: string, headingFamily: string, bodyScale: number, leading: number, };
export type ThemeTypographyOverride = { bodyFamily?: string, headingFamily?: string, bodyScale?: number, leading?: number, };
export type UpdateDefaultViewRequest = { content: ViewContent, };
export type UpdateItemMetaRequest = { id: string, version: string, meta: { [key in string]: OpaqueJson }, options?: ReadOptions, };
export type UpdateItemRequest = { id: string, version: string, meta: { [key in string]: OpaqueJson }, content: { [key in string]: OpaqueJson }, tags?: Array<string>, pins?: Array<string>, tasks?: Array<NoteTask>, };
export type UpdateSnippetRequest = { id: string, revision: string, paragraph: ReferenceParagraph, };
export type Uploaded = { src: string, filename: string, contentType: string, size: number, };
export type ViewCalendarPresentation = { dateField?: string, initialZoom?: CalendarZoom, };
export type ViewCardField = { field: string, label?: string, hidden?: boolean, maxLines?: number, layout?: CardFieldLayout, maxItems?: number, };
export type ViewCardPresentation = { fields?: Array<ViewCardField>, showMedia?: boolean, mediaHeight?: number, imageFit?: ImageFit, titleLines?: number, };
export type ViewContent = { mode?: ViewPresentationMode, query?: string, cards?: ViewCardPresentation, table?: ViewTablePresentation, kanban?: ViewKanbanPresentation, calendar?: ViewCalendarPresentation, } & ({ [key in string]: unknown });
export type ViewKanbanColumn = { id: string, hidden?: boolean, };
export type ViewKanbanPresentation = { groupBy?: string, columns?: Array<ViewKanbanColumn>, };
export type ViewPresentationMode = "cards" | "table" | "kanban" | "dashboard";
export type ViewTableColumn = { field: string, label?: string, hidden?: boolean, width?: number, };
export type ViewTablePresentation = { columns?: Array<ViewTableColumn>, };
export type WritingGoalContent = { kind: WritingGoalKind, scope: GoalScope, viewId?: string, query?: string, startDate: string, endDate: string, dailyWordTarget: number, totalWordTarget?: number, };
export type WritingGoalKind = "writing_challenge";

export type Api = {
  add_property_options: { method: 'POST'; request: AddPropertyOptionsRequest; response: MetadataField }
  analyze_query: { method: 'POST'; request: AnalyzeQueryRequest; response: QueryAnalysis }
  approve_export_theme: { method: 'POST'; request: ApproveRequest; response: Approved }
  archive_snippet: { method: 'POST'; request: ArchiveSnippetRequest; response: SnippetRecord }
  cache_status: { method: 'GET'; request: undefined; response: CacheStatus }
  cancel_export: { method: 'POST'; request: JobRequest; response: Cancelled }
  create_export: { method: 'POST'; request: ExportRequest; response: JobId }
  create_facet: { method: 'POST'; request: FacetRequest; response: FacetEntry }
  create_item: { method: 'POST'; request: CreateItemRequest; response: PublicItem }
  create_property: { method: 'POST'; request: CreatePropertyRequest; response: MetadataField }
  create_snippet: { method: 'POST'; request: CreateSnippetRequest; response: SnippetRecord }
  create_writing_goal: { method: 'POST'; request: CreateWritingGoalRequest; response: PublicItem }
  delete_item: { method: 'POST'; request: DeleteItemRequest; response: DeleteResult }
  list_facets: { method: 'POST'; request: ListFacetsRequest; response: Array<FacetEntry> }
  mutate_note_task: { method: 'POST'; request: MutateNoteTaskRequest; response: PublicItem }
  query_items: { method: 'POST'; request: QueryItemsRequest; response: QueryItemsResult }
  query_label_passages: { method: 'POST'; request: QueryPassagesRequest; response: ResultPage<PassageRecord> }
  query_note_tasks: { method: 'POST'; request: QueryTasksRequest; response: ResultPage<TaskRecord> }
  query_snippets: { method: 'POST'; request: QuerySnippetsRequest; response: SnippetResults }
  read_calendar_buckets: { method: 'POST'; request: CalendarRequest; response: CalendarResults }
  read_dashboard_analytics: { method: 'POST'; request: AnalyticsRequest; response: DashboardAnalytics }
  read_dashboard_day_details: { method: 'POST'; request: DayDetailsRequest; response: DashboardDayDetails }
  read_default_view: { method: 'POST'; request: EmptyRequest; response: ViewContent }
  read_export: { method: 'POST'; request: ReadRequest; response: Status }
  read_facet_counts: { method: 'POST'; request: FacetCountsRequest; response: Array<FacetCount> }
  read_item: { method: 'POST'; request: ReadItemRequest; response: PublicItem }
  read_items: { method: 'POST'; request: ReadItemsRequest; response: Array<PublicItem> }
  read_metadata_fields: { method: 'POST'; request: EmptyRequest; response: Array<MetadataField> }
  read_publication_theme: { method: 'POST'; request: ReadThemeRequest; response: ThemeSnapshot }
  read_snippet: { method: 'POST'; request: ReadSnippetRequest; response: SnippetRecord | null }
  read_snippet_usages: { method: 'POST'; request: ReadSnippetRequest; response: Array<SnippetUsage> }
  remove_facet: { method: 'POST'; request: FacetRequest; response: Removed }
  remove_property: { method: 'POST'; request: RemovePropertyRequest; response: Removed }
  set_item_tags: { method: 'POST'; request: SetItemTagsRequest; response: PublicItem }
  set_note_pin: { method: 'POST'; request: SetNotePinRequest; response: PinResult }
  start_export: { method: 'POST'; request: JobRequest; response: JobId }
  start_export_preview: { method: 'POST'; request: JobRequest; response: PreviewUrl }
  update_default_view: { method: 'POST'; request: UpdateDefaultViewRequest; response: ViewContent }
  update_item: { method: 'POST'; request: UpdateItemRequest; response: PublicItem }
  update_item_meta: { method: 'POST'; request: UpdateItemMetaRequest; response: PublicItem }
  update_snippet: { method: 'POST'; request: UpdateSnippetRequest; response: SnippetRecord }
}
