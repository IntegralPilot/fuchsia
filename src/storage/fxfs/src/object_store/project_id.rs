// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::FxfsError;
use crate::lsm_tree::Query;
use crate::lsm_tree::types::{ItemRef, LayerIterator};
use crate::object_store::transaction::{LockKey, Mutation, Options, lock_keys};
use crate::object_store::{
    ObjectKey, ObjectKeyData, ObjectKind, ObjectStore, ObjectValue, ProjectProperty,
};
use anyhow::{Error, anyhow};
use fprint::TypeFingerprint;
use fxfs_macros::SerializeKey;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

impl ObjectStore {
    /// Adds a mutation to set the project limit as an attribute with `bytes` and `nodes` to root
    /// node.
    pub async fn set_project_limit(
        &self,
        project_id: ProjectId,
        bytes: u64,
        nodes: u64,
    ) -> Result<(), Error> {
        let root_id = self.root_directory_object_id();
        let mut transaction = self
            .new_transaction(
                lock_keys![LockKey::ProjectId {
                    store_object_id: self.store_object_id,
                    project_id
                }],
                Options::default(),
            )
            .await?;
        transaction.add(
            self.store_object_id,
            Mutation::replace_or_insert_object(
                ObjectKey::project_limit(root_id, project_id),
                ObjectValue::BytesAndNodes {
                    bytes: bytes.try_into().map_err(|_| FxfsError::TooBig)?,
                    nodes: nodes.try_into().map_err(|_| FxfsError::TooBig)?,
                },
            ),
        );
        transaction.commit().await?;
        Ok(())
    }

    /// Clear the limit for a project by tombstoning the limits and usage attributes for the
    /// given `project_id`. Fails if the project is still in use by one or more nodes.
    pub async fn clear_project_limit(&self, project_id: ProjectId) -> Result<(), Error> {
        let root_id = self.root_directory_object_id();
        let mut transaction = self
            .new_transaction(
                lock_keys![LockKey::ProjectId {
                    store_object_id: self.store_object_id,
                    project_id
                }],
                Options::default(),
            )
            .await?;
        transaction.add(
            self.store_object_id,
            Mutation::replace_or_insert_object(
                ObjectKey::project_limit(root_id, project_id),
                ObjectValue::None,
            ),
        );
        transaction.commit().await?;
        Ok(())
    }

