// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::application::{
    catalogue_writes::{create_property, edit_declaration, update_property_options},
    error::{ApplicationError, invalid_request},
};
use crate::domain::project::{named_items, properties as property_rules};
use crate::store::Store;
use serde_json::{Value, json};

#[test]
fn declaration_commit_rejects_an_edit_that_became_stale_during_preparation() {
    let root = tempfile::tempdir().unwrap();
    property_rules::initialize(root.path()).unwrap();
    let store = Store::shared(root.path()).unwrap();
    let original = create_property(
        &store,
        &json!({
            "key":"workflow", "valueType":"select", "options":["draft"]
        }),
    )
    .unwrap();
    let result = edit_declaration(
        &store,
        "property",
        "workflow",
        original["revision"].as_str(),
        |observed: Option<property_rules::Declaration>| {
            update_property_options(
                &store,
                "workflow",
                "select",
                &json!(["review"]),
                None,
                None,
                None,
            )?;
            property_rules::update_options(
                "workflow",
                observed.unwrap(),
                "select",
                vec!["ready".into()],
                None,
                None,
            )
            .map_err(invalid_request)
        },
    );
    assert!(matches!(result, Err(ApplicationError::InvalidRequest(_))));
    let stored = store
        .read(|tx| crate::store::item::read(tx, &named_items::id("property", "workflow"), true))
        .unwrap()
        .unwrap();
    assert_eq!(stored["content"]["options"], json!(["draft", "review"]));
}

#[test]
fn declaration_creation_does_not_overwrite_an_item_created_during_preparation() {
    let root = tempfile::tempdir().unwrap();
    property_rules::initialize(root.path()).unwrap();
    let store = Store::shared(root.path()).unwrap();
    let input = json!({"key":"workflow", "valueType":"select", "options":["draft"]});
    let result = edit_declaration(
        &store,
        "property",
        "workflow",
        None,
        |observed: Option<property_rules::Declaration>| {
            assert!(observed.is_none());
            create_property(&store, &input)?;
            property_rules::new_declaration("workflow", &json!({"valueType":"text"}))
                .map_err(invalid_request)
        },
    );
    assert!(matches!(result, Err(ApplicationError::InvalidRequest(_))));
    let stored: Value = store
        .read(|tx| crate::store::item::read(tx, &named_items::id("property", "workflow"), true))
        .unwrap()
        .unwrap();
    assert_eq!(stored["content"]["valueType"], "select");
    assert_eq!(stored["content"]["options"], json!(["draft"]));
}

mod properties {
    use crate::application::catalogue_writes::*;

    use crate::domain::project::properties::{
        initialize, list_property_declarations, with_validated_metadata,
    };
    use crate::domain::project::types::ChoiceAppearance;
    use serde_json::{Map, json};
    use std::collections::BTreeMap;

