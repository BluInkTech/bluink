// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{Compiler, diagnostic};
use crate::domain::project::query::{
    Comparison, Polarity, Predicate, QueryDocument, StatePredicate, TextMode,
};
use crate::domain::project::types::MetadataFieldValueType as Type;
use crate::store::{Error, Result, normalize};

impl Compiler {
    pub(super) fn conditions(&mut self, document: &QueryDocument) -> Result<String> {
        let mut conditions = vec!["i.type <> 'conflict'".to_owned()];
        let (mut fixed_type, mut fixed_state) = (false, false);
        for (position, condition) in document.clauses.iter().enumerate() {
            let required = condition.polarity == Polarity::Required;
            let archived = matches!(
                condition.predicate,
                Predicate::Is {
                    value: StatePredicate::Archived
                }
            );
            match &condition.predicate {
                Predicate::Field {
                    field,
                    operator: Comparison::Match,
                    ..
                } if required && matches!(field.as_str(), "type" | "state") => {
                    if field == "type" {
                        fixed_type = true;
                    } else {
                        fixed_state = true;
                    }
                }
                Predicate::Field { field, .. } | Predicate::Has { field }
                    if matches!(field.as_str(), "type" | "state" | "id") => {}
                _ if archived => fixed_state = true,
                _ => self.fixed_scope = false,
            }
            let expression = self
                .predicate(&condition.predicate, required)
                .map_err(|error| match error {
                    Error::Invalid(message) => diagnostic(position, message),
                    other => other,
                })?;
            conditions.push(if required {
                expression
            } else if archived {
                // State has only two values; the equality can seek the
                // (type, state, ...) ordered indexes where a negation cannot.
                "i.state='active'".into()
            } else {
                format!("NOT ({expression})")
            });
        }
        self.fixed_scope &= fixed_type && fixed_state;
        if !self.positive_text.is_empty() {
            let intersection = self
                .positive_text
                .iter()
                .map(|name| format!("SELECT item_id FROM {name}"))
                .collect::<Vec<_>>()
                .join(" INTERSECT ");
            // Sorted posting lists can be intersected before touching item headers.
            let merge_order = if self.positive_text.len() > 1 {
                " ORDER BY 1"
            } else {
                ""
            };
            conditions.push(format!("i.id IN ({intersection}{merge_order})"));
        }
        Ok(conditions.join(" AND "))
    }

    fn predicate(&mut self, predicate: &Predicate, required: bool) -> Result<String> {
        Ok(match predicate {
            Predicate::Text { value, mode } => self.text(value, *mode, None, required),
            Predicate::Tag { key } => self.relation("tag", Some(key), true),
            Predicate::Label { key } => self.relation("label", Some(key), true),
            Predicate::Is {
                value: StatePredicate::Archived,
            } => "i.state='archived'".into(),
            Predicate::Is {
                value: StatePredicate::Pinned,
            } => {
                let scope = self.pin_scope.clone();
                self.relation("pin", scope.as_deref(), false)
            }
            Predicate::Has { field } => self.presence(field)?,
            Predicate::Field {
                field,
                operator,
                value,
            } => self.field(field, *operator, value, required)?,
        })
    }

    fn presence(&mut self, field: &str) -> Result<String> {
        match field {
            "type" | "state" | "id" | "created" | "modified" => Ok("1".into()),
            "tag" | "label" => Ok(self.relation(field, None, true)),
            "link" | "reference" => {
                let links = self.relation("reference", None, false);
                Ok(format!("({links} OR {})", self.property_links(None)))
            }
            "completed" => Ok("EXISTS(SELECT 1 FROM task_projection t WHERE t.item_id=i.id AND t.completed_ms IS NOT NULL)".into()),
            // A page has open tasks when any is open, and progress when it has any task.
            "open_tasks" => Ok(format!("{}>0", task_fact(field).unwrap_or("0"))),
            "task_progress" => Ok(format!("{} IS NOT NULL", task_fact(field).unwrap_or("NULL"))),
            "title" | "body" => Err(Error::Invalid(format!(
                "Presence checks are not supported for `{field}`."
            ))),
            _ => {
                let Some(definition) = self.fields.get(field) else {
                    return Ok("0".into());
                };
                let id = self.bind(definition.id);
                Ok(format!(
                    "i.id IN (SELECT p.item_id FROM field_presence p WHERE p.field_id={id} AND p.value_state='value')"
                ))
            }
        }
    }