    /// Apply a `project_id` to a given node. Fails if node is not found or target project is zero.
    pub async fn set_project_for_node(
        &self,
        node_id: u64,
        project_id: ProjectId,
    ) -> Result<(), Error> {
        let root_id = self.root_directory_object_id();
        let mut transaction = self
            .new_transaction(
                lock_keys![LockKey::object(self.store_object_id, node_id)],
                Options::default(),
            )
            .await?;

        let object_key = ObjectKey::object(node_id);
        let (kind, mut attributes) =
            match self.tree().find_value(&object_key).await?.ok_or(FxfsError::NotFound)? {
                ObjectValue::Object { kind, attributes } => (kind, attributes),
                _ => return Err(FxfsError::Inconsistent.into()),
            };
        // Make sure the object kind makes sense.
        match kind {
            ObjectKind::File { .. } | ObjectKind::Directory { .. } => (),
            // For now, we don't support attributes on symlink objects, so setting a project id
            // doesn't make sense.
            ObjectKind::Symlink { .. } | ObjectKind::EncryptedSymlink { .. } => {
                return Err(FxfsError::NotSupported.into());
            }
            ObjectKind::Graveyard => return Err(FxfsError::Inconsistent.into()),
        }
        let storage_size = attributes.allocated_size.try_into().map_err(|_| FxfsError::TooBig)?;
        let old_project_id = attributes.project_id;
        if old_project_id == Some(project_id) {
            return Ok(());
        }
        attributes.project_id = Some(project_id);

        transaction.add(
            self.store_object_id,
            Mutation::replace_or_insert_object(
                object_key,
                ObjectValue::Object { kind, attributes },
            ),
        );
        transaction.add(
            self.store_object_id,
            Mutation::merge_object(
                ObjectKey::project_usage(root_id, project_id),
                ObjectValue::BytesAndNodes { bytes: storage_size, nodes: 1 },
            ),
        );
        if let Some(old_project_id) = old_project_id {
            transaction.add(
                self.store_object_id,
                Mutation::merge_object(
                    ObjectKey::project_usage(root_id, old_project_id),
                    ObjectValue::BytesAndNodes { bytes: -storage_size, nodes: -1 },
                ),
            );
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Return the project_id associated with the given `node_id`.
    pub async fn get_project_for_node(&self, node_id: u64) -> Result<Option<ProjectId>, Error> {
        let project_id = self
            .tree()
            .find_map(&ObjectKey::object(node_id), |item| match item.value {
                ObjectValue::Object { attributes, .. } => Ok(attributes.project_id),
                _ => Err(anyhow!(FxfsError::Inconsistent)),
            })
            .await?
            .ok_or(FxfsError::NotFound)??;
        Ok(project_id)
    }

    /// Remove the project id for a given `node_id`. The call will do nothing and return success
    /// if the node is found to not be associated with any project.
    pub async fn clear_project_for_node(&self, node_id: u64) -> Result<(), Error> {
        let root_id = self.root_directory_object_id();
        let mut transaction = self
            .new_transaction(
                lock_keys![LockKey::object(self.store_object_id, node_id)],
                Options::default(),
            )
            .await?;

        let object_key = ObjectKey::object(node_id);
        let (kind, mut attributes) =
            match self.tree().find_value(&object_key).await?.ok_or(FxfsError::NotFound)? {
                ObjectValue::Object { kind, attributes } => (kind, attributes),
                _ => return Err(FxfsError::Inconsistent.into()),
            };
        let Some(old_project_id) = attributes.project_id else {
            return Ok(());
        };
        // Make sure the object kind makes sense.
        match kind {
            ObjectKind::File { .. } | ObjectKind::Directory { .. } => (),
            // For now, we don't support attributes on symlink objects, so setting a project id
            // doesn't make sense.
            ObjectKind::Symlink { .. } | ObjectKind::EncryptedSymlink { .. } => {
                return Err(FxfsError::NotSupported.into());
            }
            ObjectKind::Graveyard => return Err(FxfsError::Inconsistent.into()),
        }
        attributes.project_id = None;
        let storage_size = attributes.allocated_size;
        transaction.add(
            self.store_object_id,
            Mutation::replace_or_insert_object(
                object_key,
                ObjectValue::Object { kind, attributes },
            ),
        );
        // Not safe to convert storage_size to i64, as space usage can exceed i64 in size. Not
        // going to deal with handling such enormous files, fail the request.
        transaction.add(
            self.store_object_id,
            Mutation::merge_object(
                ObjectKey::project_usage(root_id, old_project_id),
                ObjectValue::BytesAndNodes {
                    bytes: -(storage_size.try_into().map_err(|_| FxfsError::TooBig)?),
                    nodes: -1,
                },
            ),
        );
        transaction.commit().await?;
        Ok(())
    }

    /// Returns a list of project ids currently tracked with project limits or usage in ascending
    /// order, beginning after `last_id` and providing up to `max_entries`. If `max_entries` would
    /// be exceeded then it also returns the final id in the list, for use in the following call to
    /// resume the listing.
    pub async fn list_projects(
        &self,
        start_id: Option<ProjectId>,
        max_entries: usize,
    ) -> Result<(Vec<ProjectId>, Option<ProjectId>), Error> {
        let start_id = start_id.unwrap_or(ProjectId::SORTED_START);
        let root_dir_id = self.root_directory_object_id();
        let layer_set = self.tree().layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger
            .query(Query::FullRange(&ObjectKey::project_limit(root_dir_id, start_id)))
            .await?;
        let mut entries = Vec::new();
        let mut prev_entry: Option<ProjectId> = None;
        let mut next_entry = None;
        while let Some(ItemRef { key: ObjectKey { object_id, data: key_data }, value, .. }) =
            iter.get()
        {
            // We've moved outside the target object id.
            if *object_id != root_dir_id {
                break;
            }
            match key_data {
                ObjectKeyData::Project { project_id, .. } => {
                    // Bypass deleted or repeated entries.
                    if *value != ObjectValue::None && prev_entry < Some(*project_id) {
                        if entries.len() == max_entries {
                            next_entry = Some(*project_id);
                            break;
                        }
                        prev_entry = Some(*project_id);
                        entries.push(*project_id);
                    }
                }
                // We've moved outside the list of Project limits and usages.
                _ => {
                    break;
                }
            }
            iter.advance().await?;
        }
        // Skip deleted entries
        Ok((entries, next_entry))
    }

    /// Looks up the limit and usage of `project_id` as a pair of bytes and notes. Any of the two
    /// fields not found will return None for them.
    pub async fn project_info(
        &self,
        project_id: ProjectId,
    ) -> Result<(Option<(u64, u64)>, Option<(u64, u64)>), Error> {
        let root_id = self.root_directory_object_id();
        let layer_set = self.tree().layer_set();
        let mut merger = layer_set.merger();
        let mut iter =
            merger.query(Query::FullRange(&ObjectKey::project_limit(root_id, project_id))).await?;
        let mut limit = None;
        let mut usage = None;
        // The limit should be immediately followed by the usage if both exist.
        while let Some(ItemRef { key: ObjectKey { object_id, data: key_data }, value, .. }) =
            iter.get()
        {
            // Should be within the bounds of the root dir id.
            if *object_id != root_id {
                break;
            }
            if let (
                ObjectKeyData::Project { project_id: found_project_id, property },
                ObjectValue::BytesAndNodes { bytes, nodes },
            ) = (key_data, value)
            {
                // Outside the range for target project information.
                if *found_project_id != project_id {
                    break;
                }
                let raw_value: (u64, u64) = (
                    // Should succeed in conversions since they shouldn't be negative.
                    (*bytes).try_into().map_err(|_| FxfsError::Inconsistent)?,
                    (*nodes).try_into().map_err(|_| FxfsError::Inconsistent)?,
                );
                match property {
                    ProjectProperty::Limit => limit = Some(raw_value),
                    ProjectProperty::Usage => usage = Some(raw_value),
                }
            } else {
                break;
            }
            iter.advance().await?;
        }
        Ok((limit, usage))
    }
}

#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Debug,
    Serialize,
    Deserialize,
    Hash,
    TypeFingerprint,
    SerializeKey,
)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
#[repr(transparent)]
pub struct ProjectId(NonZeroU64);

impl ProjectId {
    pub const SORTED_START: Self = Self::new(1).unwrap();

