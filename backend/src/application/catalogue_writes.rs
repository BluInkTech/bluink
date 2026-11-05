// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{
    Application,
    error::{Result, internal_error, internal_message, invalid_request},
};
use crate::catalogue;
use crate::domain::project::{
    facets, properties,
    types::{ChoiceAppearance, FacetKind, MetadataFieldValueType},
};
use crate::store::Store;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Load one observed declaration, prepare its edit, then commit against that revision.
/// The writer still enforces declaration relationships and cross-record invariants.
pub(super) fn edit_declaration<T: DeserializeOwned + Serialize>(
    store: &Store,
    kind: &str,
    key: &str,
    expected_revision: Option<&str>,
    edit: impl FnOnce(Option<T>) -> Result<T>,
) -> Result<Value> {
    let id = crate::domain::project::named_items::id(kind, key);
    let existing = store.read(|tx| crate::store::item::read(tx, &id, true))?;
    let revision = existing.as_ref().and_then(|item| item["revision"].as_str());
    if expected_revision.is_some_and(|expected| revision != Some(expected)) {
        return Err(invalid_request(
            "This declaration changed. Reload it before saving.",
        ));
    }
    let declaration = existing
        .as_ref()
        .filter(|item| item["meta"]["state"] == "active")
        .map(|item| serde_json::from_value::<T>(item["content"].clone()))
        .transpose()
        .map_err(internal_error)?;
    let mut content = serde_json::to_value(edit(declaration)?).map_err(internal_error)?;
    content
        .as_object_mut()
        .ok_or_else(|| internal_message("Declaration attributes must be an object"))?
        .insert("key".into(), json!(key));
    Ok(crate::store::commands::write(
        store,
        json!({"id":id,"meta":{"type":kind,"state":"active","title":key},"content":content}),
        revision,
        &chrono::Utc::now().format("%Y-%m-%d").to_string(),
    )?)
}

pub(crate) fn create_property(store: &Store, input: &Value) -> Result<Value> {
    let key = input.get("key").and_then(Value::as_str).unwrap_or("");
    let declaration = properties::new_declaration(key, input).map_err(invalid_request)?;
    edit_declaration(
        store,
        "property",
        key,
        None,
        |existing: Option<properties::Declaration>| {
            if existing.is_some() {
                return Err(invalid_request(format!("Property {key} already exists.")));
            }
            Ok(declaration)
        },
    )
}

/// Add choices without changing existing values; an authored order needs a revision.
pub(crate) fn update_property_options(
    store: &Store,
    key: &str,
    requested_type: &str,
    additions: &Value,
    appearance: Option<BTreeMap<String, ChoiceAppearance>>,
    order: Option<Vec<String>>,
    expected_revision: Option<&str>,
) -> Result<Value> {
    let additions =
        properties::option_additions(key, requested_type, additions).map_err(invalid_request)?;
    if order.is_some() && expected_revision.is_none() {
        return Err(invalid_request(
            "Reordering choices requires the property's expected revision.",
        ));
    }
    edit_declaration(store, "property", key, expected_revision, |existing| {
        let declaration =
            existing.ok_or_else(|| invalid_request(format!("Property {key} does not exist.")))?;
        properties::update_options(
            key,
            declaration,
            requested_type,
            additions,
            appearance,
            order,
        )
        .map_err(invalid_request)
    })
}

pub(crate) fn create_facet(store: &Store, kind: &str, key: &str) -> Result<Value> {
    facets::validate_facet_key(kind, key).map_err(invalid_request)?;
    edit_declaration(
        store,
        kind,
        key,
        None,
        |existing: Option<BTreeMap<String, Value>>| {
            if existing.is_some() {
                return Err(invalid_request(format!(
                    "Facet {key}.{kind} already exists."
                )));
            }
            Ok(BTreeMap::new())
        },
    )
}

#[derive(Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct CreatePropertyRequest {
    key: String,
    value_type: MetadataFieldValueType,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    options: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct UpdatePropertyOptionsRequest {
    key: String,
    value_type: MetadataFieldValueType,
    /// Options to add; existing ones are kept.
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    options: Option<Vec<String>>,
    /// When given, replaces how every option looks; options left out are automatic.
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    option_appearance: Option<std::collections::BTreeMap<String, ChoiceAppearance>>,
    /// Complete authored order, including any options added in this request.
    #[cfg_attr(test, ts(optional))]
    option_order: Option<Vec<String>>,
    /// Required when reordering to avoid overwriting another editor's changes.
    #[cfg_attr(test, ts(optional))]
    expected_revision: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct RemovePropertyRequest {
    key: String,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct FacetRequest {
    kind: FacetKind,
    key: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct Removed {
    removed: bool,
}

impl Application {
    pub(crate) fn create_property(
        &self,
        request: CreatePropertyRequest,
    ) -> Result<catalogue::MetadataField> {
        let _operation = self.begin_operation()?;
        let item = create_property(
            self.database()?.as_ref(),
            &serde_json::to_value(request).map_err(internal_error)?,
        )?;
        Self::property_changed(&item)
    }

    pub(crate) fn update_property_options(
        &self,
        request: UpdatePropertyOptionsRequest,
    ) -> Result<catalogue::MetadataField> {
        let _operation = self.begin_operation()?;
        let item = update_property_options(
            self.database()?.as_ref(),
            &request.key,
            request.value_type.as_str(),
            &serde_json::to_value(request.options.unwrap_or_default()).map_err(internal_error)?,
            request.option_appearance,
            request.option_order,
            request.expected_revision.as_deref(),
        )?;
        Self::property_changed(&item)
    }
    /// Describes the declaration this request wrote, at the revision it wrote.
    fn property_changed(item: &Value) -> Result<catalogue::MetadataField> {
        let mut field = catalogue::metadata_field(&item["content"])?;
        field.revision = item["revision"].as_str().map(str::to_owned);
        Ok(field)
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn remove_property(&self, request: RemovePropertyRequest) -> Result<Removed> {
        let _operation = self.begin_operation()?;
        crate::store::commands::archive_named(self.database()?.as_ref(), "property", &request.key)?;
        Ok(Removed { removed: true })
    }

    pub(crate) fn create_facet(&self, request: FacetRequest) -> Result<catalogue::FacetEntry> {
        let _operation = self.begin_operation()?;
        create_facet(
            self.database()?.as_ref(),
            request.kind.as_str(),
            &request.key,
        )?;
        Ok(catalogue::FacetEntry {
            key: request.key,
            kind: request.kind,
            usage_count: 0,
            inferred: false,
        })
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn remove_facet(&self, request: FacetRequest) -> Result<Removed> {
        let _operation = self.begin_operation()?;
        crate::store::commands::archive_named(
            self.database()?.as_ref(),
            request.kind.as_str(),
            &request.key,
        )?;
        Ok(Removed { removed: true })
    }
}
