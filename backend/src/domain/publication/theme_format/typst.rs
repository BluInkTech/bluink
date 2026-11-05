// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Rust owns the PDF shell. Authored themes only supply validated data.

use std::fmt::Write;

use super::{ResolvedTheme, Role, Weight};
use crate::domain::publication::typst::quoted;

/// Native heading typography overrides Typst defaults without a transformation.
/// Geometry is emitted explicitly by the shared Rust heading serializer.
#[expect(
    clippy::unwrap_used,
    reason = "Formatting into an owned String is infallible."
)]
fn heading_styles(theme: &ResolvedTheme) -> Result<String, String> {
    let mut output = String::from("#let publication-heading-styles(body) = {\n");
    // Level six is the fallback for deeper semantic headings. Keep their
    // original levels for navigation and accessibility.
    for (level, role) in [
        (6, Role::Heading6),
        (1, Role::Heading1),
        (2, Role::Heading2),
        (3, Role::Heading3),
        (4, Role::Heading4),
        (5, Role::Heading5),
    ] {
        let style = &theme.styles[&role];
        let selector = if level == 6 {
            "heading".into()
        } else {
            format!("heading.where(level: {level})")
        };
        let font = quoted(crate::domain::publication::fonts::family(
            &style.font_family,
        )?);
        let size = style.font_size_pt;
        let weight = if style.font_weight == Weight::Bold {
            700
        } else {
            400
        };
        let slant = quoted(if style.italic { "italic" } else { "normal" });
        let color = quoted(&style.color);
        writeln!(output, "  show {selector}: set text(font: {font}, size: {size}pt, weight: {weight}, style: {slant}, fill: rgb({color}), top-edge: 0.8em, bottom-edge: -0.2em)").unwrap();
    }
    output.push_str("  body\n}\n");
    Ok(output)
}

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
        style["fontFamily"] = crate::domain::publication::fonts::family(id)?.into();
    }
    let data = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    let literal = serde_json::to_string(&data).map_err(|error| error.to_string())?;
    Ok(format!(
        "#let publication-theme = json(bytes({literal}))\n{}{}",
        heading_styles(theme)?,
        include_str!("typst-runtime.typ")
    ))
}

pub(crate) fn document_shell(title: &str, language: &str) -> String {
    let title = crate::domain::publication::typst::quoted(title);
    let language = crate::domain::publication::typst::quoted(language);
    format!(
        "#import \"theme-values.typ\": *\n#show: publication-document.with(title: {title}, language: {language})\n#include \"content.typ\"\n"
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_document_shell_quotes_the_title_for_typst() {
        let shell = super::document_shell("Pasted\u{b}title \"quoted\"", "en");
        assert!(
            shell.contains(r#"title: "Pasted\u{b}title \"quoted\"""#),
            "{shell}"
        );
    }
}