    pub const fn new(project_id: u64) -> Option<Self> {
        match NonZeroU64::new(project_id) {
            None => None,
            Some(non_zero) => Some(Self(non_zero)),
        }
    }

    /// Returns the underlying `u64`.
    pub const fn raw(self) -> u64 {
        self.0.get()
    }
}

impl log::kv::ToValue for ProjectId {
    fn to_value(&self) -> log::kv::Value<'_> {
        log::kv::Value::from(self.0)
    }
}

impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// An extension trait for project ids that makes working with `Option<ProjectId>` simpler.
pub trait ProjectIdExt {
    /// Returns the underlying `u64`.
    fn raw(self) -> u64;
}

impl ProjectIdExt for Option<ProjectId> {
    fn raw(self) -> u64 {
        match self {
            None => 0,
            Some(project_id) => project_id.raw(),
        }
    }
}

pub mod optional_project_id {
    use super::{ProjectId, ProjectIdExt};
    use serde::{Deserializer, Serializer};

    /// Serialize `Option<ProjectId>` as a u64.
    pub fn serialize<S>(value: &Option<ProjectId>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(value.raw())
    }

    /// Deserialize `Option<ProjectId>` from a u64.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<ProjectId>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_u64(Visitor)
    }

    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Option<ProjectId>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "a u64",)
        }

        fn visit_u64<E>(self, raw: u64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(ProjectId::new(raw))
        }
    }

    pub fn fingerprint<T>() -> String {
        "u64".to_string()
    }
}

#[cfg(test)]
mod tests {
    // ObjectStore project id tests are done end to end from the Fuchsia endpoint and so are in
    // platform/fuchsia/volume.rs

    use super::{ProjectId, ProjectIdExt};
    use crate::serialized_types::{LATEST_VERSION, Versioned};
    use serde::{Deserialize, Serialize};