    fn field(
        &mut self,
        field: &str,
        operator: Comparison,
        value: &str,
        required: bool,
    ) -> Result<String> {
        let compare = comparison(operator);
        match field {
            "type" | "state" | "id" => {
                let column = if field == "id" { "uuid" } else { field };
                let value = self.bind(value.to_owned());
                Ok(format!("i.{column}{compare}{value}"))
            }
            "tag" | "label" if operator == Comparison::Match => {
                Ok(self.relation(field, Some(value), true))
            }
            "link" | "reference" if operator == Comparison::Match => {
                let links = self.relation("reference", Some(value), false);
                Ok(format!("({links} OR {})", self.property_links(Some(value))))
            }
            "tag" | "label" | "link" | "reference" => {
                let named = matches!(field, "tag" | "label");
                let kind = self.bind(if named { field } else { "reference" }.to_owned());
                let value = self.bind(value.to_owned());
                let column = if named { "named_key" } else { "uuid" };
                let namespace = if named {
                    format!(" AND e.named_kind={kind}")
                } else {
                    String::new()
                };
                Ok(format!(
                    "i.id IN (SELECT r.source_id FROM relation r JOIN entity e ON e.id=r.target_id WHERE r.kind={kind}{namespace} AND e.{column}{compare}{value})"
                ))
            }
            "title" | "body" if operator == Comparison::Match => {
                Ok(self.text(value, TextMode::Phrase, Some(field), required))
            }
            "title" => {
                let value = self.bind(normalize::sort_key(value));
                Ok(format!("i.title_sort{compare}{value}"))
            }
            "body" => {
                let tokens = normalize::tokens(value);
                let token = if value == "*" {
                    value
                } else if let [token] = tokens.as_slice() {
                    token.text.as_str()
                } else {
                    return Err(Error::Invalid(
                        "A body range boundary must contain exactly one searchable word.".into(),
                    ));
                };
                let term = self.bind(normalize::fts_token(token));
                Ok(format!(
                    "i.id IN (SELECT doc/2 FROM item_fts_terms WHERE col='body' AND term>='t' AND term<'u' AND term{compare}{term})"
                ))
            }
            "created" | "modified" => {
                self.date(&format!("i.{field}_ms"), Type::Timestamp, operator, value)
            }
            "completed" => {
                let expression =
                    self.date("task.completed_ms", Type::Timestamp, operator, value)?;
                Ok(format!(
                    "EXISTS(SELECT 1 FROM task_projection task WHERE task.item_id=i.id AND {expression})"
                ))
            }
            "open_tasks" | "task_progress" => {
                let value = value
                    .parse::<f64>()
                    .ok()
                    .filter(|number| number.is_finite())
                    .ok_or_else(|| Error::Invalid("Enter a finite number.".into()))?;
                let value = self.bind(value);
                Ok(format!(
                    "{}{compare}{value}",
                    task_fact(field).unwrap_or("NULL")
                ))
            }
            _ => self.metadata(field, operator, value),
        }
    }

