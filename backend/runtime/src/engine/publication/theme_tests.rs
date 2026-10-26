// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use serde_json::json;

fn project(web: Value) -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("themes/custom");
    fs::create_dir_all(package.join("html")).unwrap();
    let mut definition: Value =
        serde_json::from_slice(&builtin_theme("classic").unwrap()["manifest.json"]).unwrap();
    definition["id"] = json!("custom");
    definition["web"] = web;
    fs::write(
        package.join("manifest.json"),
        serde_json::to_vec(&definition).unwrap(),
    )
    .unwrap();
    (root, package)
}

fn context() -> HtmlShellContext<'static> {
    HtmlShellContext {
        title: "<Unsafe title>",
        language: "en",
        body: "<p>Prepared body</p>",
        navigation: "<nav>Navigation</nav>",
        contents: "<nav>Contents</nav>",
        role: "page",
        page: json!({"title":"<Unsafe page>"}),
    }
}

#[test]
fn data_only_project_themes_use_native_generators_without_trust() {
    let (root, _) = project(json!({}));
    for format in [
        PublicationFormat::Html,
        PublicationFormat::Site,
        PublicationFormat::Pdf,
        PublicationFormat::Docx,
    ] {
        let snapshot = capture_theme(root.path(), "custom", format).unwrap();
        assert!(!snapshot.requires_trust);
        if format == PublicationFormat::Pdf {
            let files = load_pdf_theme(root.path(), &snapshot, "Title", "en").unwrap();
            assert_eq!(files.len(), 2);
            assert!(files["document.typ"].starts_with(b"#import"));
        } else if format != PublicationFormat::Docx {
            let kind = if format == PublicationFormat::Html {
                HtmlShellKind::Article
            } else {
                HtmlShellKind::Site
            };
            let html = render_html_shell(root.path(), &snapshot, kind, context(), false).unwrap();
            assert!(html.contains("<p>Prepared body</p>"));
            assert!(html.contains("&lt;Unsafe title&gt;"));
        }
    }
}

#[test]
fn web_shells_require_trust_and_escape_data_while_preserving_generated_markup() {
    let (root, package) = project(json!({"html":"html/article.html"}));
    fs::write(
        package.join("html/article.html"),
        "<style>{{ theme_css }}</style><h1>{{ title }}</h1><h2>{{ page.title }}</h2>{{ body }}",
    )
    .unwrap();
    let snapshot = capture_theme(root.path(), "custom", PublicationFormat::Html).unwrap();
    assert!(snapshot.requires_trust);
    assert!(
        render_html_shell(
            root.path(),
            &snapshot,
            HtmlShellKind::Article,
            context(),
            false
        )
        .unwrap_err()
        .contains("explicit trust")
    );
    let html = render_html_shell(
        root.path(),
        &snapshot,
        HtmlShellKind::Article,
        context(),
        true,
    )
    .unwrap();
    assert!(html.contains("<h1>&lt;Unsafe title&gt;</h1>"));
    assert!(html.contains("<h2>&lt;Unsafe page&gt;</h2>"));
    assert!(html.contains("<p>Prepared body</p>"));
    assert!(
        !capture_theme(root.path(), "custom", PublicationFormat::Pdf)
            .unwrap()
            .requires_trust
    );
}

#[test]
fn portable_data_changes_preserve_web_approval_but_never_plan_freshness() {
    let (root, package) = project(json!({"html":"html/article.html"}));
    fs::write(
        package.join("html/article.html"),
        "{{ theme_css }}{{ body }}",
    )
    .unwrap();
    let original = capture_theme(root.path(), "custom", PublicationFormat::Html).unwrap();
    let manifest = package.join("manifest.json");
    let mut definition: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    definition["name"] = json!("Renamed");
    definition["root"]["fontSize"] = json!("14pt");
    fs::write(&manifest, serde_json::to_vec_pretty(&definition).unwrap()).unwrap();
    let data = capture_theme(root.path(), "custom", PublicationFormat::Html).unwrap();
    assert_ne!(original.sha256, data.sha256);
    assert_eq!(original.execution_sha256, data.execution_sha256);
    assert!(assert_theme_fresh(root.path(), &original, PublicationFormat::Html).is_err());
    crate::export::trusted_theme(&data, Some(&original.execution_sha256)).unwrap();
    assert!(crate::export::trusted_theme(&data, Some(&data.sha256)).is_err());
    definition["web"]["controls"] =
        json!([{"kind":"toggle","id":"sidebar","label":"Sidebar","default":true}]);
    fs::write(&manifest, serde_json::to_vec(&definition).unwrap()).unwrap();
    let web = capture_theme(root.path(), "custom", PublicationFormat::Html).unwrap();
    assert_ne!(web.execution_sha256, data.execution_sha256);
    assert!(crate::export::trusted_theme(&web, Some(&data.execution_sha256)).is_err());
    fs::write(
        package.join("extra.txt"),
        "An include or other package asset",
    )
    .unwrap();
    let asset = capture_theme(root.path(), "custom", PublicationFormat::Html).unwrap();
    assert_ne!(web.execution_sha256, asset.execution_sha256);
    fs::write(
        package.join("html/article.html"),
        "{{ theme_css }}{{ body }}<!-- changed -->",
    )
    .unwrap();
    let code = capture_theme(root.path(), "custom", PublicationFormat::Html).unwrap();
    assert_ne!(code.execution_sha256, asset.execution_sha256);
}

