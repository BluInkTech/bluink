// SPDX-License-Identifier: MIT
// See backend/contracts/LICENSE.
// Generated from Rust; do not edit.
// Run npm run contracts:generate from the repository root.

export const rpcMethods = {
  add_property_options: 'POST',
  analyze_query: 'POST',
  approve_export_theme: 'POST',
  archive_snippet: 'POST',
  cache_status: 'GET',
  cancel_export: 'POST',
  create_export: 'POST',
  create_facet: 'POST',
  create_item: 'POST',
  create_property: 'POST',
  create_snippet: 'POST',
  create_writing_goal: 'POST',
  delete_item: 'POST',
  list_facets: 'POST',
  mutate_note_task: 'POST',
  query_items: 'POST',
  query_label_passages: 'POST',
  query_note_tasks: 'POST',
  query_snippets: 'POST',
  read_calendar_buckets: 'POST',
  read_dashboard_analytics: 'POST',
  read_dashboard_day_details: 'POST',
  read_default_view: 'POST',
  read_export: 'POST',
  read_facet_counts: 'POST',
  read_item: 'POST',
  read_items: 'POST',
  read_metadata_fields: 'POST',
  read_publication_theme: 'POST',
  read_snippet: 'POST',
  read_snippet_usages: 'POST',
  remove_facet: 'POST',
  remove_property: 'POST',
  set_item_tags: 'POST',
  set_note_pin: 'POST',
  start_export: 'POST',
  start_export_preview: 'POST',
  update_default_view: 'POST',
  update_item: 'POST',
  update_item_meta: 'POST',
  update_snippet: 'POST',
} as const

export const ItemType = {
  PAGE: 'page',
  WORKSPACE: 'workspace',
  GOAL: 'goal',
  WIDGET: 'widget',
  QUERY: 'query',
  SETTING: 'setting',
  PUBLICATION: 'publication',
} as const

export const ItemState = {
  ACTIVE: 'active',
  ARCHIVED: 'archived',
} as const

export const FacetKind = {
  TAG: 'tag',
  LABEL: 'label',
} as const

export const PublicSystemItemId = { NOTES_ORDER: '00000000-0000-7000-8000-000000000001' } as const