    fn metadata(&mut self, field: &str, operator: Comparison, value: &str) -> Result<String> {
        let Some(definition) = self.fields.get(field).cloned() else {
            return Ok("0".into());
        };
        let field_id = self.bind(definition.id);
        let compare = comparison(operator);
        let predicate = match definition.kind {
            Type::Number | Type::Rating => {
                let value = value
                    .parse::<f64>()
                    .ok()
                    .filter(|number| number.is_finite())
                    .ok_or_else(|| Error::Invalid("Enter a finite number.".into()))?;
                let value = self.bind(value);
                format!("v.number_value{compare}{value}")
            }
            Type::Checkbox => {
                let value = self.bind(i64::from(value == "true"));
                format!("v.number_value{compare}{value}")
            }
            kind @ (Type::Date | Type::DateTime | Type::Timestamp) => {
                self.date("v.time_value", kind, operator, value)?
            }
            // A linked page matches by its exact identity.
            Type::Page => {
                let value = self.bind(value.to_owned());
                format!("v.text_value{compare}{value}")
            }
            _ if operator == Comparison::Match => return Ok(self.metadata_phrase(&field_id, value)),
            _ => {
                let value = self.bind(value.to_owned());
                format!("v.text_value{compare}{value}")
            }
        };
        Ok(format!(
            "i.id IN (SELECT v.item_id FROM field_value v WHERE v.field_id={field_id} AND {predicate})"
        ))
    }

    /// Items whose page-valued properties point at `target`, or at any page.
    fn property_links(&mut self, target: Option<&str>) -> String {
        let target = target.map_or_else(String::new, |target| {
            format!(" AND v.text_value={}", self.bind(target.to_owned()))
        });
        format!(
            "i.id IN (SELECT v.item_id FROM field_value v JOIN field_definition f ON f.id=v.field_id WHERE f.value_type='page'{target})"
        )
    }

    fn metadata_phrase(&mut self, field_id: &str, value: &str) -> String {
        let tokens = normalize::tokens(value);
        let Some(first) = tokens.first() else {
            return format!("({field_id} IS NULL AND 0)");
        };
        let first_position = first.position;
        let first_term = self.bind(first.text.clone());
        let mut predicates = vec![format!("t0.field_id={field_id} AND t0.term={first_term}")];
        for token in tokens.iter().skip(1) {
            let term = self.bind(token.text.clone());
            let offset = token.position - first_position;
            predicates.push(format!("EXISTS(SELECT 1 FROM field_token tn WHERE tn.item_id=t0.item_id AND tn.field_id=t0.field_id AND tn.position=t0.position+{offset} AND tn.term={term})"));
        }
        format!(
            "i.id IN (SELECT t0.item_id FROM field_token t0 WHERE {})",
            predicates.join(" AND ")
        )
    }

    fn text(
        &mut self,
        value: &str,
        mode: TextMode,
        column: Option<&str>,
        required: bool,
    ) -> String {
        let (value, boost) = if mode == TextMode::Terms && column.is_none() {
            value
                .rsplit_once('^')
                .and_then(|(term, weight)| {
                    weight
                        .parse::<f64>()
                        .ok()
                        .filter(|weight| !term.is_empty() && weight.is_finite() && *weight >= 0.0)
                        .map(|weight| (term, weight))
                })
                .unwrap_or((value, 1.0))
        } else {
            (value, 1.0)
        };
        if mode == TextMode::Terms && value == "*" {
            return "1".into();
        }
        let tokens = normalize::tokens(value);
        if tokens.is_empty() {
            return "0".into();
        }
        let terms = tokens
            .iter()
            .map(|token| normalize::fts_token(&token.text))
            .collect::<Vec<_>>();
        if mode == TextMode::Terms {
            return terms
                .iter()
                .map(|term| self.fts_match(term, column, required, boost))
                .collect::<Vec<_>>()
                .join(" AND ");
        }
        let gaps = mode == TextMode::Phrase
            && tokens
                .windows(2)
                .any(|pair| pair[1].position != pair[0].position + 1);
        let text = if mode == TextMode::Phrase && !gaps {
            format!("\"{}\"", terms.join(" "))
        } else {
            terms.join(" AND ")
        };
        let membership = self.fts_match(&text, column, required, boost);
        if gaps {
            let matched = format!("fts{}", self.ctes.len() - 1);
            format!(
                "({membership} AND {})",
                self.phrase_offsets(&tokens, column, &matched)
            )
        } else {
            membership
        }
    }

