// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::*;
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum FileScope {
    Whole,
    Json {
        keys: Vec<String>,
    },
    Text {
        before: String,
        after: String,
        start: bool,
        end: bool,
    },
}

impl FileScope {
    pub(super) fn validate(&self, path: &str) -> Result<(), String> {
        match self {
            Self::Whole => Ok(()),
            Self::Json { keys }
                if (path == "bluink.json"
                    || path.starts_with("themes/") && path.ends_with("/manifest.json"))
                    && !keys.is_empty() =>
            {
                Ok(())
            }
            Self::Text {
                before,
                after,
                start,
                end,
            } if !path.starts_with("assets/")
                && (*start || !before.is_empty())
                && (*end || !after.is_empty()) =>
            {
                Ok(())
            }
            _ => Err("Invalid scope for package decision".into()),
        }
    }
}

impl FileDecision {
    pub(crate) fn current(&self, source: Option<&str>) -> Result<Fragment, String> {
        match &self.scope {
            FileScope::Whole => {
                Ok(source.map_or(Fragment::Missing, |text| Fragment::Present(json!(text))))
            }
            FileScope::Json { keys } => {
                let value: Value = serde_json::from_str(source.ok_or("JSON file disappeared")?)
                    .map_err(|e| e.to_string())?;
                let (last, parent) = keys.split_last().ok_or("Missing JSON field")?;
                let fields = parent
                    .iter()
                    .try_fold(&value, |value, key| {
                        value.get(key).ok_or("JSON parent disappeared")
                    })?
                    .as_object()
                    .ok_or("JSON parent is no longer an object")?;
                Ok(Fragment::from(fields.get(last)))
            }
            FileScope::Text { .. } => {
                let text = source.ok_or("Theme text disappeared")?;
                let range = self.range(text)?;
                Ok(if range.is_empty() {
                    Fragment::Missing
                } else {
                    Fragment::Present(json!(text[range]))
                })
            }
        }
    }

    /// Recheck the actual fragment, then substitute only that fragment. Adjacent
    /// accepted fields/hunks and author edits outside it survive unchanged.
    pub(crate) fn apply(
        &self,
        source: Option<&str>,
        selected: &Fragment,
    ) -> Result<Option<String>, String> {
        let actual = self.current(source)?;
        if actual == *selected {
            return Ok(source.map(str::to_owned));
        }
        if !self.alternatives.iter().any(|a| a.value == actual) {
            return Err("Package fragment changed independently; refresh the decision".into());
        }
        match &self.scope {
            FileScope::Whole => match selected {
                Fragment::Missing => Ok(None),
                Fragment::Present(value) => {
                    Ok(Some(value.as_str().ok_or("Expected file text")?.into()))
                }
            },
            FileScope::Json { keys } => {
                let mut value: Value = serde_json::from_str(source.ok_or("JSON file disappeared")?)
                    .map_err(|e| e.to_string())?;
                let (last, parent) = keys.split_last().ok_or("Missing JSON field")?;
                let fields = parent
                    .iter()
                    .try_fold(&mut value, |value, key| {
                        value.get_mut(key).ok_or("JSON parent disappeared")
                    })?
                    .as_object_mut()
                    .ok_or("JSON parent is no longer an object")?;
                match selected {
                    Fragment::Missing => {
                        fields.remove(last);
                    }
                    Fragment::Present(field) => {
                        fields.insert(last.clone(), field.clone());
                    }
                }
                Ok(Some(
                    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?,
                ))
            }
            FileScope::Text { .. } => {
                let text = source.ok_or("Theme text disappeared")?;
                let range = self.range(text)?;
                let mut next = text.to_owned();
                let replacement = match selected {
                    Fragment::Missing => "",
                    Fragment::Present(value) => value.as_str().ok_or("Invalid text hunk")?,
                };
                next.replace_range(range, replacement);
                Ok(Some(next))
            }
        }
    }

    fn range(&self, text: &str) -> Result<Range<usize>, String> {
        let FileScope::Text {
            before,
            after,
            start,
            end,
        } = &self.scope
        else {
            return Err("Expected text hunk".into());
        };
        let mut ranges = std::collections::BTreeSet::new();
        for alternative in &self.alternatives {
            let value = match &alternative.value {
                Fragment::Missing => "",
                Fragment::Present(value) => value.as_str().ok_or("Invalid text hunk")?,
            };
            let pattern = format!("{before}{value}{after}");
            for (offset, _) in text.match_indices(&pattern) {
                if offset > 0 && text.as_bytes()[offset - 1] != b'\n' {
                    continue;
                }
                if (*start && offset != 0) || (*end && offset + pattern.len() != text.len()) {
                    continue;
                }
                let begin = offset + before.len();
                ranges.insert((begin, begin + value.len()));
            }
        }
        if ranges.len() != 1 {
            return Err("Theme text hunk is missing or ambiguous; refresh the decision".into());
        }
        let (start, end) = ranges.pop_first().ok_or("Theme text hunk disappeared")?;
        Ok(start..end)
    }
}
