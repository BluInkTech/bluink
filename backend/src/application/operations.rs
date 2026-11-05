// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! One registry owns dispatch and the generated browser RPC contract.
use super::{
    Application, analytics, catalogue_writes, changes, collections, drafts,
    error::{Result, invalid_request},
    item_writes, publishing, query_documents,
    request::decode,
    settings, snippets, themes,
};
use serde::Serialize;

macro_rules! operations {
    ($($name:literal, $method:literal, $variant:ident($request:ty) -> $response:ty => $handler:expr;)+) => {
        pub(crate) enum Request { $($variant($request)),+ }
        #[derive(Debug, Serialize)]
        #[serde(untagged)]
        pub(crate) enum Response { $($variant(Box<$response>)),+ }
        impl Request {
            pub(crate) fn http_method(operation: &str) -> Option<&'static str> {
                match operation { $($name => Some($method),)+ _ => None }
            }
            pub(crate) fn decode(operation: &str, source: &[u8]) -> Result<Self> {
                match operation {
                    $($name => decode(source).map(Self::$variant),)+
                    _ => Err(invalid_request("Unknown application operation.")),
                }
            }
        }
        impl Application {
            pub(crate) fn execute(&self, request: Request) -> Result<Response> {
                match request { $(Request::$variant(request) => {
                    let result: Result<$response> = ($handler)(self, request);
                    result.map(|response| Response::$variant(Box::new(response)))
                }),+ }
            }
        }
        #[cfg(test)]
        pub(crate) fn contracts(registry: &mut crate::contracts::Registry) {
            $(registry.operation::<$request, $response>($name, $method);)+
        }
    };
}

