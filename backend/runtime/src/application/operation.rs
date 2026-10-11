// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! One registry owns dispatch and the generated browser RPC contract.
use super::error::{Result, invalid_request};
use super::request::decode;
use super::{
    Application, analytics, catalogue_writes, items, mutations, queries, settings, snippets,
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
    "create_export", "POST", CreateExport(crate::engine::publication::request::ExportRequest) -> crate::export::model::JobId => Application::create_export;
    "read_export", "POST", ReadExport(crate::export::model::ReadRequest) -> crate::export::model::Status => Application::read_export;
    "start_export", "POST", StartExport(crate::export::model::JobRequest) -> crate::export::model::JobId => Application::start_export;
    "approve_export_theme", "POST", ApproveExport(crate::export::model::ApproveRequest) -> crate::export::model::Approved => Application::approve_export;
    "cancel_export", "POST", CancelExport(crate::export::model::JobRequest) -> crate::export::model::Cancelled => Application::cancel_export;
    "start_export_preview", "POST", PreviewExport(crate::export::model::JobRequest) -> crate::export::model::PreviewUrl => Application::preview_export;
    "read_dashboard_analytics", "POST", Analytics(analytics::AnalyticsRequest) -> crate::engine::project::reports::DashboardAnalytics => Application::dashboard_analytics;
    "read_dashboard_day_details", "POST", DayDetails(analytics::DayDetailsRequest) -> crate::engine::project::reports::DashboardDayDetails => Application::dashboard_day_details;
    "create_snippet", "POST", CreateSnippet(snippets::CreateSnippetRequest) -> crate::snippets::SnippetRecord => Application::create_snippet;
    "update_snippet", "POST", UpdateSnippet(snippets::UpdateSnippetRequest) -> crate::snippets::SnippetRecord => Application::update_snippet;
    "read_snippet", "POST", ReadSnippet(snippets::ReadSnippetRequest) -> Option<crate::snippets::SnippetRecord> => Application::read_snippet;
    "archive_snippet", "POST", ArchiveSnippet(snippets::ArchiveSnippetRequest) -> crate::snippets::SnippetRecord => Application::archive_snippet;
    "query_snippets", "POST", QuerySnippets(snippets::QuerySnippetsRequest) -> crate::snippets::SnippetResults => Application::query_snippets;
    "read_snippet_usages", "POST", SnippetUsages(snippets::ReadSnippetRequest) -> Vec<crate::snippets::SnippetUsage> => Application::snippet_usages;
    "read_item", "POST", ReadItem(items::ReadItemRequest) -> items::PublicItem => Application::read_item;
    "read_items", "POST", ReadItems(items::ReadItemsRequest) -> Vec<items::PublicItem> => Application::read_items;
    "query_items", "POST", QueryItems(items::QueryItemsRequest) -> items::QueryItemsResult => Application::query_items;
    "create_item", "POST", CreateItem(mutations::CreateItemRequest) -> items::PublicItem => Application::create_item;
    "update_item", "POST", UpdateItem(mutations::UpdateItemRequest) -> items::PublicItem => Application::update_item;
    "update_item_meta", "POST", UpdateItemMeta(mutations::UpdateItemMetaRequest) -> items::PublicItem => Application::update_item_meta;
    "set_item_tags", "POST", SetItemTags(mutations::SetItemTagsRequest) -> items::PublicItem => Application::set_item_tags;
    "mutate_note_task", "POST", MutateNoteTask(mutations::MutateNoteTaskRequest) -> items::PublicItem => Application::mutate_note_task;
    "create_writing_goal", "POST", CreateWritingGoal(mutations::CreateWritingGoalRequest) -> items::PublicItem => Application::create_writing_goal;
    "set_note_pin", "POST", SetNotePin(mutations::SetNotePinRequest) -> mutations::PinResult => Application::set_note_pin;
    "delete_item", "POST", DeleteItem(mutations::DeleteItemRequest) -> mutations::DeleteResult => Application::delete_item;
    "query_note_tasks", "POST", QueryTasks(queries::QueryTasksRequest) -> crate::model::ResultPage<crate::model::TaskRecord> => Application::query_tasks;
    "query_label_passages", "POST", QueryPassages(queries::QueryPassagesRequest) -> crate::model::ResultPage<crate::model::PassageRecord> => Application::query_passages;
    "analyze_query", "POST", AnalyzeQuery(queries::AnalyzeQueryRequest) -> crate::query::QueryAnalysis => Application::analyze_query;
    "read_calendar_buckets", "POST", Calendar(queries::CalendarRequest) -> crate::model::CalendarResults => Application::calendar_buckets;
    "read_metadata_fields", "POST", MetadataFields(queries::EmptyRequest) -> Vec<crate::catalogue::MetadataField> => |app: &Application, _: queries::EmptyRequest| app.metadata_fields();
    "list_facets", "POST", ListFacets(queries::ListFacetsRequest) -> Vec<crate::catalogue::FacetEntry> => Application::list_facets;
    "read_facet_counts", "POST", FacetCounts(queries::FacetCountsRequest) -> Vec<queries::FacetCount> => Application::facet_counts;
    "cache_status", "GET", CacheStatus(queries::EmptyRequest) -> queries::CacheStatus => |app: &Application, _: queries::EmptyRequest| app.cache_status();
    "create_property", "POST", CreateProperty(catalogue_writes::CreatePropertyRequest) -> crate::catalogue::MetadataField => Application::create_property;
    "add_property_options", "POST", AddPropertyOptions(catalogue_writes::AddPropertyOptionsRequest) -> crate::catalogue::MetadataField => Application::add_property_options;
    "remove_property", "POST", RemoveProperty(catalogue_writes::RemovePropertyRequest) -> catalogue_writes::Removed => Application::remove_property;
    "create_facet", "POST", CreateFacet(catalogue_writes::FacetRequest) -> crate::catalogue::FacetEntry => Application::create_facet;
    "remove_facet", "POST", RemoveFacet(catalogue_writes::FacetRequest) -> catalogue_writes::Removed => Application::remove_facet;
    "read_default_view", "POST", DefaultView(queries::EmptyRequest) -> crate::engine::project::types::ViewContent => |app: &Application, _: queries::EmptyRequest| app.read_default_view();
    "update_default_view", "POST", UpdateDefaultView(settings::UpdateDefaultViewRequest) -> crate::engine::project::types::ViewContent => Application::update_default_view;
    "read_publication_theme", "POST", Theme(settings::ReadThemeRequest) -> crate::engine::publication::theme::ThemeSnapshot => Application::read_theme;
}
