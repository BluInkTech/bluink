// SPDX-License-Identifier: MIT
// See backend/contracts/LICENSE.
// Generated from Rust; do not edit.
// Run npm run contracts:generate from the repository root.

export type ActivityDay = { date: string, notesTouched: number, wordsAdded: number, wordsDeleted: number, netWordChange: number, editingEvents: number, tasksCompleted: number, };
export type AnalyticsRequest = { startDate?: string | null, endDate?: string | null, query?: QueryDocument | null, pinScopeId?: string | null, };
export type ApproveRequest = { jobId: string, hash: string, };
export type Approved = { trusted: boolean, };
export type ArchiveSnippetRequest = { id: string, revision: string, };
export type AuthInput = { account: string, token: string | null, privateKey: string | null, passphrase: string | null, forget: boolean, useAgent: boolean | null, };
export type AuthStatus = { account: string, hasToken: boolean, privateKey: string | null, hostFingerprint: string | null, useAgent: boolean, };
export type CalendarBucket = { date: string, count: number, };
export type CalendarGranularity = "day" | "month" | "year";
export type CalendarRequest = { query?: QueryDocument | null, field: string, startDate: string, endDate: string, granularity: CalendarGranularity, pinScopeId?: string | null, };
export type CalendarResults = { buckets: Array<CalendarBucket>, };
export type CalendarZoom = "month" | "week";
export type Cancelled = { cancelled: boolean, };
export type ChangeBatch = { generation: string, domains: Array<ChangeDomain>,
/**
 * A reset invalidates every record in these domains, without enumerating IDs.
 */
reset: boolean, items: Array<ChangedItem>, };
export type ChangeCursor = { epoch: string,
/**
 * Decimal SQLite generation; strings preserve the entire integer range in JS.
 */
generation: string, };
export type ChangeDomain = "items" | "fields" | "decisions" | "history" | "activity" | "settings";
export type ChangeOperation = "upsert" | "delete";
export type ChangedItem = { id: string, operation: ChangeOperation, itemType?: ItemType, };
export type ChoiceAppearance = { color: ChoiceColor, };
export type ChoiceColor = "neutral" | "blue" | "green" | "amber" | "purple" | "rose" | "cyan";
export type CloneProject = { origin: string, destination: string, };
export type ClonedProject = { path: string, };
export type Comparison = "match" | "gt" | "gte" | "lt" | "lte";
export type Condition = { polarity: Polarity, predicate: Predicate, };
export type ConditionDiagnostic = { path: string, code: string, message: string, start?: number, end?: number, };
export type ConfigureSync = { origin: string | null, create?: boolean, };
export type ConfigureSyncAuth = { origin: string, auth: AuthInput, };
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
export type DecisionFile = { path: string, content: OpaqueJson, };
export type DecisionPreview = { decision: PublicItem, currentTargets: Array<PublicItem>, currentFiles: Array<DecisionFile>, allowedSelections: Array<Selection>, expected: Expected, };
export type DecisionRequest = { id: string, };
export type DecisionWarning = { pageId: string, decisionIds: Array<string>, };
export type DecisionsRequest = { targetId?: string, };
export type DecisionsResult = { decisions: Array<PublicItem>, };
export type DeleteItemRequest = { id: string, };
export type DeletePublicationThemeRequest = { themeId: string, revision: string, };
export type DeleteResult = { deleted: boolean, };
export type DocumentPatch = { attrs?: { [key in string]: OpaqueJson }, order: Array<string>,
/**
 * Only new or changed complete top-level subtrees, keyed by editor block ID.
 */
blocks: { [key in string]: OpaqueJson }, };
export type DocumentQueryRequest = { document: QueryDocument, pinScopeId?: string | null, };
export type DocxTarget = { outsideLinks: OutsideLinks, };
export type DraftItem = { meta: Metadata, content: { [key in string]: OpaqueJson }, tags?: Array<string>, pins?: Array<string>, };
export type DuplicatePublicationThemeRequest = { themeId: string, revision: string, name: string, definition?: PublicationThemeDefinition, };
export type EditorBaseLease = { retained: boolean, };
export type EditorDraft = { "kind": "replace", item: DraftItem, } | { "kind": "documentPatch", meta: Metadata, tags?: Array<string>, pins?: Array<string>,
/**
 * Complete resulting content fields other than `document`.
 */
content: { [key in string]: OpaqueJson }, document: DocumentPatch, };
export type EmptyRequest = Record<symbol, never>;
export type Expected = { decisionRevision: string,
/**
 * Complete preview read set, including unchanged dependency owners.
 */
targets: { [key in string]: string }, };
export type ExportRequest = PageExportInput<NoteSource, NoteFormat> | PageExportInput<LinkedSource, LinkedFormat> | PublicationExportInput;
export type FacetCount = { key: string, count: number, };
export type FacetCountsRequest = { kind: FacetKind, keys?: Array<string> | null, sourceType?: string | null, };
export type FacetEntry = { key: string, kind: FacetKind, usageCount: number, inferred: boolean, };
export type FacetKind = "tag" | "label";
export type FacetRequest = { kind: FacetKind, key: string, };
export type FactIssue = { blockId?: string, message: string, };
export type FieldSource = "registered" | "inferred" | "builtin";
export type GenerateKeyInput = { name: string, account: string, passphrase: string, };
export type GenerateSyncKey = { origin: string, key: GenerateKeyInput, };
export type GeneratedKey = { privateKey: string, publicKey: string, fingerprint: string, };
export type GoalScope = "all_notes" | "view";
export type HistoryChange = "added" | "changed" | "removed";
export type HistoryClientActivity = { accepted: boolean, };
export type HistoryClientActivityRequest = { clientId: string, visible: boolean, };
export type HistoryDownload = { reachableBytes: number, missingBytes: number, };
export type HistoryEntry = { id: string, message: string, createdAt: string, namedVersions: Array<NamedVersion>, attachmentsAvailable: boolean, };
export type HistoryItemChange = { id: string, title: string, change: HistoryChange, };
export type HistoryItemRequest = { commitId: string, itemId: string, };
export type HistoryPage = { entries: Array<HistoryEntry>, cursor: string | null, };
export type HistoryPreview = { commitId: string, itemId: string | null, expectedHead: string, expectedGeneration: string, changedItems: Array<HistoryItemChange>, changedAssets: number, changedSettings: boolean, changedThemes: number, archivedTasks: Array<string>, };
export type HistoryPreviewRequest = { commitId: string, itemId?: string, };
export type HistoryRequest = { itemId?: string, before?: string, limit?: number, };
export type HistoryRestoreRequest = { commitId: string, itemId?: string, expectedHead: string, expectedGeneration: string, };
export type HistoryStatus = { head: string | null, dirty: boolean, lastCheckpointAt: string | null, error: string | null, localOnly: boolean, };
export type HomePlacement = { span: WidgetSpan,
/**
 * The card's height; without one, the type's default height.
 */
height?: WidgetHeight,
/**
 * Kept off Home while keeping its place, for a widget reached from the top bar.
 */
hidden?: boolean, };
export type HostIdentity = { host: string, port: number, fingerprint: string, };
export type HtmlTarget = { outsideLinks: OutsideLinks, };
export type ItemCover = { "kind": "gradient", name: CoverGradient, } | { "kind": "image", url: string, position: number, };
export type ItemIcon = { "kind": "symbol", name: string, } | { "kind": "emoji", value: string, } | { "kind": "image", url: string, };
export type ItemState = "active" | "archived";
export type ItemType = "page" | "workspace" | "goal" | "widget" | "query" | "setting" | "publication" | "task" | "tag" | "label" | "property" | "snippet" | "template" | "conflict";
export type JobId = { jobId: string, };
export type JobRequest = { jobId: string, };
export type JsonValue = number | string | boolean | Array<JsonValue> | { [key in string]: JsonValue } | null;
export type LinkAttributes = { href: string, };
export type LinkDepth = "direct" | "two-levels" | "reachable";
export type LinkResolution = "included" | "text_only" | "external" | "anchor";
export type LinkedFormat = "pdf" | "docx" | "epub" | "site";
export type LinkedSource = { "kind": "linked-pages", id: string, depth: LinkDepth, };
export type ListFacetsRequest = { kind?: FacetKind | null, };
export type MergeDraftRequest = { id: string, baseRevision: string, submissionId: string, device: string, draft: EditorDraft, };
export type MergeDraftResult = { "kind": "ack", version: string, modifiedAt: string, conflictIds: Array<string>, submissionId: string, } | { "kind": "reconciled", item: PublicItem, conflictIds: Array<string>, submissionId: string, };
export type Metadata = { state?: ItemState, type?: ItemType, title?: string, summary?: string, image?: string, color?: string, icon?: ItemIcon, cover?: ItemCover, propertyKeys?: Array<string>, lang?: string, } & ({ [key in string]: unknown });
export type MetadataField = { key: string, revision?: string, valueType: MetadataFieldValueType, source: FieldSource, system?: boolean, options?: Array<string>,
/**
 * How chosen options look; options without an entry are automatic.
 */
optionAppearance?: { [key in string]: ChoiceAppearance }, };
export type MetadataFieldValueType = "text" | "url" | "email" | "tel" | "number" | "checkbox" | "date" | "dateTime" | "timestamp" | "select" | "multiSelect" | "language" | "color" | "image" | "rating" | "page";
export type MutateNoteTaskRequest = { noteId: string, change: TaskChange, };
export type NamedVersion = { id: string, title: string, itemId: string | null, commitId: string, };
export type NamedVersionRequest = { requestId: string, title: string, itemId?: string, };
export type NoteFormat = "pdf" | "docx" | "epub" | "html";
export type NotePresentation = "list" | "cards";
export type NoteSource = { "kind": "note", id: string, };
export type OpaqueJson = unknown;
export type OutlineNode = { "kind": "page", pageId: string, children: Array<OutlineNode>, showInNavigation?: boolean, slug?: string | null, } | { "kind": "section", id: string, title: string, introPageId?: string | null, children: Array<OutlineNode>, } | { "kind": "collection", id: string, title: string, query: QueryDocument, showPagesInNavigation?: boolean, listingSlug?: string | null, pageSize?: number, feed?: boolean, archives?: boolean, };
export type OutsideLinks = "error" | "text-only";
export type PageExportInput<S, F> = { source: S, format: F, start: boolean, themeId: string, outsideLinks: OutsideLinks, profile: PdfProfile, };
export type PageIssuesRequest = { id: string, };
export type PagePlan = { pageId: string, pageRevision: string, title: string, format: PublicationFormat, profile: PdfProfile, outsideLinks: OutsideLinks, theme: ThemeSnapshot, assets: Array<PlannedAsset>, links: Array<PlannedLink>, snippets: Array<PlannedSnippet>, warnings: Array<DecisionWarning>, decisionsChecked: boolean, };
export type ParagraphKind = "paragraph";
export type PassageRecord = { itemId: string, blockId: string, revision: string, noteTitle: string, labels: Array<string>, text: string, fragment: { [key in string]: OpaqueJson }, truncated: boolean, };
export type PdfProfile = "default" | "accessible";
export type PdfTarget = { profile: PdfProfile, outsideLinks: OutsideLinks, };
export type PinResult = { pinned: boolean, version: string, };
export type Plan = { "kind": "page", "value": PagePlan } | { "kind": "publication", "value": PublicationPlan };
export type PlanIssue = { pageId: string, title: string, blockId?: string, message: string, };
export type PlannedAsset = { src: string, ownerId: string, filename: string, bytes: number, sha256: string,
/**
 * The image file's stamp when it was hashed.
 */
stamp?: string, };
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
export type Progress = { checked: number, discovered: number, };
export type PublicItem = { id: string, version: string, createdAt: string, modifiedAt: string, meta: Metadata, content: { [key in string]: OpaqueJson }, tags?: Array<string>, pins?: Array<string>, preview?: string, previewImage?: string,
/**
 * A result card's active tasks; absent for full reads and pages without tasks.
 */
tasks?: TaskCounts, };
export type PublicationContent = { kind: PublicationKind, selectedOutput: PublicationOutput, rootPageId: string, outline: Array<OutlineNode>, selectionRules: Array<SelectionRule>, themeId: string, themeOverrides?: PublicationThemeOverrides, themeOptions?: { [key in string]: ThemeOptionValue }, targets: PublicationTargets, };
export type PublicationExportInput = { source: PublicationSource, format: PublicationFormat, start: boolean, };
export type PublicationFormat = "pdf" | "docx" | "epub" | "html" | "site";
export type PublicationKind = "publication";
export type PublicationOutput = "pdf" | "docx" | "epub" | "html" | "site";
export type PublicationPlan = { sourceKind: PublicationSourceKind, publicationId: string, publicationRevision: string, title: string, format: PublicationFormat, themeId: string, theme: ThemeSnapshot, rootPageId: string, structure: Array<PlannedStructureNode>, pages: Array<PlannedPage>, collections: Array<PlannedCollection>, listings: Array<PlannedListing>, assets: Array<PlannedAsset>, links: Array<PlannedLink>, snippets: Array<PlannedSnippet>, selectionRules: Array<SelectionRule>, targets: PublicationTargets,
/**
 * A linked-pages plan resolves its links, images and snippets when
 * generation reads its pages; until then this is true and they are empty.
 */
unresolved?: boolean,
/**
 * Problems that stop this export, listed together so the writer can fix
 * them all; a plan with issues does not generate.
 */
issues?: Array<PlanIssue>,
/**
 * Non-blocking decisions on included content. Export uses its currently
 * accepted provisional form, never the alternatives embedded in a decision.
 */
warnings: Array<DecisionWarning>, decisionsChecked: boolean, };
export type PublicationSource = { "kind": "publication", id: string, };
export type PublicationSourceKind = "publication" | "linked_pages";
export type PublicationTargets = { pdf: PdfTarget, docx: DocxTarget, epub: HtmlTarget, html: HtmlTarget, site: SiteTarget, };
export type PublicationThemeAlignment = "start" | "center" | "end" | "justify";
export type PublicationThemeAsset = { path: string, bytes: number, sha256: string, };
export type PublicationThemeBorderSides = "all" | "start";
export type PublicationThemeCaptionPosition = "before" | "after";
export type PublicationThemeChapterStart = "continuous" | "next-page";
export type PublicationThemeColors = { ink: PublicationThemeValue<string>, muted: PublicationThemeValue<string>, link: PublicationThemeValue<string>, accent: PublicationThemeValue<string>, surface: PublicationThemeValue<string>, paper: PublicationThemeValue<string>,
/**
 * The intents a box can carry, each drawn in its own colour.
 */
note: PublicationThemeValue<string>, tip: PublicationThemeValue<string>, important: PublicationThemeValue<string>, warning: PublicationThemeValue<string>, caution: PublicationThemeValue<string>, };
export type PublicationThemeColorsPatch = { ink?: PublicationThemeValue<string>, muted?: PublicationThemeValue<string>, link?: PublicationThemeValue<string>, accent?: PublicationThemeValue<string>, surface?: PublicationThemeValue<string>, paper?: PublicationThemeValue<string>, note?: PublicationThemeValue<string>, tip?: PublicationThemeValue<string>, important?: PublicationThemeValue<string>, warning?: PublicationThemeValue<string>, caution?: PublicationThemeValue<string>, };
export type PublicationThemeDefinition = { schemaVersion: number, id: string, version: string, name: string, tokens: { [key in string]: PublicationThemeToken }, root: PublicationThemeRoot, colors: PublicationThemeColors, styles: { [key in PublicationThemeRole]?: PublicationThemeStyle }, page: PublicationThemePage, targets: { [key in string]: PublicationThemePatch }, web: PublicationThemeWeb, };
export type PublicationThemeDeleted = { deleted: boolean, };
export type PublicationThemeLength = string;
export type PublicationThemeOrientation = "portrait" | "landscape";
export type PublicationThemeOverrides = { shared: PublicationThemePatch, targets: { [key in string]: PublicationThemePatch }, };
export type PublicationThemePage = { width: PublicationThemeValue<PublicationThemeLength>, height: PublicationThemeValue<PublicationThemeLength>, orientation: PublicationThemeOrientation, marginTop: PublicationThemeValue<PublicationThemeLength>, marginRight: PublicationThemeValue<PublicationThemeLength>, marginBottom: PublicationThemeValue<PublicationThemeLength>, marginLeft: PublicationThemeValue<PublicationThemeLength>, headerDistance: PublicationThemeValue<PublicationThemeLength>, footerDistance: PublicationThemeValue<PublicationThemeLength>, chapterStart: PublicationThemeChapterStart, referencesAt: PublicationThemeReferencesAt,
/**
 * The heading above a reference list.
 */
referencesHeading: string, header: PublicationThemeRunningLine, footer: PublicationThemeRunningLine, measureCharacters: number, };
export type PublicationThemePagePatch = { width?: PublicationThemeValue<PublicationThemeLength>, height?: PublicationThemeValue<PublicationThemeLength>, orientation?: PublicationThemeOrientation, marginTop?: PublicationThemeValue<PublicationThemeLength>, marginRight?: PublicationThemeValue<PublicationThemeLength>, marginBottom?: PublicationThemeValue<PublicationThemeLength>, marginLeft?: PublicationThemeValue<PublicationThemeLength>, headerDistance?: PublicationThemeValue<PublicationThemeLength>, footerDistance?: PublicationThemeValue<PublicationThemeLength>, chapterStart?: PublicationThemeChapterStart, referencesAt?: PublicationThemeReferencesAt, referencesHeading?: string, header?: PublicationThemeRunningLinePatch, footer?: PublicationThemeRunningLinePatch, measureCharacters?: number, };
export type PublicationThemePatch = { tokens?: { [key in string]: PublicationThemeToken }, root?: PublicationThemeRoot, colors?: PublicationThemeColorsPatch, styles?: { [key in PublicationThemeRole]?: PublicationThemeStyle }, page?: PublicationThemePagePatch, };
export type PublicationThemeRecord = { summary: PublicationThemeSummary, definition: PublicationThemeDefinition, assets: Array<PublicationThemeAsset>, };
export type PublicationThemeReference = { ref: string, };
export type PublicationThemeReferencesAt = "chapter" | "book";
export type PublicationThemeRole = "body" | "heading1" | "heading2" | "heading3" | "heading4" | "heading5" | "heading6" | "link" | "quote" | "code" | "inlineCode" | "list" | "table" | "tableHeader" | "tableCell" | "caption" | "footnote" | "embed" | "rule" | "attribution" | "box" | "boxHeading" | "boxNote" | "boxTip" | "boxImportant" | "boxWarning" | "boxCaution";
export type PublicationThemeRoot = { fontSize: PublicationThemeValue<PublicationThemeLength>, };
export type PublicationThemeRunningLine = { left: Array<PublicationThemeRunningPart>, center: Array<PublicationThemeRunningPart>, right: Array<PublicationThemeRunningPart>, showOnFirstPage: boolean, };
export type PublicationThemeRunningLinePatch = { left?: Array<PublicationThemeRunningPart>, center?: Array<PublicationThemeRunningPart>, right?: Array<PublicationThemeRunningPart>, showOnFirstPage?: boolean, };
export type PublicationThemeRunningPart = { "kind": "text", "text": string } | { "kind": "documentTitle" } | { "kind": "chapterTitle" } | { "kind": "pageNumber" } | { "kind": "pageCount" };
export type PublicationThemeStroke = "solid" | "dashed" | "dotted";
export type PublicationThemeStyle = { fontFamily?: PublicationThemeValue<string>, fontSize?: PublicationThemeValue<PublicationThemeLength>, fontWeight?: PublicationThemeWeight, italic?: boolean, lineHeight?: PublicationThemeValue<number>, color?: PublicationThemeValue<string>, background?: PublicationThemeValue<string>, spaceBefore?: PublicationThemeValue<PublicationThemeLength>, spaceAfter?: PublicationThemeValue<PublicationThemeLength>, indent?: PublicationThemeValue<PublicationThemeLength>, padding?: PublicationThemeValue<PublicationThemeLength>, borderWidth?: PublicationThemeValue<PublicationThemeLength>, borderColor?: PublicationThemeValue<string>, borderStyle?: PublicationThemeStroke, borderSides?: PublicationThemeBorderSides, alignment?: PublicationThemeAlignment, keepWithNext?: boolean, keepTogether?: boolean, captionPosition?: PublicationThemeCaptionPosition, };
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
export type QueryTasksRequest = { query?: QueryDocument | null, pinScopeId?: string | null, noteId?: string | null,
/**
 * Conditions on the tasks themselves, applied after the page scope.
 */
taskQuery?: QueryDocument | null, status: TaskStatus, limit?: number | null, offset?: number | null, };
export type QueryValidation = { valid: boolean, diagnostics: Array<ConditionDiagnostic>, };
export type ReadChangesRequest = { cursor?: ChangeCursor, };
export type ReadChangesResult = { cursor: ChangeCursor, reset: boolean, batches: Array<ChangeBatch>, hasMore: boolean, };
export type ReadItemRequest = { id: string, options?: ReadOptions, };
export type ReadItemsRequest = { ids?: Array<string> | null, query?: QueryDocument | null, options?: ReadOptions, limit?: number | null, offset?: number | null, };
export type ReadOptions = { content?: boolean | null, };
export type ReadPublicationThemePackageRequest = { themeId: string, };
export type ReadRequest = { jobId: string, includePlan?: boolean, };
export type ReadSnippetRequest = { id: string, };
export type ReadThemeRequest = { themeId: string, format: string, overrides?: PublicationThemeOverrides | null, };
export type RecoveryRef = { name: string, commit: string, };
export type ReferenceMark = { type: ReferenceMarkKind, attrs?: LinkAttributes, };
export type ReferenceMarkKind = "strong" | "emphasis" | "underline" | "strike" | "code" | "link" | "superscript" | "subscript";
export type ReferenceParagraph = { type: ParagraphKind, content?: Array<ReferenceText>, };
export type ReferenceText = { type: TextKind, text: string, marks?: Array<ReferenceMark>, };
export type RemovePropertyRequest = { key: string, };
export type RemoveWorkspaceRequest = { id: string, version: string, replacementId?: string | null, };
export type Removed = { removed: boolean, };
export type RenewEditorBaseRequest = { leaseId: string, id: string, revision: string, };
export type ResolveDecisionRequest = { id: string, expected: Expected, selection: Selection, device: string, };
export type ResolveDecisionResult = { decision: PublicItem, items: Array<PublicItem>, deletedIds: Array<string>, };
export type ResolveWorkspaceNotesRequest = { workspaceId: string, ids: Array<string>, limit?: number | null, };
export type ResolvedPublicationTheme = { rootFontSizePt: number, colors: { [key in string]: string }, styles: { [key in PublicationThemeRole]?: ResolvedPublicationThemeStyle }, page: ResolvedPublicationThemePage, fonts: Array<string>, };
export type ResolvedPublicationThemePage = { widthPt: number, heightPt: number, marginTopPt: number, marginRightPt: number, marginBottomPt: number, marginLeftPt: number, headerDistancePt: number, footerDistancePt: number, chapterStart: PublicationThemeChapterStart, referencesAt: PublicationThemeReferencesAt, referencesHeading: string, header: PublicationThemeRunningLine, footer: PublicationThemeRunningLine, measureCharacters: number, };
export type ResolvedPublicationThemeStyle = { fontFamily: string, fontSizePt: number, fontWeight: PublicationThemeWeight, italic: boolean, lineHeight: number, color: string, background: string | null, spaceBeforePt: number, spaceAfterPt: number, indentPt: number, paddingPt: number, borderWidthPt: number, borderColor: string, borderStyle: PublicationThemeStroke, borderSides: PublicationThemeBorderSides, alignment: PublicationThemeAlignment, keepWithNext: boolean, keepTogether: boolean, captionPosition: PublicationThemeCaptionPosition | null, };
export type ResultPage<T> = { items: Array<T>, count: number, };
export type RpcFailure = { error: string, diagnostics: Array<ConditionDiagnostic>, code?: RpcFailureCode, };
export type RpcFailureCode = "stale_decision" | "project_busy";
export type Selection = "provisional" | "first" | "second" | "both";
export type SelectionEffect = "include" | "exclude";
export type SelectionMatcher = { kind: SelectionMatcherKind, value: string, };
export type SelectionMatcherKind = "block_type" | "block_label" | "page_tag";
export type SelectionRule = { id: string, effect: SelectionEffect, target: SelectionTarget, matcher: SelectionMatcher, };
export type SelectionTarget = "page" | "block" | "section";
export type SetItemTagsRequest = { id: string, version: string, tags: Array<string>, };
export type SetNotePinRequest = { noteId: string, version: string, pinned: boolean, scopeId: string, };
export type SettingSchema = { "kind": "integer", min: number, max: number, } | { "kind": "choice", options: Array<OpaqueJson>, } | { "kind": "choices", options: Array<OpaqueJson>, } | { "kind": "text", maxLength: number, } | { "kind": "item", itemType: ItemType, optional: boolean, } | { "kind": "items", itemType: ItemType, } | { "kind": "query", optional: boolean, } | { "kind": "property", optional: boolean, };
export type SiteLayout = "documentation" | "website";
export type SiteTarget = { layout: SiteLayout, outsideLinks: OutsideLinks, baseUrl?: string,
/**
 * Also typeset the whole Publication as one PDF, linked from the home page.
 */
pdfDownload?: boolean, };
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
export type Status = { state: State, phase: string,
/**
 * Pages checked so far while a linked-pages export is planned.
 */
progress?: Progress, format: PublicationFormat, plan?: Plan, summary?: Summary, themeTrust?: ThemeTrust, themePreview?: ThemePreviewSummary, artifactUrl?: string, bundleUrl?: string, previewUrl?: string, error?: string, };
export type StorageStatus = { projectId: string, schemaVersion: number, generation: string, itemCount: number, pendingCheckpointPaths: number, };
export type Summary = { title: string, pages: number, collections: number, assets: number, links: number, };
export type SyncOrigin = { origin: string, };
export type SyncStatus = { origin: string | null, state: string, phase: string | null, error: string | null, head: string | null, lastSyncedHead: string | null, decisions: number, recoveryRefs: Array<RecoveryRef>, remoteHead: string | null, };
export type TaskChange = { "kind": "create", text: string, } | { "kind": "edit", taskId: string, revision: string, text: string, } | { "kind": "complete", taskId: string, revision: string, } | { "kind": "reopen", taskId: string, revision: string, } | { "kind": "delete", taskId: string, revision: string, } | { "kind": "move", taskId: string, revision: string, toNoteId: string, };
export type TaskCounts = { open: number, total: number, };
export type TaskRecord = { noteId: string, taskId: string, revision: string, noteTitle: string, text: string, createdAt: string, completedAt: string | null, dueDate: string | null, modifiedAt: string,
/**
 * The task's own property values, such as a context or a start date.
 */
properties: { [key in string]: OpaqueJson }, };
export type TaskStatus = "open" | "completed";
export type TextKind = "text";
export type TextMode = "terms" | "phrase";
export type ThemeOptionValue = boolean | string;
export type ThemeOrigin = "builtin" | "project";
export type ThemePreviewSummary = { themeId: string, revision: string, draftSha256: string, };
export type ThemeSnapshot = { id: string, version: string, name: string, origin: ThemeOrigin, sha256: string, executionSha256: string, format: PublicationFormat, definition: PublicationThemeDefinition, overrides: PublicationThemeOverrides, resolved: ResolvedPublicationTheme, options: { [key in string]: ThemeOptionValue }, requiresTrust: boolean, };
export type ThemeTrust = { hash: string, trusted: boolean, root: string, };
export type TrustInput = { fingerprint: string,
/**
 * Must match the prior stored trust when explicitly replacing a key.
 */
previousFingerprint: string | null, };
export type TrustSyncHost = { origin: string, trust: TrustInput, };
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
optionAppearance?: { [key in string]: ChoiceAppearance },
/**
 * Complete authored order, including any options added in this request.
 */
