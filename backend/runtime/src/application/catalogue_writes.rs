// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::engine::project::{
    facets, properties,
    types::{ChoiceAppearance, FacetKind, MetadataFieldValueType},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Application;
use super::error::{Result, internal_error};
use crate::catalogue;
use crate::indexing::service;

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
        let declaration = properties::create_property(
            &self.root,
            serde_json::to_value(request).map_err(internal_error)?,
        )?;
        self.property_changed(declaration)
    }

    pub(crate) fn update_property_options(
        &self,
        request: UpdatePropertyOptionsRequest,
    ) -> Result<catalogue::MetadataField> {
        let _operation = self.begin_operation()?;
        let declaration = properties::update_property_options(
            &self.root,
            &request.key,
            request.value_type.as_str(),
            serde_json::to_value(request.options.unwrap_or_default()).map_err(internal_error)?,
            request.option_appearance,
        )?;
        self.property_changed(declaration)
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    fn property_changed(&self, declaration: Value) -> Result<catalogue::MetadataField> {
        self.publish_declaration(
            "property",
            declaration["key"]
                .as_str()
                .ok_or_else(|| internal_error("Property has no key"))?,
        )?;
        self.refresh_schema()?;
        catalogue::metadata_field(&declaration)
    }

    fn refresh_schema(&self) -> Result<()> {
        service::refresh_project_query_fields(&self.root)
    }

    fn publish_declaration(&self, kind: &str, key: &str) -> Result<()> {
        let item = crate::engine::project::named_items::read(&self.root, kind, key)
            .map_err(internal_error)?
            .ok_or_else(|| internal_error("Written declaration is missing"))?;
        service::mark_project_write(
            &self.root,
            item["id"]
                .as_str()
                .ok_or_else(|| internal_error("Declaration has no ID"))?,
            item["revision"]
                .as_str()
                .ok_or_else(|| internal_error("Declaration has no revision"))?,
        )
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn remove_property(&self, request: RemovePropertyRequest) -> Result<Removed> {
        let _operation = self.begin_operation()?;
        let check = crate::catalogue_usage::UsageCheck::new(
            &self.root,
            crate::engine::project::usage::Usage::Property(&request.key),
        );
        properties::remove_property(&self.root, &request.key, || {
            Self::declaration_usage_result(check.used())
        })?;
        self.publish_declaration("property", &request.key)?;
        self.refresh_schema()?;
        Ok(Removed { removed: true })
    }

    pub(crate) fn create_facet(&self, request: FacetRequest) -> Result<catalogue::FacetEntry> {
        let _operation = self.begin_operation()?;
        facets::create_facet(&self.root, request.kind.as_str(), &request.key)?;
        self.publish_declaration(request.kind.as_str(), &request.key)?;
        catalogue::invalidate_facets(&self.options.project_root)?;
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
        let usage =
            crate::engine::project::usage::Usage::facet(request.kind.as_str(), &request.key)
                .map_err(super::error::invalid_request)?;
        let check = crate::catalogue_usage::UsageCheck::new(&self.root, usage);
        facets::remove_facet(&self.root, request.kind.as_str(), &request.key, || {
            Self::declaration_usage_result(check.used())
        })?;
        self.publish_declaration(request.kind.as_str(), &request.key)?;
        catalogue::invalidate_facets(&self.options.project_root)?;
        Ok(Removed { removed: true })
    }

    fn declaration_usage_result(
        result: Result<bool>,
    ) -> std::result::Result<bool, crate::engine::project::write::MutationError> {
        use crate::engine::project::write::MutationError;
        result.map_err(|error| match error {
            super::error::ApplicationError::InvalidRequest(message) => {
                MutationError::Validation(message)
            }
            error => MutationError::Internal(error.to_string()),
        })
    }
}
