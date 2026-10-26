// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Rust owns the PDF shell. Authored themes only supply validated data.

use super::ResolvedTheme;

pub(crate) fn values(theme: &ResolvedTheme) -> Result<String, String> {
    let mut value = serde_json::to_value(theme).map_err(|error| error.to_string())?;
    for style in value["styles"]
        .as_object_mut()
        .ok_or("Resolved theme has no styles")?
        .values_mut()
    {
        let id = style["fontFamily"]
            .as_str()
            .ok_or("Resolved style has no font")?;
        style["fontFamily"] = crate::engine::publication::fonts::family(id)?.into();
    }
    let data = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    let literal = serde_json::to_string(&data).map_err(|error| error.to_string())?;
    Ok(format!(
        "#let publication-theme = json(bytes({literal}))\n{}",
        include_str!("typst-runtime.typ")
    ))
}

pub(crate) fn document_shell(title: &str, language: &str) -> Result<String, String> {
    let title = serde_json::to_string(title).map_err(|error| error.to_string())?;
    let language = serde_json::to_string(language).map_err(|error| error.to_string())?;
    Ok(format!(
        "#import \"theme-values.typ\": *\n#show: publication-document.with(title: {title}, language: {language})\n#include \"content.typ\"\n"
    ))
}
