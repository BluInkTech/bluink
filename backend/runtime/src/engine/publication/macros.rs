// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use minijinja::value::Value;
use minijinja::{AutoEscape, Environment, UndefinedBehavior};

/// Macros receive finished child output. Rust remains responsible for walking
/// the document and resolving links, assets, and labelled snippets.
pub(crate) struct RenderMacros {
    environment: Environment<'static>,
    format: &'static str,
    theme: Option<String>,
}

impl RenderMacros {
    pub(crate) fn new(format: &'static str, overrides: Option<&str>) -> Result<Self, String> {
        let defaults = match format {
            "html" => include_str!("../../../../vendor/theme-macros/html.html"),
            "pdf" => include_str!("../../../../vendor/theme-macros/pdf.j2"),
            _ => return Err(format!("Unsupported macro format: {format}.")),
        };
        // Later definitions replace earlier definitions in this one module,
        // including references from macros declared before an override.
        let source = format!("{defaults}\n{}", overrides.unwrap_or_default());
        let mut environment = Environment::new();
        environment.set_auto_escape_callback(move |_| {
            if format == "html" {
                AutoEscape::Html
            } else {
                AutoEscape::None
            }
        });
        environment.set_undefined_behavior(UndefinedBehavior::Strict);
        environment.set_fuel(Some(50_000));
        environment
            .add_template_owned("macros".to_owned(), source)
            .map_err(|error| format!("Invalid {format} macro template: {error}"))?;
        let registry = Self {
            environment,
            format,
            theme: None,
        };
        registry.capture()?;
        Ok(registry)
    }

    pub(crate) fn for_theme(mut self, theme: &str) -> Self {
        self.theme = Some(theme.to_owned());
        self
    }

    fn capture(&self) -> Result<minijinja::Captured<'_>, String> {
        self.environment
            .get_template("macros")
            .map_err(|error| format!("Cannot load {} macros: {error}", self.format))?
            .render_captured(())
            .map_err(|error| format!("Cannot initialize {} macros: {error}", self.format))
    }

    pub(crate) fn render(
        &self,
        name: &str,
        context: Value,
        children: &str,
    ) -> Result<String, String> {
        let captured = self.capture()?;
        let children = if self.format == "html" {
            Value::from_safe_string(children.to_owned())
        } else {
            Value::from(children)
        };
        captured
            .state()
            .call_macro(name, &[context, children])
            .map_err(|error| {
                let theme = self
                    .theme
                    .as_deref()
                    .map(|id| format!("Theme {id} "))
                    .unwrap_or_default();
                format!("{theme}{} macro {name} failed: {error}", self.format)
            })
    }
}

#[cfg(test)]
mod tests {
    use minijinja::value::Value;

    use super::RenderMacros;

    #[test]
    fn later_macro_replaces_only_its_matching_default() {
        let registry = RenderMacros::new(
            "html",
            Some("{% macro heading(ctx, children) %}<h2>{{ children }}</h2>{% endmacro %}"),
        )
        .unwrap();
        assert_eq!(
            registry
                .render("heading", Value::from(()), "Safe &amp; sound")
                .unwrap(),
            "<h2>Safe &amp; sound</h2>"
        );
        assert_eq!(
            registry.render("strong", Value::from(()), "word").unwrap(),
            "<strong>word</strong>"
        );
    }

    #[test]
    fn same_module_macro_calls_see_later_overrides() {
        let registry = RenderMacros::new(
            "html",
            Some(
                r"
{% macro inner(ctx, children) %}old{% endmacro %}
{% macro outer(ctx, children) %}{{ inner(ctx, children) }}{% endmacro %}
{% macro inner(ctx, children) %}new{% endmacro %}
",
            ),
        )
        .unwrap();
        assert_eq!(
            registry.render("inner", Value::from(()), "").unwrap(),
            "new"
        );
        assert_eq!(
            registry.render("outer", Value::from(()), "").unwrap(),
            "new"
        );
    }

    #[test]
    fn wrapper_override_preserves_default_children_and_other_marks() {
        let registry = RenderMacros::new(
            "html",
            Some("{% macro start_superscript(ctx) %}<sup class=\"company-script\">{% endmacro %}"),
        )
        .unwrap();
        assert_eq!(
            registry
                .render("superscript", Value::from(()), "<em>2 &amp; 3</em>")
                .unwrap(),
            "<sup class=\"company-script\"><em>2 &amp; 3</em></sup>"
        );
        assert_eq!(
            registry.render("subscript", Value::from(()), "2").unwrap(),
            "<sub>2</sub>"
        );
        let pdf = RenderMacros::new("pdf", Some(
            "{% macro start_subscript(ctx) %}#sub[#text(fill: accent)[{% endmacro %}{% macro end_subscript(ctx) %}]]{% endmacro %}",
        )).unwrap();
        assert_eq!(
            pdf.render("subscript", Value::from(()), "#text(\"2\")")
                .unwrap(),
            "#sub[#text(fill: accent)[#text(\"2\")]]"
        );
    }
}
