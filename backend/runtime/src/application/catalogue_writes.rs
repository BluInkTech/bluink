// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::engine::project::{
    facets, properties,
    types::{FacetKind, MetadataFieldValueType},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Application;
use super::error::{Result, internal_error};
use crate::{catalogue, service};

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
pub(crate) struct AddPropertyOptionsRequest {
    key: String,
    value_type: MetadataFieldValueType,
    options: Vec<String>,
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

    pub(crate) fn add_property_options(
        &self,
        request: AddPropertyOptionsRequest,
    ) -> Result<catalogue::MetadataField> {
        let _operation = self.begin_operation()?;
        let declaration = properties::add_property_options(
            &self.root,
            &request.key,
            request.value_type.as_str(),
            serde_json::to_value(request.options).map_err(internal_error)?,
        )?;
        self.property_changed(declaration)
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    fn property_changed(&self, declaration: Value) -> Result<catalogue::MetadataField> {
        self.refresh_schema()?;
        catalogue::metadata_field(&declaration)
    }

    fn refresh_schema(&self) -> Result<()> {
        let index = self.state()?.index.as_ref().map(|index| index.path.clone());
        if let Some(index) = index {
            service::refresh_query_fields(index)?;
        }
        Ok(())
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn remove_property(&self, request: RemovePropertyRequest) -> Result<Removed> {
        let _operation = self.begin_operation()?;
        properties::remove_property(&self.root, &request.key)?;
        self.refresh_schema()?;
        Ok(Removed { removed: true })
    }

    pub(crate) fn create_facet(&self, request: FacetRequest) -> Result<catalogue::FacetEntry> {
        let _operation = self.begin_operation()?;
        facets::create_facet(&self.root, request.kind.as_str(), &request.key)?;
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
        facets::remove_facet(&self.root, request.kind.as_str(), &request.key)?;
        catalogue::invalidate_facets(&self.options.project_root)?;
        Ok(Removed { removed: true })
    }
}
