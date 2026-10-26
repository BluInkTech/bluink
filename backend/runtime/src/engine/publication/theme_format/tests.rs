// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use serde_json::json;

use super::*;

pub(crate) fn fixture() -> Definition {
    serde_json::from_str(include_str!(
        "../../../../../test-fixtures/publication/portable-theme.json"
    ))
    .unwrap()
}

fn patch(value: serde_json::Value) -> Patch {
    serde_json::from_value(value).unwrap()
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 0.000_001,
        "{actual} != {expected}"
    );
}

#[test]
fn authored_theme_round_trips_without_resolving_references() {
    let theme = fixture();
    let serialized = serde_json::to_value(&theme).unwrap();
    assert_eq!(serialized["root"]["fontSize"], json!({"ref":"root-size"}));
    assert_eq!(
        serde_json::from_value::<Definition>(serialized).unwrap(),
        theme
    );
    assert_eq!(theme.styles.len(), Role::ALL.len());
    for target in ["html", "site", "pdf", "docx", "epub"] {
        let resolved = resolve(&theme, target, &Overrides::default()).unwrap();
        assert_eq!(resolved.styles.len(), Role::ALL.len());
        assert_eq!(resolved.fonts.len(), 3);
        assert_close(resolved.root_font_size_pt, 12.0);
        assert_close(resolved.styles[&Role::Heading1].font_size_pt, 24.0);
    }
}

#[test]
fn units_share_a_physical_scale_and_reject_unsupported_values() {
    for (source, points) in [
        ("1rem", 12.0),
        ("16px", 12.0),
        ("12pt", 12.0),
        ("25.4mm", 72.0),
        ("1in", 72.0),
    ] {
        let length: Length = source.parse().unwrap();
        assert_close(length.points(12.0), points);
        assert_eq!(length.to_string().parse::<Length>().unwrap(), length);
    }
    for invalid in [
        "12",
        "1em",
        "1%",
        "NaNpt",
        "infpt",
        "100001px",
        "12pt ",
        "calc(1rem)",
    ] {
        assert!(invalid.parse::<Length>().is_err(), "accepted {invalid}");
    }
}

#[test]
fn cascade_resolves_references_after_all_overrides_and_preserves_source() {
    let mut theme = fixture();
    let before = theme.clone();
    theme.targets.insert(
        "html".into(),
        patch(json!({"tokens":{"root-size":{"type":"length","value":"14pt"}}})),
    );
    theme.targets.insert(
        "site".into(),
        patch(json!({"styles":{"body":{"italic":true}}})),
    );
    let overrides: Overrides = serde_json::from_value(json!({
        "shared":{"tokens":{"root-size":{"type":"length","value":"15pt"}},"styles":{"body":{"alignment":"justify"}}},
        "targets":{
            "html":{"tokens":{"root-size":{"type":"length","value":"16pt"}}},
            "site":{"tokens":{"root-size":{"type":"length","value":"18pt"}},"styles":{"heading1":{"fontSize":"3rem"}}}
        }
    })).unwrap();
    let authored = theme.clone();
    let site = resolve(&theme, "site", &overrides).unwrap();
    assert_close(site.root_font_size_pt, 18.0);
    assert_close(site.styles[&Role::Heading1].font_size_pt, 54.0);
    assert!(site.styles[&Role::Body].italic);
    assert_eq!(site.styles[&Role::Body].alignment, Alignment::Justify);
    assert_close(
        resolve(&theme, "html", &overrides)
            .unwrap()
            .root_font_size_pt,
        16.0,
    );
    assert_close(
        resolve(&theme, "docx", &overrides)
            .unwrap()
            .root_font_size_pt,
        15.0,
    );
    assert_eq!(theme, authored);
    assert_ne!(theme, before);
}

#[test]
fn website_inherits_html_before_its_own_patch() {
    let mut theme = fixture();
    theme.targets.insert(
        "html".into(),
        patch(json!({"styles":{"heading2":{"italic":true,"spaceBefore":"3rem"}}})),
    );
    theme.targets.insert(
        "site".into(),
        patch(json!({"styles":{"heading2":{"spaceBefore":"4rem"}}})),
    );
    let site = resolve(&theme, "site", &Overrides::default()).unwrap();
    assert!(site.styles[&Role::Heading2].italic);
    assert_close(site.styles[&Role::Heading2].space_before_pt, 48.0);
    assert!(
        !resolve(&theme, "pdf", &Overrides::default())
            .unwrap()
            .styles[&Role::Heading2]
            .italic
    );
}

#[test]
fn deep_page_patches_preserve_other_slots_and_arrays_replace() {
    let overrides: Overrides = serde_json::from_value(json!({
        "shared":{"page":{"header":{"left":[],"showOnFirstPage":true},"orientation":"landscape"}},"targets":{}
    })).unwrap();
    let resolved = resolve(&fixture(), "docx", &overrides).unwrap();
    assert_eq!(resolved.page.header.left, []);
    assert_eq!(resolved.page.header.right, vec![RunningPart::ChapterTitle]);
    assert!(resolved.page.header.show_on_first_page);
    assert_close(resolved.page.width_pt, 297.0 * 72.0 / 25.4);
    assert_close(resolved.page.height_pt, 210.0 * 72.0 / 25.4);
}

