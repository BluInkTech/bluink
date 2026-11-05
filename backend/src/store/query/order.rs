// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::Compiler;
use crate::domain::project::query::{QueryDocument, SortDirection};
use crate::domain::project::types::MetadataFieldValueType as Type;

impl Compiler {
    pub(super) fn order(&mut self, document: &QueryDocument, score: &str) -> Vec<String> {
        if document.sort.is_empty() {
            return if self.rank_text {
                vec![format!("{score} DESC"), "i.uuid ASC".into()]
            } else {
                vec!["i.modified_ms DESC".into(), "i.uuid ASC".into()]
            };
        }
        let mut order = Vec::new();
        for sort in &document.sort {
            let direction = if sort.direction == SortDirection::Desc {
                "DESC"
            } else {
                "ASC"
            };
            let builtin = match sort.field.as_str() {
                "id" => Some("i.uuid"),
                "title" => Some("i.title_sort"),
                "type" => Some("i.type"),
                "state" => Some("i.state"),
                "created" => Some("i.created_ms"),
                "modified" => Some("i.modified_ms"),
                _ => None,
            };
            if let Some(column) = builtin {
                order.push(format!("{column} {direction}"));
                continue;
            }
            if let Some(fact) = super::compile::task_fact(&sort.field) {
                order.push(format!("{fact} {direction} NULLS LAST"));
                continue;
            }
            let Some(definition) = self.fields.get(&sort.field).cloned() else {
                continue;
            };
            let field = self.bind(definition.id);
            let column = match definition.kind {
                Type::Select | Type::MultiSelect => {
                    // Each correlated subquery runs once per sorted row, so a
                    // choice key reads its values twice rather than once per term.
                    let values = format!(
                        "FROM field_value v LEFT JOIN field_option o ON o.field_id=v.field_id AND o.option_key=v.text_value WHERE v.item_id=i.id AND v.field_id={field}"
                    );
                    let value = "MIN(NULLIF(COALESCE(v.choice_sort,v.text_value),''))";
                    // Valued before empty, then declared options before open
                    // choices. Members after the earliest rank do not break
                    // equal-rank ties.
                    order.push(format!(
                        "(SELECT 2*({value} IS NULL)+(MIN(o.position) IS NULL) {values}) ASC"
                    ));
                    order.push(format!(
                        "(SELECT COALESCE(MIN(o.position),{value}) {values}) {direction}"
                    ));
                    continue;
                }
                Type::Number | Type::Rating | Type::Checkbox => "v.number_value",
                Type::Date | Type::DateTime | Type::Timestamp => "v.time_value",
                _ => "v.text_value",
            };
            order.push(format!(
                "(SELECT MIN({column}) FROM field_value v WHERE v.item_id=i.id AND v.field_id={field}) {direction} NULLS LAST"
            ));
        }
        if !document.sort.iter().any(|sort| sort.field == "id") {
            order.push("i.uuid ASC".into());
        }
        order
    }
}
