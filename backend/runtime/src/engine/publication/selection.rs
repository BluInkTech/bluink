// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use serde_json::Value;

use crate::engine::publication::{
    SelectionEffect, SelectionMatcherKind, SelectionRule, SelectionTarget,
};

/// Return the outermost blocks omitted by ordered Publication rules. The
/// renderer can prune these IDs without repeating rule evaluation.
pub(crate) fn excluded_block_ids(
    document: &Value,
    rules: &[SelectionRule],
) -> Result<Vec<String>, String> {
    if rules
        .iter()
        .all(|rule| matches!(rule.target, SelectionTarget::Page))
    {
        return Ok(Vec::new());
    }
    let mut excluded = Vec::new();
    visit_children(document, true, rules, &mut excluded)?;
    Ok(excluded)
}

#[derive(Clone, Copy)]
struct Section {
    level: u64,
    included: bool,
}

fn visit_children(
    parent: &Value,
    parent_included: bool,
    rules: &[SelectionRule],
    excluded: &mut Vec<String>,
) -> Result<(), String> {
    let Some(children) = parent.get("content").and_then(Value::as_array) else {
        return Ok(());
    };
    let mut sections: Vec<Section> = Vec::new();
    for child in children {
        let kind = child["type"]
            .as_str()
            .ok_or("Document block has no type.")?;
        let mut inherited = sections
            .last()
            .map_or(parent_included, |section| section.included);
        if kind == "heading" {
            let level = child["attrs"]["level"].as_u64().unwrap_or(1);
            while sections
                .last()
                .is_some_and(|section| section.level >= level)
            {
                sections.pop();
            }
            inherited = sections
                .last()
                .map_or(parent_included, |section| section.included);
            let included = inherited && rule_decision(child, SelectionTarget::Section, rules);
            sections.push(Section { level, included });
            inherited = included;
        }
        let included = inherited && rule_decision(child, SelectionTarget::Block, rules);
        if included {
            visit_children(child, true, rules, excluded)?;
        } else {
            let id = child["attrs"]["id"].as_str().ok_or_else(|| {
                format!("Excluded {kind} block has no stable ID; save the page before publishing.")
            })?;
            excluded.push(id.to_owned());
        }
    }
    Ok(())
}

fn rule_decision(node: &Value, target: SelectionTarget, rules: &[SelectionRule]) -> bool {
    let mut included = true;
    for rule in rules {
        if rule.target != target || !matches_rule(node, rule) {
            continue;
        }
        included = matches!(rule.effect, SelectionEffect::Include);
    }
    included
}

fn matches_rule(node: &Value, rule: &SelectionRule) -> bool {
    match rule.matcher.kind {
        SelectionMatcherKind::BlockType => {
            node["type"].as_str() == Some(rule.matcher.value.as_str())
        }
        SelectionMatcherKind::BlockLabel => {
            node["attrs"]["labels"].as_array().is_some_and(|labels| {
                labels
                    .iter()
                    .any(|label| label.as_str() == Some(rule.matcher.value.as_str()))
            })
        }
        SelectionMatcherKind::PageTag => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::excluded_block_ids;
    use crate::engine::publication::validate_publication;

    #[test]
    fn excludes_labelled_blocks_and_heading_sections_without_traversing_omitted_children() {
        let content = json!({
            "kind":"publication","preset":"book","selectedOutput":"pdf", "rootPageId":"01a0e2a0-0000-7000-8000-000000000001",
            "outline":[{"kind":"page","pageId":"01a0e2a0-0000-7000-8000-000000000002","children":[]}],
            "selectionRules":[
                {"id":"01a0e2a0-0000-7000-8000-000000000003","effect":"exclude",
                 "target":"section","matcher":{"kind":"block_label","value":"private"}},
                {"id":"01a0e2a0-0000-7000-8000-000000000004","effect":"exclude",
                 "target":"block","matcher":{"kind":"block_label","value":"draft"}}
            ],
            "themeId":"classic", "targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}
        });
        let rules = validate_publication(&content).unwrap().selection_rules;
        let document = json!({"type":"doc","content":[
            {"type":"heading","attrs":{"id":"a","level":1,"labels":["private"]}},
            {"type":"paragraph","attrs":{"id":"b"}},
            {"type":"heading","attrs":{"id":"c","level":2}},
            {"type":"paragraph","attrs":{"id":"d"}},
            {"type":"heading","attrs":{"id":"e","level":1}},
            {"type":"paragraph","attrs":{"id":"f","labels":["draft"]}},
            {"type":"paragraph","attrs":{"id":"g"}},
            {"type":"blockquote","attrs":{"id":"h"},"content":[
                {"type":"paragraph","attrs":{"id":"i","labels":["draft"]}}
            ]}
        ]});
        assert_eq!(
            excluded_block_ids(&document, &rules).unwrap(),
            ["a", "b", "c", "d", "f", "i"]
        );
    }
}
