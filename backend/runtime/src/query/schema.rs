// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Property types and picker metadata share one snapshot, refreshed on schema changes.
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;

use crate::engine::project::types::MetadataFieldValueType;

#[derive(Clone, Default)]
pub(crate) struct QuerySchema {
    keys: Arc<HashSet<String>>,
    types: Arc<HashMap<String, MetadataFieldValueType>>,
    metadata: Arc<Vec<crate::catalogue::MetadataField>>,
}

impl QuerySchema {
    pub(crate) fn from_properties(
        properties: impl Iterator<Item = (String, MetadataFieldValueType)>,
    ) -> Self {
        let types: HashMap<_, _> = properties.collect();
        Self {
            keys: Arc::new(types.keys().cloned().collect()),
            types: Arc::new(types),
            metadata: Arc::default(),
        }
    }

    pub(crate) fn from_metadata(metadata: Vec<crate::catalogue::MetadataField>) -> Self {
        let mut schema = Self::from_properties(
            metadata
                .iter()
                .filter(|field| field.key != "summary")
                .map(|field| (field.key.clone(), field.value_type)),
        );
        schema.metadata = Arc::new(metadata);
        schema
    }

    pub(crate) fn metadata(&self) -> &[crate::catalogue::MetadataField] {
        &self.metadata
    }

    pub(crate) fn value_type(&self, field: &str) -> Option<MetadataFieldValueType> {
        match field {
            "created" | "modified" | "completed" => Some(MetadataFieldValueType::Timestamp),
            "published_on" => Some(MetadataFieldValueType::Date),
            _ => self.types.get(field).copied(),
        }
    }
}

impl Deref for QuerySchema {
    type Target = HashSet<String>;
    fn deref(&self) -> &Self::Target {
        &self.keys
    }
}
