// SPDX-License-Identifier: MIT
// See backend/contracts/LICENSE.
// Generated from Rust; do not edit.
// Run npm run contracts:generate from the repository root.

export const rpcMethods = {
  approve_export_theme: 'POST',
  archive_snippet: 'POST',
  cache_status: 'GET',
  cancel_export: 'POST',
  create_export: 'POST',
  create_facet: 'POST',
  create_item: 'POST',
  create_property: 'POST',
  create_publication_theme: 'POST',
  create_snippet: 'POST',
  create_workspace_from_setup: 'POST',
  create_writing_goal: 'POST',
  delete_item: 'POST',
  delete_publication_theme: 'POST',
  duplicate_publication_theme: 'POST',
  list_facets: 'POST',
  list_publication_themes: 'POST',
  mutate_note_task: 'POST',
  preview_publication_theme: 'POST',
  query_items: 'POST',
  query_label_passages: 'POST',
  query_note_tasks: 'POST',
  query_snippets: 'POST',
  read_calendar_buckets: 'POST',
  read_dashboard_analytics: 'POST',
  read_dashboard_day_details: 'POST',
  read_export: 'POST',
  read_facet_counts: 'POST',
  read_item: 'POST',
  read_items: 'POST',
  read_metadata_fields: 'POST',
  read_publication_theme: 'POST',
  read_publication_theme_package: 'POST',
  read_query_capabilities: 'POST',
  read_snippet: 'POST',
  read_snippet_usages: 'POST',
  read_workspace_context: 'POST',
  remove_facet: 'POST',
  remove_property: 'POST',
  remove_workspace: 'POST',
  resolve_workspace_notes: 'POST',
  set_default_workspace: 'POST',
  set_item_tags: 'POST',
  set_note_pin: 'POST',
  start_export: 'POST',
  start_export_preview: 'POST',
  update_item: 'POST',
  update_item_meta: 'POST',
  update_property_options: 'POST',
  update_publication_theme: 'POST',
  update_snippet: 'POST',
  validate_publication_theme: 'POST',
  validate_query: 'POST',
} as const

export const ItemType = {
  PAGE: 'page',
  WORKSPACE: 'workspace',
  GOAL: 'goal',
  WIDGET: 'widget',
  QUERY: 'query',
  SETTING: 'setting',
  PUBLICATION: 'publication',
  TASK: 'task',
  TAG: 'tag',
  LABEL: 'label',
  PROPERTY: 'property',
  SNIPPET: 'snippet',
  TEMPLATE: 'template',
} as const

export const ItemState = {
  ACTIVE: 'active',
  ARCHIVED: 'archived',
} as const

export const FacetKind = {
  TAG: 'tag',
  LABEL: 'label',
} as const

export const CoverGradient = {
  DAWN: 'dawn',
  SEA: 'sea',
  MEADOW: 'meadow',
  DUSK: 'dusk',
  BLOOM: 'bloom',
  GROVE: 'grove',
} as const

export const homeWidgetConstraints = [{"kind":"continue_writing","widths":[4,6,8,12],"heights":["compact","regular","tall"],"settings":[{"key":"noteId","schema":{"kind":"item","itemType":"page","optional":true}}]},{"kind":"recent_notes","widths":[4,6,8,12],"heights":["compact","regular","tall"],"settings":[{"key":"count","schema":{"kind":"integer","min":1,"max":50}},{"key":"presentation","schema":{"kind":"choice","options":["cards","list"]}}]},{"kind":"workspace_shortcuts","widths":[4,6,8,12],"heights":["compact","regular","tall"],"settings":[{"key":"workspaceIds","schema":{"kind":"items","itemType":"workspace"}}]},{"kind":"pinned_notes","widths":[4,6,8,12],"heights":["compact","regular","tall"],"settings":[{"key":"count","schema":{"kind":"integer","min":1,"max":50}},{"key":"presentation","schema":{"kind":"choice","options":["cards","list"]}}]},{"kind":"layout","widths":[6,8,12],"heights":["compact","regular","tall"],"settings":[{"key":"query","schema":{"kind":"query"}},{"key":"mode","schema":{"kind":"choice","options":["cards","table","kanban","list"]}}]},{"kind":"tasks","widths":[4,6,8,12],"heights":["compact","regular","tall"],"settings":[{"key":"status","schema":{"kind":"choice","options":["open","completed"]}},{"key":"count","schema":{"kind":"integer","min":1,"max":50}}]},{"kind":"writing_activity","widths":[4,6,8,12],"heights":["compact","regular","tall"],"settings":[{"key":"days","schema":{"kind":"choice","options":[7,30,90,365]}}]},{"kind":"publications","widths":[4,6,8,12],"heights":["compact","regular","tall"],"settings":[{"key":"publicationIds","schema":{"kind":"items","itemType":"publication"}},{"key":"count","schema":{"kind":"integer","min":1,"max":50}}]},{"kind":"writing_stats","widths":[4,6,8,12],"heights":["compact","regular","tall"],"settings":[{"key":"stats","schema":{"kind":"choices","options":["words_today","streak","active_days","words_added","net_change","tasks_completed"]}},{"key":"days","schema":{"kind":"choice","options":[7,30,90,365]}}]},{"kind":"writing_rhythm","widths":[6,8,12],"heights":["compact","regular","tall"],"settings":[{"key":"days","schema":{"kind":"choice","options":[7,30,90,365]}}]},{"kind":"writing_goal","widths":[4,6,8,12],"heights":["compact","regular","tall"],"settings":[]},{"kind":"create_buttons","widths":[4,6,8,12],"heights":["compact","regular","tall"],"settings":[{"key":"templateIds","schema":{"kind":"items","itemType":"template"}}]},{"kind":"count","widths":[4,6,8,12],"heights":["compact","regular","tall"],"settings":[{"key":"query","schema":{"kind":"query"}}]},{"kind":"introduction","widths":[4,6,8,12],"heights":["compact","regular","tall"],"settings":[{"key":"title","schema":{"kind":"text","maxLength":160}},{"key":"description","schema":{"kind":"text","maxLength":10000}}]}] satisfies import('./types.js').WidgetConstraint[]
export const homeWidgetTitleLength = 80
export const assetImageWidths = [160, 320, 640, 1280] as const
