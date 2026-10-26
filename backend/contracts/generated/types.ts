// SPDX-License-Identifier: MIT
// See backend/contracts/LICENSE.
// Generated from Rust; do not edit.
// Run npm run contracts:generate from the repository root.

export type ActivityDay = { date: string, notesTouched: number, wordsAdded: number, wordsDeleted: number, netWordChange: number, editingEvents: number, tasksCompleted: number, };
export type AnalyticsRequest = { startDate?: string | null, endDate?: string | null, query?: QueryDocument | null, pinScopeId?: string | null, };
export type ApproveRequest = { jobId: string, hash: string, };
export type Approved = { trusted: boolean, };
export type ArchiveSnippetRequest = { id: string, revision: string, };
export type CacheStatus = { projectId: string, structuralState: string, pending: number, documents: number, schemaVersion: number, schemaFingerprint: string, reconciliationTimings?: ReconciliationTimings, lastReconciledAt?: number, error?: string, };
export type CalendarBucket = { date: string, count: number, };
export type CalendarGranularity = "day" | "month" | "year";
export type CalendarRequest = { query?: QueryDocument | null, field: string, startDate: string, endDate: string, granularity: CalendarGranularity, pinScopeId?: string | null, };
export type CalendarResults = { buckets: Array<CalendarBucket>, };
export type CalendarZoom = "month" | "week";
export type Cancelled = { cancelled: boolean, };
export type ChoiceAppearance = { color: ChoiceColor, };
export type ChoiceColor = "neutral" | "blue" | "green" | "amber" | "purple" | "rose" | "cyan";
export type Comparison = "match" | "gt" | "gte" | "lt" | "lte";
export type Condition = { polarity: Polarity, predicate: Predicate, };
export type ConditionDiagnostic = { path: string, code: string, message: string, start?: number, end?: number, };
export type CoverGradient = "dawn" | "sea" | "meadow" | "dusk" | "bloom" | "grove";
export type CreateItemRequest = { meta?: { [key in string]: OpaqueJson }, content?: { [key in string]: OpaqueJson }, tags?: Array<string>, pins?: Array<string>, };
export type CreatePropertyRequest = { key: string, valueType: MetadataFieldValueType, options?: Array<string>, };
export type CreatePublicationThemeRequest = { name: string, };
export type CreateSnippetRequest = { id: string, paragraph: ReferenceParagraph, };
export type CreateWorkspaceFromSetupRequest = { setup: OpaqueJson, title: string, };
export type CreateWritingGoalRequest = { scope: GoalScope, startDate: string, endDate: string, dailyWordTarget: number, totalWordTarget?: number, viewId?: string, title?: string, };
export type DashboardAnalytics = { days: Array<ActivityDay>, trendStart: string, summary: DashboardSummary, };
export type DashboardDayDetails = { date: string, items: Array<DashboardDayItem>, };
export type DashboardDayItem = { id: string, title: string, wordsAdded: number, wordsDeleted: number, netWordChange: number, editingEvents: number, tasksCompleted: number, };
export type DashboardSummary = { activeDays: number, wordsAdded: number, wordsDeleted: number, netWordChange: number, editingEvents: number, tasksCompleted: number, currentStreak: number, bestStreak: number, };
export type DayDetailsRequest = { date: string, query?: QueryDocument | null, pinScopeId?: string | null, };
export type DeleteItemRequest = { id: string, };
export type DeletePublicationThemeRequest = { themeId: string, revision: string, };
export type DeleteResult = { deleted: boolean, };
export type DocumentQueryRequest = { document: QueryDocument, pinScopeId?: string | null, };
export type DocxTarget = { outsideLinks: OutsideLinks, };
export type DuplicatePublicationThemeRequest = { themeId: string, revision: string, name: string, definition?: PublicationThemeDefinition, };
export type EmptyRequest = Record<symbol, never>;
export type ExportRequest = PageExportInput<NoteSource, NoteFormat> | PageExportInput<LinkedSource, LinkedFormat> | PublicationExportInput;
export type FacetCount = { key: string, count: number, };
export type FacetCountsRequest = { kind: FacetKind, keys?: Array<string> | null, sourceType?: string | null, };
export type FacetEntry = { key: string, kind: FacetKind, usageCount: number, inferred: boolean, };
export type FacetKind = "tag" | "label";
export type FacetRequest = { kind: FacetKind, key: string, };
export type FieldSource = "registered" | "inferred" | "builtin";
export type GoalScope = "all_notes" | "view";
export type HomePlacement = { span: WidgetSpan,
/**
 * The card's height; without one, the type's default height.
 */
height?: WidgetHeight,
/**
 * Kept off Home while keeping its place, for a widget reached from the top bar.
 */
hidden?: boolean, };
export type HtmlTarget = { outsideLinks: OutsideLinks, };
export type ItemCover = { "kind": "gradient", name: CoverGradient, } | { "kind": "image", url: string, position: number, };
export type ItemIcon = { "kind": "symbol", name: string, } | { "kind": "emoji", value: string, } | { "kind": "image", url: string, };
export type ItemState = "active" | "archived";
export type ItemType = "page" | "workspace" | "goal" | "widget" | "query" | "setting" | "publication" | "task" | "tag" | "label" | "property" | "snippet" | "template";
export type JobId = { jobId: string, };
export type JobRequest = { jobId: string, };
export type JsonValue = number | string | boolean | Array<JsonValue> | { [key in string]: JsonValue } | null;
export type LinkAttributes = { href: string, };
export type LinkResolution = "included" | "text_only" | "external" | "anchor";
export type LinkedFormat = "pdf" | "docx" | "epub" | "site";
export type LinkedSource = { "kind": "linked-pages", id: string, };
export type ListFacetsRequest = { kind?: FacetKind | null, };
export type Metadata = { state?: ItemState, type?: ItemType, title?: string, summary?: string, image?: string, color?: string, icon?: ItemIcon, cover?: ItemCover, propertyKeys?: Array<string>, lang?: string, } & ({ [key in string]: unknown });
export type MetadataField = { key: string, valueType: MetadataFieldValueType, source: FieldSource, system?: boolean, options?: Array<string>,
/**
 * How chosen options look; options without an entry are automatic.
 */
optionAppearance?: { [key in string]: ChoiceAppearance }, };
export type MetadataFieldValueType = "text" | "url" | "email" | "tel" | "number" | "checkbox" | "date" | "dateTime" | "timestamp" | "select" | "multiSelect" | "language" | "color" | "image" | "rating";
export type MutateNoteTaskRequest = { noteId: string, change: TaskChange, };
export type NoteFormat = "pdf" | "docx" | "epub" | "html";
export type NotePresentation = "list" | "cards";
export type NoteSource = { "kind": "note", id: string, };
export type OpaqueJson = unknown;
export type OutlineNode = { "kind": "page", pageId: string, children: Array<OutlineNode>, showInNavigation?: boolean, slug?: string | null, } | { "kind": "section", id: string, title: string, introPageId?: string | null, children: Array<OutlineNode>, } | { "kind": "collection", id: string, title: string, query: QueryDocument, showPagesInNavigation?: boolean, listingSlug?: string | null, pageSize?: number, feed?: boolean, archives?: boolean, };
export type OutsideLinks = "error" | "text-only";
export type PageExportInput<S, F> = { source: S, format: F, start: boolean, themeId: string, outsideLinks: OutsideLinks, profile: PdfProfile, };
export type PagePlan = { pageId: string, pageRevision: string, title: string, format: PublicationFormat, profile: PdfProfile, outsideLinks: OutsideLinks, theme: ThemeSnapshot, assets: Array<PlannedAsset>, links: Array<PlannedLink>, snippets: Array<PlannedSnippet>, };
export type ParagraphKind = "paragraph";
export type PassageRecord = { itemId: string, blockId: string, revision: string, noteTitle: string, labels: Array<string>, text: string, fragment: { [key in string]: OpaqueJson }, truncated: boolean, };
export type PdfProfile = "default" | "accessible";
export type PdfTarget = { profile: PdfProfile, outsideLinks: OutsideLinks, };
export type PinResult = { pinned: boolean, version: string, };
export type Plan = { "kind": "page", "value": PagePlan } | { "kind": "publication", "value": PublicationPlan };
export type PlannedAsset = { src: string, ownerId: string, filename: string, bytes: number, sha256: string, };
export type PlannedCollection = { id: string, title: string, query: QueryDocument,
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
export type Polarity = "required" | "excluded";
export type Predicate = { "kind": "text", value: string, mode: TextMode, } | { "kind": "tag", key: string, } | { "kind": "label", key: string, } | { "kind": "field", field: string, operator: Comparison, value: string, } | { "kind": "has", field: string, } | { "kind": "is", value: StatePredicate, };
export type PreviewPublicationThemeRequest = { themeId: string, revision: string, definition: PublicationThemeDefinition, format: PublicationFormat, start: boolean, };
export type PreviewUrl = { url: string, };
export type PublicItem = { id: string, version: string, createdAt: string, modifiedAt: string, meta: Metadata, content: { [key in string]: OpaqueJson }, tags?: Array<string>, pins?: Array<string>, preview?: string, previewImage?: string, };
export type PublicationContent = { kind: PublicationKind, preset: PublicationPreset, selectedOutput: PublicationOutput, rootPageId: string, outline: Array<OutlineNode>, selectionRules: Array<SelectionRule>, themeId: string, themeOverrides?: PublicationThemeOverrides, themeOptions?: { [key in string]: ThemeOptionValue }, targets: PublicationTargets, };
export type PublicationExportInput = { source: PublicationSource, format: PublicationFormat, start: boolean, };
export type PublicationFormat = "pdf" | "docx" | "epub" | "html" | "site";
export type PublicationKind = "publication";
export type PublicationOutput = "pdf" | "docx" | "epub" | "html" | "site";
export type PublicationPlan = { sourceKind: PublicationSourceKind, publicationId: string, publicationRevision: string, title: string, format: PublicationFormat, themeId: string, theme: ThemeSnapshot, rootPageId: string, structure: Array<PlannedStructureNode>, pages: Array<PlannedPage>, collections: Array<PlannedCollection>, listings: Array<PlannedListing>, assets: Array<PlannedAsset>, links: Array<PlannedLink>, snippets: Array<PlannedSnippet>, selectionRules: Array<SelectionRule>, targets: PublicationTargets, };
export type PublicationPreset = "book" | "documentation" | "blog" | "website";
export type PublicationSource = { "kind": "publication", id: string, };
export type PublicationSourceKind = "publication" | "linked_pages";
export type PublicationTargets = { pdf: PdfTarget, docx: DocxTarget, epub: HtmlTarget, html: HtmlTarget, site: SiteTarget, };
export type PublicationThemeAlignment = "start" | "center" | "end" | "justify";
export type PublicationThemeAsset = { path: string, bytes: number, sha256: string, };
export type PublicationThemeCaptionPosition = "before" | "after";
export type PublicationThemeChapterStart = "continuous" | "next-page";
export type PublicationThemeColors = { ink: PublicationThemeValue<string>, muted: PublicationThemeValue<string>, link: PublicationThemeValue<string>, accent: PublicationThemeValue<string>, surface: PublicationThemeValue<string>, paper: PublicationThemeValue<string>, };
export type PublicationThemeColorsPatch = { ink?: PublicationThemeValue<string>, muted?: PublicationThemeValue<string>, link?: PublicationThemeValue<string>, accent?: PublicationThemeValue<string>, surface?: PublicationThemeValue<string>, paper?: PublicationThemeValue<string>, };
export type PublicationThemeDefinition = { schemaVersion: number, id: string, version: string, name: string, tokens: { [key in string]: PublicationThemeToken }, root: PublicationThemeRoot, colors: PublicationThemeColors, styles: { [key in PublicationThemeRole]?: PublicationThemeStyle }, page: PublicationThemePage, targets: { [key in string]: PublicationThemePatch }, web: PublicationThemeWeb, };
export type PublicationThemeDeleted = { deleted: boolean, };
export type PublicationThemeLength = string;
export type PublicationThemeOrientation = "portrait" | "landscape";
export type PublicationThemeOverrides = { shared: PublicationThemePatch, targets: { [key in string]: PublicationThemePatch }, };
export type PublicationThemePage = { width: PublicationThemeValue<PublicationThemeLength>, height: PublicationThemeValue<PublicationThemeLength>, orientation: PublicationThemeOrientation, marginTop: PublicationThemeValue<PublicationThemeLength>, marginRight: PublicationThemeValue<PublicationThemeLength>, marginBottom: PublicationThemeValue<PublicationThemeLength>, marginLeft: PublicationThemeValue<PublicationThemeLength>, headerDistance: PublicationThemeValue<PublicationThemeLength>, footerDistance: PublicationThemeValue<PublicationThemeLength>, chapterStart: PublicationThemeChapterStart, header: PublicationThemeRunningLine, footer: PublicationThemeRunningLine, measureCharacters: number, };
export type PublicationThemePagePatch = { width?: PublicationThemeValue<PublicationThemeLength>, height?: PublicationThemeValue<PublicationThemeLength>, orientation?: PublicationThemeOrientation, marginTop?: PublicationThemeValue<PublicationThemeLength>, marginRight?: PublicationThemeValue<PublicationThemeLength>, marginBottom?: PublicationThemeValue<PublicationThemeLength>, marginLeft?: PublicationThemeValue<PublicationThemeLength>, headerDistance?: PublicationThemeValue<PublicationThemeLength>, footerDistance?: PublicationThemeValue<PublicationThemeLength>, chapterStart?: PublicationThemeChapterStart, header?: PublicationThemeRunningLinePatch, footer?: PublicationThemeRunningLinePatch, measureCharacters?: number, };
export type PublicationThemePatch = { tokens?: { [key in string]: PublicationThemeToken }, root?: PublicationThemeRoot, colors?: PublicationThemeColorsPatch, styles?: { [key in PublicationThemeRole]?: PublicationThemeStyle }, page?: PublicationThemePagePatch, };
export type PublicationThemeRecord = { summary: PublicationThemeSummary, definition: PublicationThemeDefinition, assets: Array<PublicationThemeAsset>, };
export type PublicationThemeReference = { ref: string, };
export type PublicationThemeRole = "body" | "heading1" | "heading2" | "heading3" | "heading4" | "heading5" | "heading6" | "link" | "quote" | "code" | "inlineCode" | "list" | "table" | "tableHeader" | "tableCell" | "caption" | "footnote" | "embed" | "rule" | "box" | "boxHeading" | "boxInfo" | "boxAbstract" | "boxNote" | "boxTip" | "boxSuccess" | "boxImportant" | "boxWarning" | "boxError" | "boxQuestion" | "boxExample";
export type PublicationThemeRoot = { fontSize: PublicationThemeValue<PublicationThemeLength>, };
export type PublicationThemeRunningLine = { left: Array<PublicationThemeRunningPart>, center: Array<PublicationThemeRunningPart>, right: Array<PublicationThemeRunningPart>, showOnFirstPage: boolean, };
export type PublicationThemeRunningLinePatch = { left?: Array<PublicationThemeRunningPart>, center?: Array<PublicationThemeRunningPart>, right?: Array<PublicationThemeRunningPart>, showOnFirstPage?: boolean, };
export type PublicationThemeRunningPart = { "kind": "text", "text": string } | { "kind": "documentTitle" } | { "kind": "chapterTitle" } | { "kind": "pageNumber" } | { "kind": "pageCount" };
export type PublicationThemeStroke = "solid" | "dashed" | "dotted";
export type PublicationThemeStyle = { fontFamily?: PublicationThemeValue<string>, fontSize?: PublicationThemeValue<PublicationThemeLength>, fontWeight?: PublicationThemeWeight, italic?: boolean, lineHeight?: PublicationThemeValue<number>, color?: PublicationThemeValue<string>, background?: PublicationThemeValue<string>, spaceBefore?: PublicationThemeValue<PublicationThemeLength>, spaceAfter?: PublicationThemeValue<PublicationThemeLength>, indent?: PublicationThemeValue<PublicationThemeLength>, padding?: PublicationThemeValue<PublicationThemeLength>, borderWidth?: PublicationThemeValue<PublicationThemeLength>, borderColor?: PublicationThemeValue<string>, borderStyle?: PublicationThemeStroke, alignment?: PublicationThemeAlignment, keepWithNext?: boolean, keepTogether?: boolean, captionPosition?: PublicationThemeCaptionPosition, };
export type PublicationThemeSummary = { id: string, name: string, version: string, origin: ThemeOrigin, revision: string, usages: Array<PublicationThemeUsage>, };
export type PublicationThemeToken = { "type": "color", "value": PublicationThemeValue<string> } | { "type": "length", "value": PublicationThemeValue<PublicationThemeLength> } | { "type": "font", "value": PublicationThemeValue<string> } | { "type": "number", "value": PublicationThemeValue<number> };
export type PublicationThemeUsage = { id: string, title: string, archived: boolean, };
export type PublicationThemeValidation = { resolved: { [key in string]: ResolvedPublicationTheme }, executionSha256: string, };
export type PublicationThemeValue<T> = T | PublicationThemeReference;
export type PublicationThemeWeb = { html?: string, site?: string, siteRoles?: { [key in string]: string }, controls?: Array<PublicationThemeWebControl>, };
export type PublicationThemeWebControl = { "kind": "toggle", id: string, label: string, default: boolean, } | { "kind": "choice", id: string, label: string, default: string, choices: Array<string>, };
export type PublicationThemeWeight = "regular" | "bold";
export type QueryCapabilities = { version: number, fields: Array<QueryFieldCapability>, textModes: Array<TextMode>, states: Array<StatePredicate>, };
export type QueryDocument = { version: number, clauses: Array<Condition>, sort: Array<QuerySort>, };
export type QueryFieldCapability = { key: string, valueType: MetadataFieldValueType, operators: Array<Comparison>, options: Array<string>, canTestPresence: boolean, canSort: boolean, referencesItems: boolean, relativeValues: Array<string>, };
export type QueryItemsRequest = { query: QueryDocument, options?: ReadOptions, limit?: number | null, offset?: number | null, pinScopeId?: string | null, prioritizePins?: boolean | null, };
export type QueryItemsResult = { items: Array<PublicItem>, count: number, pinnedCount?: number, };
export type QueryPassagesRequest = { label: string, query?: QueryDocument | null, pinScopeId?: string | null, limit?: number | null, offset?: number | null, };
export type QuerySnippetsRequest = { query?: string | null, limit?: number | null, offset?: number | null, };
export type QuerySort = { field: string, direction: SortDirection, };
export type QueryTasksRequest = { query?: QueryDocument | null, pinScopeId?: string | null, noteId?: string | null, status: TaskStatus, limit?: number | null, offset?: number | null, };
export type QueryValidation = { valid: boolean, diagnostics: Array<ConditionDiagnostic>, };
export type ReadItemRequest = { id: string, options?: ReadOptions, };
export type ReadItemsRequest = { ids?: Array<string> | null, query?: QueryDocument | null, options?: ReadOptions, limit?: number | null, offset?: number | null, };
export type ReadOptions = { content?: boolean | null, };
export type ReadPublicationThemePackageRequest = { themeId: string, };
export type ReadRequest = { jobId: string, includePlan?: boolean, };
export type ReadSnippetRequest = { id: string, };
export type ReadThemeRequest = { themeId: string, format: string, overrides?: PublicationThemeOverrides | null, };
export type ReconciliationTimings = { walkMs: number, pinScopeMs: number, fileStampMs: number, indexedStampMs: number, diffMs: number, };
export type ReferenceMark = { type: ReferenceMarkKind, attrs?: LinkAttributes, };
export type ReferenceMarkKind = "strong" | "emphasis" | "underline" | "strike" | "code" | "link" | "superscript" | "subscript";
export type ReferenceParagraph = { type: ParagraphKind, content?: Array<ReferenceText>, };
export type ReferenceText = { type: TextKind, text: string, marks?: Array<ReferenceMark>, };
export type RemovePropertyRequest = { key: string, };
export type RemoveWorkspaceRequest = { id: string, version: string, replacementId?: string | null, };
export type Removed = { removed: boolean, };
export type ResolveWorkspaceNotesRequest = { workspaceId: string, ids: Array<string>, limit?: number | null, };
export type ResolvedPublicationTheme = { rootFontSizePt: number, colors: { [key in string]: string }, styles: { [key in PublicationThemeRole]?: ResolvedPublicationThemeStyle }, page: ResolvedPublicationThemePage, fonts: Array<string>, };
export type ResolvedPublicationThemePage = { widthPt: number, heightPt: number, marginTopPt: number, marginRightPt: number, marginBottomPt: number, marginLeftPt: number, headerDistancePt: number, footerDistancePt: number, chapterStart: PublicationThemeChapterStart, header: PublicationThemeRunningLine, footer: PublicationThemeRunningLine, measureCharacters: number, };
export type ResolvedPublicationThemeStyle = { fontFamily: string, fontSizePt: number, fontWeight: PublicationThemeWeight, italic: boolean, lineHeight: number, color: string, background: string | null, spaceBeforePt: number, spaceAfterPt: number, indentPt: number, paddingPt: number, borderWidthPt: number, borderColor: string, borderStyle: PublicationThemeStroke, alignment: PublicationThemeAlignment, keepWithNext: boolean, keepTogether: boolean, captionPosition: PublicationThemeCaptionPosition | null, };
export type ResultPage<T> = { items: Array<T>, count: number, };
export type RpcFailure = { error: string, diagnostics: Array<ConditionDiagnostic>, };
export type SelectionEffect = "include" | "exclude";
export type SelectionMatcher = { kind: SelectionMatcherKind, value: string, };
export type SelectionMatcherKind = "block_type" | "block_label" | "page_tag";
export type SelectionRule = { id: string, effect: SelectionEffect, target: SelectionTarget, matcher: SelectionMatcher, };
export type SelectionTarget = "page" | "block" | "section";
export type SetDefaultWorkspaceRequest = { id: string, };
export type SetItemTagsRequest = { id: string, version: string, tags: Array<string>, };
export type SetNotePinRequest = { noteId: string, version: string, pinned: boolean, scopeId: string, };
export type SettingSchema = { "kind": "integer", min: number, max: number, } | { "kind": "choice", options: Array<OpaqueJson>, } | { "kind": "choices", options: Array<OpaqueJson>, } | { "kind": "text", maxLength: number, } | { "kind": "item", itemType: ItemType, optional: boolean, } | { "kind": "items", itemType: ItemType, } | { "kind": "query" };
export type SiteLayout = "documentation" | "website";
export type SiteTarget = { layout: SiteLayout, outsideLinks: OutsideLinks, baseUrl?: string, };
export type SnippetHit = { usageCount: number, format: string, formatVersion: number, id: string, state: SnippetState, revision: string, createdAt: string, modifiedAt: string, paragraph: ReferenceParagraph, };
export type SnippetOccurrence = { kind: SnippetOccurrenceKind, blockId?: string, };
export type SnippetOccurrenceKind = "citation" | "embed";
export type SnippetRecord = { format: string, formatVersion: number, id: string, state: SnippetState, revision: string, createdAt: string, modifiedAt: string, paragraph: ReferenceParagraph, };
export type SnippetResults = { items: Array<SnippetHit>, count: number, };
export type SnippetState = "active" | "archived";
export type SnippetUsage = { noteId: string, noteTitle: string, revision: string, occurrences: Array<SnippetOccurrence>, };
export type SortDirection = "asc" | "desc";
export type State = "planning" | "prepared" | "awaiting_theme_approval" | "queued" | "rendering" | "ready" | "failed" | "cancelled";
export type StatePredicate = "archived" | "pinned";
export type Status = { state: State, phase: string, format: PublicationFormat, plan?: Plan, summary?: Summary, themeTrust?: ThemeTrust, themePreview?: ThemePreviewSummary, artifactUrl?: string, bundleUrl?: string, previewUrl?: string, error?: string, };
export type Summary = { title: string, pages: number, collections: number, assets: number, links: number, };
export type TaskChange = { "kind": "create", text: string, } | { "kind": "edit", taskId: string, revision: string, text: string, } | { "kind": "complete", taskId: string, revision: string, } | { "kind": "reopen", taskId: string, revision: string, } | { "kind": "delete", taskId: string, revision: string, };
export type TaskRecord = { noteId: string, taskId: string, revision: string, noteTitle: string, text: string, createdAt: string, completedAt: string | null, dueDate: string | null, modifiedAt: string, };
export type TaskStatus = "open" | "completed";
export type TextKind = "text";
export type TextMode = "terms" | "phrase";
export type ThemeOptionValue = boolean | string;
export type ThemeOrigin = "builtin" | "project";
export type ThemePreviewSummary = { themeId: string, revision: string, draftSha256: string, };
export type ThemeSnapshot = { id: string, version: string, name: string, origin: ThemeOrigin, sha256: string, executionSha256: string, format: PublicationFormat, definition: PublicationThemeDefinition, overrides: PublicationThemeOverrides, resolved: ResolvedPublicationTheme, options: { [key in string]: ThemeOptionValue }, requiresTrust: boolean, };
export type ThemeTrust = { hash: string, trusted: boolean, root: string, };
export type UpdateItemMetaRequest = { id: string, version: string, meta: { [key in string]: OpaqueJson }, options?: ReadOptions, };
export type UpdateItemRequest = { id: string, version: string, meta: { [key in string]: OpaqueJson }, content: { [key in string]: OpaqueJson }, tags?: Array<string>, pins?: Array<string>, };
export type UpdatePropertyOptionsRequest = { key: string, valueType: MetadataFieldValueType,
/**
 * Options to add; existing ones are kept.
 */
options?: Array<string>,
/**
 * When given, replaces how every option looks; options left out are automatic.
 */
optionAppearance?: { [key in string]: ChoiceAppearance }, };
export type UpdatePublicationThemeRequest = { themeId: string, revision: string, definition: PublicationThemeDefinition, };
export type UpdateSnippetRequest = { id: string, revision: string, paragraph: ReferenceParagraph, };
export type Uploaded = { src: string, filename: string, contentType: string, size: number, };
export type ViewCalendarPresentation = { dateField?: string, initialZoom?: CalendarZoom, };
export type ViewContent = { query?: QueryDocument,
/**
 * The workspace's widgets, each with its own settings, in Home's order.
 * Those pinned to the top bar are also shortcuts there.
 */
widgets?: Array<WorkspaceWidget>, calendar?: ViewCalendarPresentation,
/**
 * The note templates the workspace's New menu offers, in order.
 */
templates?: WorkspaceTemplates, } & ({ [key in string]: unknown });
export type ViewKanbanColumn = { id: string, hidden?: boolean, };
export type ViewKanbanPresentation = { groupBy?: string, columns?: Array<ViewKanbanColumn>, };
export type ViewPresentationMode = "cards" | "table" | "kanban" | "list";
export type ViewTableColumn = { field: string, label?: string, hidden?: boolean, width?: number, };
export type ViewTablePresentation = { columns?: Array<ViewTableColumn>, };
export type WidgetConfig = { "kind": "continue_writing", noteId: string | null, } | { "kind": "recent_notes", count: number, presentation: NotePresentation, } | { "kind": "pinned_notes", count: number, presentation: NotePresentation, } | { "kind": "workspace_shortcuts", workspaceIds: Array<string>, } | { "kind": "layout", query: QueryDocument, mode: ViewPresentationMode,
/**
 * The card design, owned and checked by the browser so it can change
 * without a backend release.
 */
cards?: OpaqueJson, table?: ViewTablePresentation, kanban?: ViewKanbanPresentation, } | { "kind": "tasks", status: TaskStatus, count: number, } | { "kind": "writing_activity", days: number, } | { "kind": "publications", publicationIds: Array<string>, count: number, } | { "kind": "introduction", title: string, description: string, } | { "kind": "writing_stats", stats: Array<WritingStat>, days: number, } | { "kind": "writing_goal", } | { "kind": "create_buttons", templateIds: Array<string>, } | { "kind": "count", query: QueryDocument, } | { "kind": "writing_rhythm", days: number, };
export type WidgetConstraint = { kind: string, widths: Array<WidgetSpan>, heights: Array<WidgetHeight>, settings: Array<WidgetSettingConstraint>, };
export type WidgetHeight = "compact" | "regular" | "tall";
export type WidgetSettingConstraint = { key: string, schema: SettingSchema, };
export type WidgetSpan = 4 | 6 | 8 | 12;
export type WorkspaceContext = { projectId: string, defaultWorkspace: PublicItem, };
export type WorkspaceTemplates = { offered: Array<string>,
/**
 * The template the main New action uses; none creates a plain note.
 */
default: string | null, };
export type WorkspaceWidget = { id: string,
/**
 * Replaces the type's name as the widget's heading.
 */
title?: string,
/**
 * Where it sits on Home.
 */
home?: HomePlacement,
/**
 * Pinned to the top bar as a shortcut to the widget.
 */
topBar?: boolean, config: WidgetConfig, };
export type WritingGoalContent = { kind: WritingGoalKind, scope: GoalScope, viewId?: string, query?: QueryDocument, startDate: string, endDate: string, dailyWordTarget: number, totalWordTarget?: number, };
export type WritingGoalKind = "writing_challenge";
export type WritingStat = "words_today" | "streak" | "active_days" | "words_added" | "net_change" | "tasks_completed";

export type Api = {
  approve_export_theme: { method: 'POST'; request: ApproveRequest; response: Approved }
  archive_snippet: { method: 'POST'; request: ArchiveSnippetRequest; response: SnippetRecord }
  cache_status: { method: 'GET'; request: undefined; response: CacheStatus }
  cancel_export: { method: 'POST'; request: JobRequest; response: Cancelled }
  create_export: { method: 'POST'; request: ExportRequest; response: JobId }
  create_facet: { method: 'POST'; request: FacetRequest; response: FacetEntry }
  create_item: { method: 'POST'; request: CreateItemRequest; response: PublicItem }
  create_property: { method: 'POST'; request: CreatePropertyRequest; response: MetadataField }
  create_publication_theme: { method: 'POST'; request: CreatePublicationThemeRequest; response: PublicationThemeRecord }
  create_snippet: { method: 'POST'; request: CreateSnippetRequest; response: SnippetRecord }
  create_workspace_from_setup: { method: 'POST'; request: CreateWorkspaceFromSetupRequest; response: PublicItem }
  create_writing_goal: { method: 'POST'; request: CreateWritingGoalRequest; response: PublicItem }
  delete_item: { method: 'POST'; request: DeleteItemRequest; response: DeleteResult }
  delete_publication_theme: { method: 'POST'; request: DeletePublicationThemeRequest; response: PublicationThemeDeleted }
  duplicate_publication_theme: { method: 'POST'; request: DuplicatePublicationThemeRequest; response: PublicationThemeRecord }
  list_facets: { method: 'POST'; request: ListFacetsRequest; response: Array<FacetEntry> }
  list_publication_themes: { method: 'POST'; request: EmptyRequest; response: Array<PublicationThemeSummary> }
  mutate_note_task: { method: 'POST'; request: MutateNoteTaskRequest; response: PublicItem }
  preview_publication_theme: { method: 'POST'; request: PreviewPublicationThemeRequest; response: JobId }
  query_items: { method: 'POST'; request: QueryItemsRequest; response: QueryItemsResult }
  query_label_passages: { method: 'POST'; request: QueryPassagesRequest; response: ResultPage<PassageRecord> }
  query_note_tasks: { method: 'POST'; request: QueryTasksRequest; response: ResultPage<TaskRecord> }
  query_snippets: { method: 'POST'; request: QuerySnippetsRequest; response: SnippetResults }
  read_calendar_buckets: { method: 'POST'; request: CalendarRequest; response: CalendarResults }
  read_dashboard_analytics: { method: 'POST'; request: AnalyticsRequest; response: DashboardAnalytics }
  read_dashboard_day_details: { method: 'POST'; request: DayDetailsRequest; response: DashboardDayDetails }
  read_export: { method: 'POST'; request: ReadRequest; response: Status }
  read_facet_counts: { method: 'POST'; request: FacetCountsRequest; response: Array<FacetCount> }
  read_item: { method: 'POST'; request: ReadItemRequest; response: PublicItem }
  read_items: { method: 'POST'; request: ReadItemsRequest; response: Array<PublicItem> }
  read_metadata_fields: { method: 'POST'; request: EmptyRequest; response: Array<MetadataField> }
  read_publication_theme: { method: 'POST'; request: ReadThemeRequest; response: ThemeSnapshot }
  read_publication_theme_package: { method: 'POST'; request: ReadPublicationThemePackageRequest; response: PublicationThemeRecord }
  read_query_capabilities: { method: 'POST'; request: EmptyRequest; response: QueryCapabilities }
  read_snippet: { method: 'POST'; request: ReadSnippetRequest; response: SnippetRecord | null }
  read_snippet_usages: { method: 'POST'; request: ReadSnippetRequest; response: Array<SnippetUsage> }
  read_workspace_context: { method: 'POST'; request: EmptyRequest; response: WorkspaceContext }
  remove_facet: { method: 'POST'; request: FacetRequest; response: Removed }
  remove_property: { method: 'POST'; request: RemovePropertyRequest; response: Removed }
  remove_workspace: { method: 'POST'; request: RemoveWorkspaceRequest; response: WorkspaceContext }
  resolve_workspace_notes: { method: 'POST'; request: ResolveWorkspaceNotesRequest; response: Array<PublicItem> }
  set_default_workspace: { method: 'POST'; request: SetDefaultWorkspaceRequest; response: PublicItem }
  set_item_tags: { method: 'POST'; request: SetItemTagsRequest; response: PublicItem }
  set_note_pin: { method: 'POST'; request: SetNotePinRequest; response: PinResult }
  start_export: { method: 'POST'; request: JobRequest; response: JobId }
  start_export_preview: { method: 'POST'; request: JobRequest; response: PreviewUrl }
  update_item: { method: 'POST'; request: UpdateItemRequest; response: PublicItem }
  update_item_meta: { method: 'POST'; request: UpdateItemMetaRequest; response: PublicItem }
  update_property_options: { method: 'POST'; request: UpdatePropertyOptionsRequest; response: MetadataField }
  update_publication_theme: { method: 'POST'; request: UpdatePublicationThemeRequest; response: PublicationThemeRecord }
  update_snippet: { method: 'POST'; request: UpdateSnippetRequest; response: SnippetRecord }
  validate_publication_theme: { method: 'POST'; request: UpdatePublicationThemeRequest; response: PublicationThemeValidation }
  validate_query: { method: 'POST'; request: DocumentQueryRequest; response: QueryValidation }
}