operations! {
    "read_changes", "POST", ReadChanges(crate::store::changes::ReadChangesRequest) -> crate::store::changes::ReadChangesResult => Application::read_changes;
    "renew_editor_base", "POST", RenewEditorBase(changes::RenewEditorBaseRequest) -> changes::EditorBaseLease => Application::renew_editor_base;
    "generate_sync_key", "POST", GenerateSyncKey(crate::versioning::sync::auth::GenerateSyncKey) -> crate::versioning::transport::model::GeneratedKey => Application::generate_sync_key;
    "read_history_download", "POST", ReadHistoryDownload(collections::EmptyRequest) -> crate::versioning::sync::model::HistoryDownload => Application::read_history_download;
    "merge_item_draft", "POST", MergeDraft(crate::records::drafts::MergeDraftRequest) -> crate::records::drafts::MergeDraftResult => Application::merge_item_draft;
    "list_decisions", "POST", ListDecisions(drafts::DecisionsRequest) -> drafts::DecisionsResult => Application::list_decisions;
    "preview_decision", "POST", PreviewDecision(drafts::DecisionRequest) -> drafts::DecisionPreview => Application::preview_decision;
    "resolve_decision", "POST", ResolveDecision(drafts::ResolveDecisionRequest) -> drafts::ResolveDecisionResult => Application::resolve_decision;
    "read_sync_auth", "POST", ReadSyncAuth(crate::versioning::sync::auth::SyncOrigin) -> crate::versioning::transport::model::AuthStatus => Application::read_sync_auth;
    "configure_sync_auth", "POST", ConfigureSyncAuth(crate::versioning::sync::auth::ConfigureSyncAuth) -> crate::versioning::transport::model::AuthStatus => Application::configure_sync_auth;
    "inspect_sync_host", "POST", InspectSyncHost(crate::versioning::sync::auth::SyncOrigin) -> crate::versioning::transport::model::HostIdentity => Application::inspect_sync_host;
    "trust_sync_host", "POST", TrustSyncHost(crate::versioning::sync::auth::TrustSyncHost) -> crate::versioning::transport::model::AuthStatus => Application::trust_sync_host;
    "read_sync_status", "POST", SyncStatus(collections::EmptyRequest) -> crate::versioning::sync::model::SyncStatus => Application::read_sync_status;
    "configure_sync", "POST", ConfigureSync(crate::versioning::sync::model::ConfigureSync) -> crate::versioning::sync::model::SyncStatus => Application::configure_sync;
    "sync_project", "POST", SyncProject(collections::EmptyRequest) -> crate::versioning::sync::model::SyncStatus => Application::sync_project;
    "download_history_assets", "POST", DownloadHistoryAssets(collections::EmptyRequest) -> crate::versioning::sync::model::SyncStatus => Application::download_history_assets;
    "clone_project", "POST", CloneProject(crate::versioning::sync::model::CloneProject) -> crate::versioning::sync::model::ClonedProject => Application::clone_project;
    "read_history_status", "POST", HistoryStatus(collections::EmptyRequest) -> crate::versioning::model::HistoryStatus => Application::history_status;
    "checkpoint_history", "POST", CheckpointHistory(collections::EmptyRequest) -> crate::versioning::model::HistoryStatus => Application::checkpoint_history;
    "read_history", "POST", ReadHistory(crate::versioning::model::HistoryRequest) -> crate::versioning::model::HistoryPage => Application::read_history;
    "read_history_item", "POST", ReadHistoryItem(crate::versioning::model::HistoryItemRequest) -> crate::records::items::PublicItem => Application::read_history_item;
    "create_named_version", "POST", CreateNamedVersion(crate::versioning::model::NamedVersionRequest) -> crate::versioning::model::NamedVersion => Application::create_named_version;
    "preview_history_restore", "POST", PreviewHistoryRestore(crate::versioning::model::HistoryPreviewRequest) -> crate::versioning::model::HistoryPreview => Application::preview_history_restore;
    "restore_history", "POST", RestoreHistory(crate::versioning::model::HistoryRestoreRequest) -> crate::versioning::model::HistoryStatus => Application::restore_history;
    "history_client_activity", "POST", HistoryClientActivity(crate::versioning::model::HistoryClientActivityRequest) -> crate::versioning::model::HistoryClientActivity => Application::history_client_activity;
    "create_export", "POST", CreateExport(crate::domain::publication::request::ExportRequest) -> crate::export_jobs::model::JobId => Application::create_export;
    "read_export", "POST", ReadExport(crate::export_jobs::model::ReadRequest) -> crate::export_jobs::model::Status => Application::read_export;
    "start_export", "POST", StartExport(crate::export_jobs::model::JobRequest) -> crate::export_jobs::model::JobId => Application::start_export;
    "approve_export_theme", "POST", ApproveExport(crate::export_jobs::model::ApproveRequest) -> crate::export_jobs::model::Approved => Application::approve_export;
    "cancel_export", "POST", CancelExport(crate::export_jobs::model::JobRequest) -> crate::export_jobs::model::Cancelled => Application::cancel_export;
    "start_export_preview", "POST", PreviewExport(crate::export_jobs::model::JobRequest) -> crate::export_jobs::model::PreviewUrl => Application::preview_export;
    "read_page_export_issues", "POST", PageExportIssues(publishing::PageIssuesRequest) -> Vec<crate::domain::publication::preflight::FactIssue> => Application::page_export_issues;
    "read_dashboard_analytics", "POST", Analytics(analytics::AnalyticsRequest) -> crate::domain::project::reports::DashboardAnalytics => Application::dashboard_analytics;
    "read_dashboard_day_details", "POST", DayDetails(analytics::DayDetailsRequest) -> crate::domain::project::reports::DashboardDayDetails => Application::dashboard_day_details;
    "create_snippet", "POST", CreateSnippet(snippets::CreateSnippetRequest) -> crate::records::SnippetRecord => Application::create_snippet;
    "update_snippet", "POST", UpdateSnippet(snippets::UpdateSnippetRequest) -> crate::records::SnippetRecord => Application::update_snippet;
    "read_snippet", "POST", ReadSnippet(snippets::ReadSnippetRequest) -> Option<crate::records::SnippetRecord> => Application::read_snippet;
    "archive_snippet", "POST", ArchiveSnippet(snippets::ArchiveSnippetRequest) -> crate::records::SnippetRecord => Application::archive_snippet;
    "query_snippets", "POST", QuerySnippets(snippets::QuerySnippetsRequest) -> crate::records::SnippetResults => Application::query_snippets;
    "read_snippet_usages", "POST", SnippetUsages(snippets::ReadSnippetRequest) -> Vec<crate::records::SnippetUsage> => Application::snippet_usages;
    "read_item", "POST", ReadItem(crate::records::items::ReadItemRequest) -> crate::records::items::PublicItem => Application::read_item;
    "read_items", "POST", ReadItems(crate::records::items::ReadItemsRequest) -> Vec<crate::records::items::PublicItem> => Application::read_items;
    "query_items", "POST", QueryItems(crate::records::items::QueryItemsRequest) -> crate::records::items::QueryItemsResult => Application::query_items;
    "resolve_workspace_notes", "POST", ResolveWorkspaceNotes(crate::records::items::ResolveWorkspaceNotesRequest) -> Vec<crate::records::items::PublicItem> => Application::resolve_workspace_notes;
    "create_item", "POST", CreateItem(item_writes::CreateItemRequest) -> crate::records::items::PublicItem => Application::create_item;
    "update_item", "POST", UpdateItem(item_writes::UpdateItemRequest) -> crate::records::items::PublicItem => Application::update_item;
    "update_item_meta", "POST", UpdateItemMeta(item_writes::UpdateItemMetaRequest) -> crate::records::items::PublicItem => Application::update_item_meta;
    "set_item_tags", "POST", SetItemTags(item_writes::SetItemTagsRequest) -> crate::records::items::PublicItem => Application::set_item_tags;
    "mutate_note_task", "POST", MutateNoteTask(item_writes::MutateNoteTaskRequest) -> crate::records::items::PublicItem => Application::mutate_note_task;
    "create_writing_goal", "POST", CreateWritingGoal(item_writes::CreateWritingGoalRequest) -> crate::records::items::PublicItem => Application::create_writing_goal;
    "create_workspace_from_setup", "POST", CreateWorkspaceFromSetup(item_writes::CreateWorkspaceFromSetupRequest) -> crate::records::items::PublicItem => Application::create_workspace_from_setup;
    "set_note_pin", "POST", SetNotePin(item_writes::SetNotePinRequest) -> item_writes::PinResult => Application::set_note_pin;
    "delete_item", "POST", DeleteItem(item_writes::DeleteItemRequest) -> item_writes::DeleteResult => Application::delete_item;
    "query_note_tasks", "POST", QueryTasks(collections::QueryTasksRequest) -> crate::records::ResultPage<crate::records::TaskRecord> => Application::query_tasks;
    "query_label_passages", "POST", QueryPassages(collections::QueryPassagesRequest) -> crate::records::ResultPage<crate::records::PassageRecord> => Application::query_passages;
    "validate_query", "POST", ValidateQuery(query_documents::DocumentQueryRequest) -> query_documents::QueryValidation => Application::validate_query;
    "read_query_capabilities", "POST", QueryCapabilities(collections::EmptyRequest) -> query_documents::QueryCapabilities => |app: &Application, _: collections::EmptyRequest| app.query_capabilities();
    "read_calendar_buckets", "POST", Calendar(collections::CalendarRequest) -> crate::records::CalendarResults => Application::calendar_buckets;
    "read_metadata_fields", "POST", MetadataFields(collections::EmptyRequest) -> Vec<crate::catalogue::MetadataField> => |app: &Application, _: collections::EmptyRequest| app.metadata_fields();
    "list_facets", "POST", ListFacets(collections::ListFacetsRequest) -> Vec<crate::catalogue::FacetEntry> => Application::list_facets;
    "read_facet_counts", "POST", FacetCounts(collections::FacetCountsRequest) -> Vec<collections::FacetCount> => Application::facet_counts;
    "read_storage_status", "GET", StorageStatus(collections::EmptyRequest) -> collections::StorageStatus => |app: &Application, _: collections::EmptyRequest| app.storage_status();
    "create_property", "POST", CreateProperty(catalogue_writes::CreatePropertyRequest) -> crate::catalogue::MetadataField => Application::create_property;
    "update_property_options", "POST", UpdatePropertyOptions(catalogue_writes::UpdatePropertyOptionsRequest) -> crate::catalogue::MetadataField => Application::update_property_options;
    "remove_property", "POST", RemoveProperty(catalogue_writes::RemovePropertyRequest) -> catalogue_writes::Removed => Application::remove_property;
    "create_facet", "POST", CreateFacet(catalogue_writes::FacetRequest) -> crate::catalogue::FacetEntry => Application::create_facet;
    "remove_facet", "POST", RemoveFacet(catalogue_writes::FacetRequest) -> catalogue_writes::Removed => Application::remove_facet;
    "read_workspace_context", "POST", WorkspaceContext(collections::EmptyRequest) -> settings::WorkspaceContext => |app: &Application, _: collections::EmptyRequest| app.workspace_context();
    "remove_workspace", "POST", RemoveWorkspace(settings::RemoveWorkspaceRequest) -> settings::WorkspaceContext => Application::remove_workspace;
    "read_publication_theme", "POST", Theme(settings::ReadThemeRequest) -> crate::domain::publication::theme::ThemeSnapshot => Application::read_theme;
    "list_publication_themes", "POST", ListThemes(collections::EmptyRequest) -> Vec<crate::export_jobs::theme_store::Summary> => Application::list_themes;
    "read_publication_theme_package", "POST", ReadThemePackage(themes::ReadRequest) -> crate::export_jobs::theme_store::Record => Application::read_theme_package;
    "create_publication_theme", "POST", CreateTheme(themes::CreateRequest) -> crate::export_jobs::theme_store::Record => Application::create_theme;
    "duplicate_publication_theme", "POST", DuplicateTheme(themes::DuplicateRequest) -> crate::export_jobs::theme_store::Record => Application::duplicate_theme;
    "update_publication_theme", "POST", UpdateTheme(themes::UpdateRequest) -> crate::export_jobs::theme_store::Record => Application::update_theme;
    "validate_publication_theme", "POST", ValidateTheme(themes::UpdateRequest) -> crate::export_jobs::theme_store::Validation => Application::validate_theme;
    "delete_publication_theme", "POST", DeleteTheme(themes::DeleteRequest) -> themes::Deleted => Application::delete_theme;
    "preview_publication_theme", "POST", PreviewTheme(themes::PreviewRequest) -> crate::export_jobs::model::JobId => Application::preview_theme;
}