    #[test]
    fn mutations_preserve_entry_attributes_and_validate_values() {
        let root = tempfile::tempdir().unwrap();
        initialize(root.path()).unwrap();
        let store = crate::store::Store::shared(root.path()).unwrap();
        create_property(
            &store,
            &json!({"key":"workflow","valueType":"select","options":["review","draft"]}),
        )
        .unwrap();
        create_property(&store, &json!({"key":"due_date","valueType":"date"})).unwrap();
        let mut document = crate::store::snapshots::read_named(root.path(), "property", "workflow")
            .unwrap()
            .unwrap();
        document["content"]["color"] = json!("blue");
        document["content"]["display"] = json!({"note":"kept by other editors"});
        crate::store::commands::write(
            &store,
            document.clone(),
            document["revision"].as_str(),
            "2026-10-22",
        )
        .unwrap();
        let updated = update_property_options(
            &store,
            "workflow",
            "select",
            &json!(["ready", "draft"]),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            updated["content"]["options"],
            json!(["review", "draft", "ready"])
        );
        let stored = crate::store::snapshots::read_named(root.path(), "property", "workflow")
            .unwrap()
            .unwrap();
        assert_eq!(stored["content"]["color"], "blue");
        assert_eq!(stored["content"]["display"], document["content"]["display"]);
        assert!(stored["content"].get("metadata").is_none());
        let mut meta = Map::from_iter([
            ("workflow".to_owned(), json!("ready")),
            ("due_date".to_owned(), json!("2026-10-22")),
        ]);
        with_validated_metadata(root.path(), &meta, || Ok(())).unwrap();
        meta.insert("workflow".into(), json!("invalid"));
        assert!(
            with_validated_metadata::<()>(root.path(), &meta, || panic!("Invalid value committed"))
                .is_err()
        );
        meta.insert("workflow".into(), json!("draft"));
        meta.insert("due_date".into(), json!("2026-02-30"));
        assert!(with_validated_metadata(root.path(), &meta, || Ok(())).is_err());
        assert!(create_property(&store, &json!({"key":"workflow","valueType":"text"})).is_err());
        assert!(create_property(&store, &json!({"key":"summary","valueType":"text"})).is_err());
        assert!(
            update_property_options(&store, "workflow", "text", &json!([]), None, None, None)
                .is_err()
        );
        crate::store::commands::archive_named(&store, "property", "workflow").unwrap();
        assert!(crate::store::commands::archive_named(&store, "property", "workflow").is_err());
        drop(store);
        assert_eq!(
            list_property_declarations(root.path())
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn reorder_checks_revision_and_complete_permutation_without_changing_other_items() {
        let root = tempfile::tempdir().unwrap();
        initialize(root.path()).unwrap();
        let store = crate::store::Store::shared(root.path()).unwrap();
        create_property(
            &store,
            &json!({"key":"workflow","valueType":"select","options":["review","draft","final"]}),
        )
        .unwrap();
        create_property(&store, &json!({"key":"other","valueType":"text"})).unwrap();
        let current =
            || crate::store::snapshots::read_named(root.path(), "property", "other").unwrap();
        let unchanged = current();
        let original = crate::store::snapshots::read_named(root.path(), "property", "workflow")
            .unwrap()
            .unwrap();
        let revision = original["revision"].as_str().unwrap();
        for order in [
            vec!["draft", "final"],
            vec!["draft", "draft", "final"],
            vec!["draft", "review", "unknown"],
        ] {
            assert!(
                update_property_options(
                    &store,
                    "workflow",
                    "select",
                    &json!([]),
                    None,
                    Some(order.into_iter().map(str::to_owned).collect()),
                    Some(revision)
                )
                .is_err()
            );
        }
        let order = vec!["final".into(), "review".into(), "draft".into()];
        assert!(
            update_property_options(
                &store,
                "workflow",
                "select",
                &json!([]),
                None,
                Some(order.clone()),
                None
            )
            .is_err()
        );
        let updated = update_property_options(
            &store,
            "workflow",
            "select",
            &json!([]),
            None,
            Some(order.clone()),
            Some(revision),
        )
        .unwrap();
        assert_eq!(updated["content"]["options"], json!(order));
        assert!(matches!(
            update_property_options(
                &store,
                "workflow",
                "select",
                &json!([]),
                None,
                Some(order),
                Some(revision)
            ),
            Err(crate::application::error::ApplicationError::InvalidRequest(
                _
            ))
        ));
        assert_eq!(current(), unchanged);
    }

    #[test]
    fn options_keep_their_appearance_until_it_is_replaced() {
        use crate::domain::project::types::ChoiceColor;
        let root = tempfile::tempdir().unwrap();
        initialize(root.path()).unwrap();
        let store = crate::store::Store::shared(root.path()).unwrap();
        create_property(
            &store,
            &json!({"key":"workflow","valueType":"select","options":["draft","review"]}),
        )
        .unwrap();
        let colours = |pairs: &[(&str, ChoiceColor)]| {
            pairs
                .iter()
                .map(|(option, color)| ((*option).to_owned(), ChoiceAppearance { color: *color }))
                .collect::<BTreeMap<_, _>>()
        };
        // New options and their colours arrive together.
        let updated = update_property_options(
            &store,
            "workflow",
            "select",
            &json!(["final"]),
            Some(colours(&[
                ("final", ChoiceColor::Green),
                ("review", ChoiceColor::Amber),
            ])),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            updated["content"]["options"],
            json!(["draft", "review", "final"])
        );
        assert_eq!(
            updated["content"]["optionAppearance"],
            json!({"final": {"color": "green"}, "review": {"color": "amber"}})
        );
        // Adding options alone leaves colours as they were.
        let kept = update_property_options(
            &store,
            "workflow",
            "select",
            &json!(["ready"]),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            kept["content"]["optionAppearance"],
            updated["content"]["optionAppearance"]
        );
        // A new map replaces the old; an empty one makes every option automatic.
        let replaced = update_property_options(
            &store,
            "workflow",
            "select",
            &json!([]),
            Some(colours(&[("draft", ChoiceColor::Neutral)])),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            replaced["content"]["optionAppearance"],
            json!({"draft": {"color": "neutral"}})
        );
        let cleared = update_property_options(
            &store,
            "workflow",
            "select",
            &json!([]),
            Some(BTreeMap::new()),
            None,
            None,
        )
        .unwrap();
        assert!(cleared["content"].get("optionAppearance").is_none());
        // Only existing options can have a colour, and a refused update changes nothing.
        assert!(
            update_property_options(
                &store,
                "workflow",
                "select",
                &json!([]),
                Some(colours(&[("missing", ChoiceColor::Blue)])),
                None,
                None,
            )
            .is_err()
        );
        let listed = list_property_declarations(root.path()).unwrap();
        let workflow = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["key"] == "workflow")
            .unwrap();
        assert!(workflow.get("optionAppearance").is_none());
    }

    #[test]
    fn required_properties_are_protected_but_default_kind_can_be_removed() {
        let root = tempfile::tempdir().unwrap();
        crate::domain::project::config::open_or_create_project(root.path(), "UTC").unwrap();
        let store = crate::store::Store::shared(root.path()).unwrap();
        assert!(crate::store::commands::archive_named(&store, "property", "status").is_err());
        crate::store::commands::archive_named(&store, "property", "kind").unwrap();
        drop(store);
    }
}

mod facets {
    use crate::application::catalogue_writes::*;
    use crate::domain::project::facets::initialize;

    #[test]
    fn facet_keys_preserve_hierarchy_and_only_active_declarations_can_be_removed() {
        let root = tempfile::tempdir().unwrap();
        initialize(root.path()).unwrap();
        let store = crate::store::Store::shared(root.path()).unwrap();
        create_facet(&store, "tag", "births/1900").unwrap();
        assert!(create_facet(&store, "tag", "births/1900").is_err());
        assert!(create_facet(&store, "label", "bad/path").is_err());
        crate::store::commands::archive_named(&store, "tag", "births/1900").unwrap();
        assert!(crate::store::commands::archive_named(&store, "tag", "births/1900").is_err());
        drop(store);
        root.close().unwrap();
    }
}