#[test]
fn site_role_shells_also_require_trust_without_a_default_site_shell() {
    let (root, package) = project(json!({"siteRoles":{"page":"html/page.html"}}));
    fs::write(
        package.join("html/page.html"),
        "{{ theme_css }}{{ navigation }}{{ contents }}{{ body }}",
    )
    .unwrap();
    assert!(
        capture_theme(root.path(), "custom", PublicationFormat::Site)
            .unwrap()
            .requires_trust
    );
    assert!(
        !capture_theme(root.path(), "custom", PublicationFormat::Html)
            .unwrap()
            .requires_trust
    );
}

#[test]
fn package_changes_and_tampered_resolved_values_invalidate_snapshots() {
    let (root, package) = project(json!({}));
    let mut snapshot = capture_theme(root.path(), "custom", PublicationFormat::Pdf).unwrap();
    let changes =
        serde_json::from_value(json!({"shared":{"root":{"fontSize":"15pt"}},"targets":{}}))
            .unwrap();
    apply_theme_overrides(&mut snapshot, &changes).unwrap();
    assert_theme_fresh(root.path(), &snapshot, PublicationFormat::Pdf).unwrap();
    assert!(
        assert_theme_fresh(root.path(), &snapshot, PublicationFormat::Html)
            .unwrap_err()
            .contains("format")
    );
    snapshot.resolved.root_font_size_pt = 16.0;
    assert!(
        assert_theme_fresh(root.path(), &snapshot, PublicationFormat::Pdf)
            .unwrap_err()
            .contains("authored")
    );
    let snapshot = capture_theme(root.path(), "custom", PublicationFormat::Pdf).unwrap();
    fs::write(package.join("extra.txt"), "changed").unwrap();
    assert!(
        assert_theme_fresh(root.path(), &snapshot, PublicationFormat::Pdf)
            .unwrap_err()
            .contains("changed")
    );
}

#[test]
fn old_native_hooks_unknown_fields_missing_slots_and_unsafe_paths_are_rejected() {
    for obsolete in ["macros", "settings", "templates"] {
        let (root, package) = project(json!({}));
        let path = package.join("manifest.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value[obsolete] = json!({});
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            capture_theme(root.path(), "custom", PublicationFormat::Html)
                .unwrap_err()
                .contains("unknown field")
        );
    }
    let (root, package) = project(json!({"html":"html/article.html"}));
    fs::write(package.join("html/article.html"), "{{ body }}").unwrap();
    assert!(
        capture_theme(root.path(), "custom", PublicationFormat::Html)
            .unwrap_err()
            .contains("theme_css")
    );
    for path in ["../outside.html", "C:/outside.html", "html\\article.html"] {
        assert!(!safe_theme_path(path));
    }
    let (root, _) = project(json!({"html":"../outside.html"}));
    assert!(capture_theme(root.path(), "custom", PublicationFormat::Html).is_err());
}

#[test]
fn strict_undefined_and_template_fuel_prevent_silent_or_unbounded_rendering() {
    for (source, message) in [
        ("{{ theme_css }}{{ body }}{{ missing_value }}", "undefined"),
        (
            "{{ theme_css }}{{ body }}{% for x in range(10000) %}{% for y in range(100) %}{{ x }}{{ y }}{% endfor %}{% endfor %}",
            "fuel",
        ),
    ] {
        let (root, package) = project(json!({"html":"html/article.html"}));
        fs::write(package.join("html/article.html"), source).unwrap();
        let snapshot = capture_theme(root.path(), "custom", PublicationFormat::Html).unwrap();
        let error = render_html_shell(
            root.path(),
            &snapshot,
            HtmlShellKind::Article,
            context(),
            true,
        )
        .unwrap_err();
        assert!(error.to_lowercase().contains(message), "{error}");
    }
}

#[test]
fn web_control_values_are_checked_against_authored_declarations() {
    let (root, _) = project(json!({"controls":[
        {"kind":"toggle","id":"sidebar","label":"Sidebar","default":true},
        {"kind":"choice","id":"density","label":"Density","choices":["compact","wide"],"default":"wide"}
    ]}));
    let mut snapshot = capture_theme(root.path(), "custom", PublicationFormat::Html).unwrap();
    apply_theme_options(
        &mut snapshot,
        &BTreeMap::from([
            ("sidebar".into(), json!(false)),
            ("density".into(), json!("compact")),
        ]),
    )
    .unwrap();
    for (id, value) in [
        ("sidebar", json!("true")),
        ("density", json!("other")),
        ("unknown", json!(true)),
    ] {
        assert!(apply_theme_options(&mut snapshot, &BTreeMap::from([(id.into(), value)])).is_err());
    }
}

#[test]
fn publication_overrides_accept_and_validate_epub() {
    let root = tempfile::tempdir().unwrap();
    let mut snapshot = capture_theme(root.path(), "classic", PublicationFormat::Html).unwrap();
    let valid = json!({"shared":{},"targets":{"epub":{"root":{"fontSize":"14pt"}}}});
    validate_theme_override(&valid).unwrap();
    // A bad EPUB override must also fail when rendering a different format.
    let invalid: Overrides = serde_json::from_value(
        json!({"shared":{},"targets":{"epub":{"root":{"fontSize":"-1pt"}}}}),
    )
    .unwrap();
    assert!(apply_theme_overrides(&mut snapshot, &invalid).is_err());
}