#[test]
fn style_variants_inherit_their_container_and_line_height_is_a_multiplier() {
    let overrides: Overrides = serde_json::from_value(json!({
        "shared":{"styles":{"box":{"padding":"2rem"},"boxWarning":{"borderStyle":"dashed"},"body":{"fontSize":"2rem","lineHeight":1.8}}},"targets":{}
    })).unwrap();
    let result = resolve(&fixture(), "pdf", &overrides).unwrap();
    assert_close(result.styles[&Role::BoxWarning].padding_pt, 24.0);
    assert_eq!(
        result.styles[&Role::BoxWarning].border_style,
        Stroke::Dashed
    );
    assert_close(result.styles[&Role::Body].font_size_pt, 24.0);
    assert_close(result.styles[&Role::Body].line_height, 1.8);
    assert_close(result.styles[&Role::Heading1].font_size_pt, 24.0);
    assert_close(result.styles[&Role::Heading1].line_height, 1.2);
}

#[test]
fn invalid_references_are_reported_even_for_unused_tokens() {
    for (token, message) in [
        (
            json!({"type":"color","value":{"ref":"missing"}}),
            "reference does not exist",
        ),
        (
            json!({"type":"color","value":{"ref":"body-font"}}),
            "expected Color",
        ),
        (json!({"type":"color","value":{"ref":"broken"}}), "cyclic"),
        (json!({"type":"color","value":"red"}), "six-digit"),
        (json!({"type":"font","value":"unavailable-font"}), "font"),
    ] {
        let mut theme = fixture();
        theme
            .tokens
            .insert("broken".into(), serde_json::from_value(token).unwrap());
        let error = resolve(&theme, "docx", &Overrides::default()).unwrap_err();
        assert!(error.contains(message), "{error}");
    }
    let mut theme = fixture();
    theme.tokens.insert(
        "first".into(),
        Token::Color(Value::Reference(Reference {
            name: "second".into(),
        })),
    );
    theme.tokens.insert(
        "second".into(),
        Token::Color(Value::Reference(Reference {
            name: "first".into(),
        })),
    );
    assert!(
        resolve(&theme, "html", &Overrides::default())
            .unwrap_err()
            .contains("cyclic")
    );
}

#[test]
fn invalid_root_style_geometry_and_contrast_are_rejected_with_paths() {
    for (value, path) in [
        (json!({"root":{"fontSize":"1rem"}}), "root.fontSize"),
        (json!({"styles":{"body":{"fontSize":"0pt"}}}), "styles.body"),
        (json!({"styles":{"body":{"lineHeight":5}}}), "lineHeight"),
        (json!({"styles":{"quote":{"padding":"-1pt"}}}), "padding"),
        (json!({"page":{"width":"1rem"}}), "page"),
        (json!({"page":{"marginLeft":"210mm"}}), "margins"),
        (json!({"page":{"headerDistance":"26mm"}}), "distances"),
        (json!({"page":{"measureCharacters":5}}), "measureCharacters"),
        (json!({"colors":{"ink":"#eeeeee"}}), "colors.ink"),
    ] {
        let overrides = Overrides {
            shared: patch(value),
            ..Overrides::default()
        };
        let error = resolve(&fixture(), "docx", &overrides).unwrap_err();
        assert!(error.contains(path), "{error}");
    }
}

#[test]
fn unknown_fields_and_invalid_metadata_are_rejected() {
    for value in [
        json!({"unexpected":1}),
        json!({"page":{"margnTop":"1in"}}),
        json!({"colors":{"unknown":"#ffffff"}}),
        json!({"styles":{"unknown":{}}}),
        json!({"styles":{"body":{"fontSiz":"1pt"}}}),
    ] {
        assert!(serde_json::from_value::<Patch>(value).is_err());
    }
    for (field, value) in [
        ("schemaVersion", json!(2)),
        ("id", json!("Bad ID")),
        ("version", json!("1.0")),
        ("name", json!(" ")),
    ] {
        let mut theme = serde_json::to_value(fixture()).unwrap();
        theme[field] = value;
        let theme = serde_json::from_value(theme).unwrap();
        assert!(resolve(&theme, "pdf", &Overrides::default()).is_err());
    }
    assert!(resolve(&fixture(), "odt", &Overrides::default()).is_err());
}

#[test]
fn web_shell_paths_and_controls_are_data_checked_before_loading_files() {
    let mut theme = fixture();
    theme.web = serde_json::from_value(json!({
        "html":"html/article.html.jinja", "site":"site/page.html.jinja", "siteRoles":{"landing":"site/landing.html.jinja"},
        "controls":[{"kind":"toggle","id":"sidebar","label":"Show sidebar","default":true},
        {"kind":"choice","id":"navigation","label":"Navigation","default":"tree","choices":["tree","flat"]}]
    })).unwrap();
    resolve(&theme, "html", &Overrides::default()).unwrap();
    for path in [
        "../shell.jinja",
        "/shell.jinja",
        "C:/shell.jinja",
        "html\\shell.jinja",
        "html/./shell.jinja",
    ] {
        theme.web.html = Some(path.into());
        assert!(
            resolve(&theme, "html", &Overrides::default())
                .unwrap_err()
                .contains("web:")
        );
    }
    theme.web.html = None;
    theme.web.controls.push(theme.web.controls[0].clone());
    assert!(
        resolve(&theme, "html", &Overrides::default())
            .unwrap_err()
            .contains("unique ID")
    );
    theme.web.controls = serde_json::from_value(json!([{"kind":"choice","id":"bad","label":"Bad","default":"missing","choices":["present"]}])).unwrap();
    assert!(
        resolve(&theme, "html", &Overrides::default())
            .unwrap_err()
            .contains("contain the default")
    );
}