    fn fts_match(
        &mut self,
        text: &str,
        column: Option<&str>,
        required: bool,
        boost: f64,
    ) -> String {
        let text = column.map_or_else(|| text.to_owned(), |column| format!("{column}:({text})"));
        let operand = self.bind(text);
        let name = format!("fts{}", self.ctes.len());
        if self.rank_text && required {
            let weight = self.bind(boost);
            self.ctes.push(format!("{name} AS MATERIALIZED (SELECT rowid,rowid/2 AS item_id,bm25(item_fts)*{weight} AS score FROM item_fts WHERE item_fts MATCH {operand})"));
        } else {
            self.ctes.push(format!("{name} AS (SELECT rowid,rowid/2 AS item_id FROM item_fts WHERE item_fts MATCH {operand})"));
        }
        if required {
            self.positive_text.push(name);
            "1".into()
        } else {
            format!("i.id IN (SELECT item_id FROM {name})")
        }
    }

    fn phrase_offsets(
        &mut self,
        tokens: &[normalize::Token],
        column: Option<&str>,
        matched: &str,
    ) -> String {
        let Some(anchor) = tokens.iter().max_by_key(|token| token.text.len()) else {
            return "0".into();
        };
        let anchor_table = self.positions(&anchor.text, column, matched);
        let mut conditions = vec!["anchor.doc IN (i.id*2,i.id*2+1)".to_owned()];
        for token in tokens
            .iter()
            .filter(|token| token.position != anchor.position)
        {
            let positions = self.positions(&token.text, column, matched);
            let direction = if token.position >= anchor.position {
                "+"
            } else {
                "-"
            };
            let distance = token.position.abs_diff(anchor.position);
            conditions.push(format!("EXISTS(SELECT 1 FROM {positions} other WHERE other.doc=anchor.doc AND other.col=anchor.col AND other.offset=anchor.offset{direction}{distance})"));
        }
        format!(
            "EXISTS(SELECT 1 FROM {anchor_table} anchor WHERE {})",
            conditions.join(" AND ")
        )
    }

    fn positions(&mut self, token: &str, column: Option<&str>, matched: &str) -> String {
        let term = self.bind(normalize::fts_token(token));
        let column = column.map_or_else(String::new, |column| {
            let column = self.bind(column.to_owned());
            format!(" AND col={column}")
        });
        let name = format!("positions{}", self.ctes.len());
        // The vocabulary supports term lookup, not owner lookup. Materialize
        // each requested term once, bounded to MATCH candidates, so correlated
        // phrase checks do not repeatedly walk its complete postings list.
        self.ctes.push(format!("{name} AS MATERIALIZED (SELECT doc,col,offset FROM item_fts_terms WHERE term={term} AND doc IN (SELECT rowid FROM {matched}){column})"));
        name
    }
}

/// What a page derives from its active tasks, correlated to the item `i`.
/// `task_progress` is the whole percentage done, or null without tasks.
pub(super) fn task_fact(field: &str) -> Option<&'static str> {
    Some(match field {
        "open_tasks" => {
            "(SELECT count(*) FROM task_projection t JOIN item ti ON ti.id=t.item_id WHERE t.note_id=i.id AND ti.state='active' AND t.completed_ms IS NULL)"
        }
        "task_progress" => {
            "(SELECT CASE WHEN count(*)=0 THEN NULL ELSE 100*count(t.completed_ms)/count(*) END FROM task_projection t JOIN item ti ON ti.id=t.item_id WHERE t.note_id=i.id AND ti.state='active')"
        }
        _ => return None,
    })
}

pub(super) const fn comparison(operator: Comparison) -> &'static str {
    match operator {
        Comparison::Match => "=",
        Comparison::Gt => ">",
        Comparison::Gte => ">=",
        Comparison::Lt => "<",
        Comparison::Lte => "<=",
    }
}