    // Versioned is used here to get the same bincode settings as would be used when the ProjectId
    // is contained inside of a Versioned struct.
    impl Versioned for ProjectId {}

    #[test]
    fn test_project_id_serialization_matches_u64() {
        fn verify_matches(x: u64) {
            // 1. Serialize x as u64
            let mut u64_buf = Vec::new();
            x.serialize_into(&mut u64_buf).unwrap();

            // 2. Serialize ProjectId
            let project_id = ProjectId::new(x).unwrap();
            let mut project_id_buf = Vec::new();
            project_id.serialize_into(&mut project_id_buf).unwrap();

            // 3. Verify binary match
            assert_eq!(u64_buf, project_id_buf);

            // 4. Deserialize u64 bytes as ProjectId
            let deserialized_project_id =
                ProjectId::deserialize_from(&mut u64_buf.as_slice(), LATEST_VERSION).unwrap();
            assert_eq!(deserialized_project_id, project_id);

            // 5. Deserialize ProjectId bytes as u64
            let deserialized_u64 =
                u64::deserialize_from(&mut project_id_buf.as_slice(), LATEST_VERSION).unwrap();
            assert_eq!(deserialized_u64, x);
        }

        verify_matches(1);
        verify_matches(2);
        verify_matches(u16::MAX as u64 - 1);
        verify_matches(u16::MAX as u64);
        verify_matches(u16::MAX as u64 + 1);
        verify_matches(u32::MAX as u64 - 1);
        verify_matches(u32::MAX as u64);
        verify_matches(u32::MAX as u64 + 1);
        verify_matches(u64::MAX - 1);
        verify_matches(u64::MAX);
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Versioned)]
    struct OptionWrapper {
        #[serde(with = "super::optional_project_id")]
        project_id: Option<ProjectId>,
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Versioned)]
    struct U64Wrapper {
        project_id: u64,
    }

    #[test]
    fn test_optional_project_id_serialization_matches_u64() {
        fn verify_matches(x: u64) {
            // 1. Serialize x as u64 inside wrapper
            let u64_wrapper = U64Wrapper { project_id: x };
            let mut u64_buf = Vec::new();
            u64_wrapper.serialize_into(&mut u64_buf).unwrap();

            // 2. Serialize Option<ProjectId> inside wrapper
            let opt_project_id = OptionWrapper { project_id: ProjectId::new(x) };
            let mut opt_buf = Vec::new();
            opt_project_id.serialize_into(&mut opt_buf).unwrap();

            // 3. Verify their binary representations match exactly
            assert_eq!(u64_buf, opt_buf);

            // 4. Deserialize the serialized u64 bytes as Option<ProjectId>
            let deserialized_opt =
                OptionWrapper::deserialize_from(&mut u64_buf.as_slice(), LATEST_VERSION).unwrap();
            assert_eq!(deserialized_opt, opt_project_id);

            // 5. Deserialize the serialized Option<ProjectId> bytes as u64
            let deserialized_u64 =
                U64Wrapper::deserialize_from(&mut opt_buf.as_slice(), LATEST_VERSION).unwrap();
            assert_eq!(deserialized_u64, u64_wrapper);
        }

        verify_matches(0);
        verify_matches(1);
        verify_matches(2);
        verify_matches(u16::MAX as u64 - 1);
        verify_matches(u16::MAX as u64);
        verify_matches(u16::MAX as u64 + 1);
        verify_matches(u32::MAX as u64 - 1);
        verify_matches(u32::MAX as u64);
        verify_matches(u32::MAX as u64 + 1);
        verify_matches(u64::MAX - 1);
        verify_matches(u64::MAX);
    }

    #[test]
    fn test_option_project_id_sorting() {
        let none = ProjectId::new(0);
        assert!(none.is_none());

        let one = ProjectId::new(1);
        let max = ProjectId::new(u64::MAX);

        // None should be sorted before all project ids.
        assert!(none < one);
        assert!(none < max);
        assert!(one < max);
    }

    #[test]
    fn test_project_id_raw() {
        assert_eq!(ProjectId::new(0).raw(), 0);
        assert_eq!(ProjectId::new(1).raw(), 1);
        assert_eq!(ProjectId::new(u64::MAX).raw(), u64::MAX);
    }
}
