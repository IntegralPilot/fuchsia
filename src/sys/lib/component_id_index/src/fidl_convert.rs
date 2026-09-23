// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Index, IndexEntry, InstanceId, InstanceIdError, PersistedIndex, ValidationError};
use fidl_fuchsia_component_internal as fcomponent_internal;
use moniker::{Moniker, MonikerError};
use thiserror::Error;

#[derive(Error, Clone, Debug, PartialEq)]
pub enum FidlConversionError {
    #[error("Missing instances")]
    MissingInstances,
    #[error("Missing instance_id")]
    MissingInstanceId,
    #[error("Missing moniker")]
    MissingMoniker,
    #[error("Invalid instance ID")]
    InstanceIdError(#[from] InstanceIdError),
    #[error("Invalid moniker")]
    MonikerError(#[from] MonikerError),
    #[error("invalid index")]
    ValidationError(#[from] ValidationError),
}

impl TryFrom<fcomponent_internal::ComponentIdIndex> for PersistedIndex {
    type Error = FidlConversionError;

    fn try_from(index: fcomponent_internal::ComponentIdIndex) -> Result<Self, Self::Error> {
        let mut instances: Vec<IndexEntry> = vec![];
        for entry in index.instances.ok_or_else(|| FidlConversionError::MissingInstances)? {
            instances.push(IndexEntry {
                instance_id: entry
                    .instance_id
                    .ok_or_else(|| FidlConversionError::MissingInstanceId)?
                    .parse::<InstanceId>()?,
                moniker: Moniker::parse_str(
                    &entry.moniker.ok_or_else(|| FidlConversionError::MissingMoniker)?,
                )?,
                ignore_duplicate_id: entry.ignore_duplicate_id.unwrap_or(false),
            });
        }
        Ok(PersistedIndex { instances })
    }
}

impl TryFrom<fcomponent_internal::ComponentIdIndex> for Index {
    type Error = FidlConversionError;

    fn try_from(index: fcomponent_internal::ComponentIdIndex) -> Result<Self, Self::Error> {
        let persisted_index: PersistedIndex = index.try_into()?;
        persisted_index.try_into().map_err(FidlConversionError::ValidationError)
    }
}

impl From<PersistedIndex> for fcomponent_internal::ComponentIdIndex {
    fn from(index: PersistedIndex) -> Self {
        fcomponent_internal::ComponentIdIndex {
            instances: Some(
                index
                    .instances
                    .into_iter()
                    .map(|entry| fcomponent_internal::InstanceIdEntry {
                        instance_id: Some(entry.instance_id.to_string()),
                        moniker: Some(entry.moniker.to_string()),
                        ignore_duplicate_id: Some(entry.ignore_duplicate_id),
                        ..Default::default()
                    })
                    .collect(),
            ),
            ..Default::default()
        }
    }
}

impl From<Index> for fcomponent_internal::ComponentIdIndex {
    fn from(index: Index) -> Self {
        let persisted_index: PersistedIndex = index.into();
        persisted_index.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fidl_native_translation() {
        let fidl_index = fcomponent_internal::ComponentIdIndex {
            instances: Some(vec![fcomponent_internal::InstanceIdEntry {
                instance_id: Some(
                    "8c90d44863ff67586cf6961081feba4f760decab8bbbee376a3bfbc77b351280".to_string(),
                ),
                moniker: Some("a/b/c".to_string()),
                ignore_duplicate_id: Some(false),
                ..Default::default()
            }]),
            ..Default::default()
        };

        let native_index = PersistedIndex {
            instances: vec![IndexEntry {
                instance_id: "8c90d44863ff67586cf6961081feba4f760decab8bbbee376a3bfbc77b351280"
                    .parse::<InstanceId>()
                    .unwrap(),
                moniker: Moniker::parse_str("a/b/c").unwrap(),
                ignore_duplicate_id: false,
            }],
        };

        assert_eq!(native_index, PersistedIndex::try_from(fidl_index.clone()).unwrap());
        assert_eq!(fidl_index, fcomponent_internal::ComponentIdIndex::from(native_index));
    }

    #[test]
    fn missing_instances() {
        assert_eq!(
            Some(FidlConversionError::MissingInstances),
            PersistedIndex::try_from(fcomponent_internal::ComponentIdIndex {
                ..Default::default()
            })
            .err()
        );
    }

    #[test]
    fn missing_instance_id() {
        assert_eq!(
            Some(FidlConversionError::MissingInstanceId),
            PersistedIndex::try_from(fcomponent_internal::ComponentIdIndex {
                instances: Some(vec![fcomponent_internal::InstanceIdEntry {
                    instance_id: None,
                    moniker: Some("a/b/c".to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            })
            .err()
        );
    }

    #[test]
    fn missing_moniker() {
        assert_eq!(
            Some(FidlConversionError::MissingMoniker),
            PersistedIndex::try_from(fcomponent_internal::ComponentIdIndex {
                instances: Some(vec![fcomponent_internal::InstanceIdEntry {
                    instance_id: Some(
                        "8c90d44863ff67586cf6961081feba4f760decab8bbbee376a3bfbc77b351280"
                            .to_string()
                    ),
                    moniker: None,
                    ..Default::default()
                }]),
                ..Default::default()
            })
            .err()
        );
    }
}