optionOrder?: Array<string>,
/**
 * Required when reordering to avoid overwriting another editor's changes.
 */
expectedRevision?: string, };
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
cards?: OpaqueJson, table?: ViewTablePresentation, kanban?: ViewKanbanPresentation, } | { "kind": "tasks", status: TaskStatus, count: number,
/**
 * Conditions on the tasks themselves, such as a context or a start date.
 */
query?: QueryDocument,
/**
 * One column per choice of this task property, such as a context.
 */
groupBy?: string,
/**
 * Only this page's tasks, with a field to add more, such as an inbox.
 */
noteId?: string, } | { "kind": "review", query: QueryDocument, field: string, days: number, } | { "kind": "writing_activity", days: number, } | { "kind": "publications", publicationIds: Array<string>, count: number, } | { "kind": "introduction", title: string, description: string, } | { "kind": "writing_stats", stats: Array<WritingStat>, days: number, } | { "kind": "writing_goal", } | { "kind": "create_buttons", templateIds: Array<string>, } | { "kind": "count", query: QueryDocument, } | { "kind": "writing_rhythm", days: number, };
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
  cancel_export: { method: 'POST'; request: JobRequest; response: Cancelled }
  checkpoint_history: { method: 'POST'; request: EmptyRequest; response: HistoryStatus }
  clone_project: { method: 'POST'; request: CloneProject; response: ClonedProject }
  configure_sync: { method: 'POST'; request: ConfigureSync; response: SyncStatus }
  configure_sync_auth: { method: 'POST'; request: ConfigureSyncAuth; response: AuthStatus }
  create_export: { method: 'POST'; request: ExportRequest; response: JobId }
  create_facet: { method: 'POST'; request: FacetRequest; response: FacetEntry }
  create_item: { method: 'POST'; request: CreateItemRequest; response: PublicItem }
  create_named_version: { method: 'POST'; request: NamedVersionRequest; response: NamedVersion }
  create_property: { method: 'POST'; request: CreatePropertyRequest; response: MetadataField }
  create_publication_theme: { method: 'POST'; request: CreatePublicationThemeRequest; response: PublicationThemeRecord }
  create_snippet: { method: 'POST'; request: CreateSnippetRequest; response: SnippetRecord }
  create_workspace_from_setup: { method: 'POST'; request: CreateWorkspaceFromSetupRequest; response: PublicItem }
  create_writing_goal: { method: 'POST'; request: CreateWritingGoalRequest; response: PublicItem }
  delete_item: { method: 'POST'; request: DeleteItemRequest; response: DeleteResult }
  delete_publication_theme: { method: 'POST'; request: DeletePublicationThemeRequest; response: PublicationThemeDeleted }
  download_history_assets: { method: 'POST'; request: EmptyRequest; response: SyncStatus }
  duplicate_publication_theme: { method: 'POST'; request: DuplicatePublicationThemeRequest; response: PublicationThemeRecord }
  generate_sync_key: { method: 'POST'; request: GenerateSyncKey; response: GeneratedKey }
  history_client_activity: { method: 'POST'; request: HistoryClientActivityRequest; response: HistoryClientActivity }
  inspect_sync_host: { method: 'POST'; request: SyncOrigin; response: HostIdentity }
  list_decisions: { method: 'POST'; request: DecisionsRequest; response: DecisionsResult }
  list_facets: { method: 'POST'; request: ListFacetsRequest; response: Array<FacetEntry> }
  list_publication_themes: { method: 'POST'; request: EmptyRequest; response: Array<PublicationThemeSummary> }
  merge_item_draft: { method: 'POST'; request: MergeDraftRequest; response: MergeDraftResult }
  mutate_note_task: { method: 'POST'; request: MutateNoteTaskRequest; response: PublicItem }
  preview_decision: { method: 'POST'; request: DecisionRequest; response: DecisionPreview }
  preview_history_restore: { method: 'POST'; request: HistoryPreviewRequest; response: HistoryPreview }
  preview_publication_theme: { method: 'POST'; request: PreviewPublicationThemeRequest; response: JobId }
  query_items: { method: 'POST'; request: QueryItemsRequest; response: QueryItemsResult }
  query_label_passages: { method: 'POST'; request: QueryPassagesRequest; response: ResultPage<PassageRecord> }
  query_note_tasks: { method: 'POST'; request: QueryTasksRequest; response: ResultPage<TaskRecord> }
  query_snippets: { method: 'POST'; request: QuerySnippetsRequest; response: SnippetResults }
  read_calendar_buckets: { method: 'POST'; request: CalendarRequest; response: CalendarResults }
  read_changes: { method: 'POST'; request: ReadChangesRequest; response: ReadChangesResult }
  read_dashboard_analytics: { method: 'POST'; request: AnalyticsRequest; response: DashboardAnalytics }
  read_dashboard_day_details: { method: 'POST'; request: DayDetailsRequest; response: DashboardDayDetails }
  read_export: { method: 'POST'; request: ReadRequest; response: Status }
  read_facet_counts: { method: 'POST'; request: FacetCountsRequest; response: Array<FacetCount> }
  read_history: { method: 'POST'; request: HistoryRequest; response: HistoryPage }
  read_history_download: { method: 'POST'; request: EmptyRequest; response: HistoryDownload }
  read_history_item: { method: 'POST'; request: HistoryItemRequest; response: PublicItem }
  read_history_status: { method: 'POST'; request: EmptyRequest; response: HistoryStatus }
  read_item: { method: 'POST'; request: ReadItemRequest; response: PublicItem }
  read_items: { method: 'POST'; request: ReadItemsRequest; response: Array<PublicItem> }
  read_metadata_fields: { method: 'POST'; request: EmptyRequest; response: Array<MetadataField> }
  read_page_export_issues: { method: 'POST'; request: PageIssuesRequest; response: Array<FactIssue> }
  read_publication_theme: { method: 'POST'; request: ReadThemeRequest; response: ThemeSnapshot }
  read_publication_theme_package: { method: 'POST'; request: ReadPublicationThemePackageRequest; response: PublicationThemeRecord }
  read_query_capabilities: { method: 'POST'; request: EmptyRequest; response: QueryCapabilities }
  read_snippet: { method: 'POST'; request: ReadSnippetRequest; response: SnippetRecord | null }
  read_snippet_usages: { method: 'POST'; request: ReadSnippetRequest; response: Array<SnippetUsage> }
  read_storage_status: { method: 'GET'; request: undefined; response: StorageStatus }
  read_sync_auth: { method: 'POST'; request: SyncOrigin; response: AuthStatus }
  read_sync_status: { method: 'POST'; request: EmptyRequest; response: SyncStatus }
  read_workspace_context: { method: 'POST'; request: EmptyRequest; response: WorkspaceContext }
  remove_facet: { method: 'POST'; request: FacetRequest; response: Removed }
  remove_property: { method: 'POST'; request: RemovePropertyRequest; response: Removed }
  remove_workspace: { method: 'POST'; request: RemoveWorkspaceRequest; response: WorkspaceContext }
  renew_editor_base: { method: 'POST'; request: RenewEditorBaseRequest; response: EditorBaseLease }
  resolve_decision: { method: 'POST'; request: ResolveDecisionRequest; response: ResolveDecisionResult }
  resolve_workspace_notes: { method: 'POST'; request: ResolveWorkspaceNotesRequest; response: Array<PublicItem> }
  restore_history: { method: 'POST'; request: HistoryRestoreRequest; response: HistoryStatus }
  set_item_tags: { method: 'POST'; request: SetItemTagsRequest; response: PublicItem }
  set_note_pin: { method: 'POST'; request: SetNotePinRequest; response: PinResult }
  start_export: { method: 'POST'; request: JobRequest; response: JobId }
  start_export_preview: { method: 'POST'; request: JobRequest; response: PreviewUrl }
  sync_project: { method: 'POST'; request: EmptyRequest; response: SyncStatus }
  trust_sync_host: { method: 'POST'; request: TrustSyncHost; response: AuthStatus }
  update_item: { method: 'POST'; request: UpdateItemRequest; response: PublicItem }
  update_item_meta: { method: 'POST'; request: UpdateItemMetaRequest; response: PublicItem }
  update_property_options: { method: 'POST'; request: UpdatePropertyOptionsRequest; response: MetadataField }
  update_publication_theme: { method: 'POST'; request: UpdatePublicationThemeRequest; response: PublicationThemeRecord }
  update_snippet: { method: 'POST'; request: UpdateSnippetRequest; response: SnippetRecord }
  validate_publication_theme: { method: 'POST'; request: UpdatePublicationThemeRequest; response: PublicationThemeValidation }
  validate_query: { method: 'POST'; request: DocumentQueryRequest; response: QueryValidation }
}
