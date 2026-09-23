// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This library must remain platform-agnostic because it used by a host tool and within Fuchsia.

use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use clonable_error::ClonableError;
use fidl_fuchsia_component_internal as fcomponent_internal;
use moniker::Moniker;
use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

pub mod fidl_convert;
mod instance_id;

pub use instance_id::{InstanceId, InstanceIdError};

/// Component ID index entry
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct IndexEntry {
    pub instance_id: InstanceId,
    pub moniker: Moniker,
    #[cfg_attr(feature = "serde", serde(default))]
    pub ignore_duplicate_id: bool,
}

/// Component ID index, only used for persistence to JSON5 and FIDL.
///
/// Unlike [Index], this type is not validated, so may contain duplicate monikers
/// and instance IDs.
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct PersistedIndex {
    instances: Vec<IndexEntry>,
}

/// A processed index of component instance IDs.
///
/// Unlike [PersistedIndex], this type is validated to only contain unique instance IDs.
#[cfg_attr(
    feature = "serde",
    derive(Deserialize, Serialize),
    serde(try_from = "PersistedIndex", into = "PersistedIndex")
)]
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct Index {
    /// All of the entries in the index
    instances: Vec<IndexEntry>,

    /// All instance IDs, derived from the contents of `instances`.
    instance_ids: HashSet<InstanceId>,

    /// All monikers, derived from the contents of `instances`.
    monikers: HashSet<Moniker>,
}

#[derive(Error, Clone, Debug)]
pub enum IndexError {
    #[error("failed to read index file '{path}'")]
    ReadFile {
        #[source]
        err: ClonableError,
        path: Utf8PathBuf,
    },
    #[error("invalid index")]
    ValidationError(#[from] ValidationError),
    #[error("could not merge indices")]
    MergeError(#[from] MergeError),
    #[error("could not convert FIDL index")]
    FidlConversionError(#[from] fidl_convert::FidlConversionError),
}

impl Index {
    /// Return an Index parsed from the FIDL file at `path`.
    pub fn from_fidl_file(path: &Utf8Path) -> Result<Self, IndexError> {
        fn fidl_index_from_file(
            path: &Utf8Path,
        ) -> Result<fcomponent_internal::ComponentIdIndex, anyhow::Error> {
            let raw_content = std::fs::read(path).context("failed to read file")?;
            let fidl_index = fidl::unpersist::<fcomponent_internal::ComponentIdIndex>(&raw_content)
                .context("failed to unpersist FIDL")?;
            Ok(fidl_index)
        }
        let fidl_index = fidl_index_from_file(path)
            .map_err(|err| IndexError::ReadFile { err: err.into(), path: path.to_owned() })?;
        let index = Index::try_from(fidl_index)?;
        Ok(index)
    }

    /// Construct an Index by merging JSON5 source files.
    #[cfg(feature = "serde")]
    pub fn merged_from_json5_files(paths: &[Utf8PathBuf]) -> Result<Self, IndexError> {
        fn index_from_json5_file(path: &Utf8Path) -> Result<Index, anyhow::Error> {
            let mut file = std::fs::File::open(&path).context("failed to open")?;
            let index: Index = serde_json5::from_reader(&mut file).context("failed to parse")?;
            Ok(index)
        }
        let mut ctx = MergeContext::default();
        for path in paths {
            let index = index_from_json5_file(path)
                .map_err(|err| IndexError::ReadFile { err: err.into(), path: path.to_owned() })?;
            ctx.merge(path, &index)?;
        }
        Ok(ctx.output())
    }

    /// Insert an entry into the index.
    pub fn insert(&mut self, entry: IndexEntry) -> Result<(), ValidationError> {
        if !entry.ignore_duplicate_id {
            if !self.instance_ids.insert(entry.instance_id.clone()) {
                return Err(ValidationError::DuplicateId(entry.instance_id));
            }
        }
        if !self.monikers.insert(entry.moniker.clone()) {
            return Err(ValidationError::DuplicateMoniker(entry.moniker));
        }
        self.instances.push(entry);
        Ok(())
    }

    /// Returns the instance ID for the moniker, if the index contains the moniker.
    pub fn id_for_moniker(&self, moniker: &Moniker) -> Option<&InstanceId> {
        self.instances.iter().find(|e| &e.moniker == moniker).map(|e| &e.instance_id)
    }

    /// Returns the moniker for the instance ID, if the index contains the instance ID. Will return
    /// just one of the matches if there are duplicate IDs.
    pub fn moniker_for_id(&self, id: &InstanceId) -> Option<&Moniker> {
        self.instances.iter().find(|e| &e.instance_id == id).map(|e| &e.moniker)
    }

    /// Returns true if the index contains the instance ID.
    pub fn contains_id(&self, id: &InstanceId) -> bool {
        self.instance_ids.contains(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &IndexEntry> {
        self.instances.iter()
    }
}

impl TryFrom<PersistedIndex> for Index {
    type Error = ValidationError;

    fn try_from(value: PersistedIndex) -> Result<Self, Self::Error> {
        let mut index = Index::default();
        for entry in value.instances.into_iter() {
            index.insert(entry)?;
        }
        Ok(index)
    }
}

impl From<Index> for PersistedIndex {
    fn from(mut value: Index) -> Self {
        value.instances.sort_by(|a, b| a.moniker.cmp(&b.moniker));
        Self { instances: value.instances }
    }
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum ValidationError {
    #[error("duplicate moniker: {}", .0)]
    DuplicateMoniker(Moniker),
    #[error("duplicate instance ID: {}", .0)]
    DuplicateId(InstanceId),
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum MergeError {
    #[error("Moniker {}' must be unique but exists in following index files:\n {}\n {}", .moniker, .source1, .source2)]
    DuplicateMoniker { moniker: Moniker, source1: Utf8PathBuf, source2: Utf8PathBuf },
    #[error("Instance ID '{}' must be unique but exists in following index files:\n {}\n {}", .instance_id, .source1, .source2)]
    DuplicateId { instance_id: InstanceId, source1: Utf8PathBuf, source2: Utf8PathBuf },
}

/// A builder that merges indices into a single accumulated index.
pub struct MergeContext {
    /// Index that contains entries accumulated from calls to [`merge()`].
    output_index: Index,
    // Path to the source index file that contains the moniker.
    moniker_to_source_path: HashMap<Moniker, Utf8PathBuf>,
    // Path to the source index file that contains the instance ID.
    instance_id_to_source_path: HashMap<InstanceId, Utf8PathBuf>,
}

impl MergeContext {
    // Merge `index` into the into the MergeContext.
    //
    // This method can be called multiple times to merge multiple indices.
    // The resulting index can be accessed with output().
    pub fn merge(&mut self, source_index_path: &Utf8Path, index: &Index) -> Result<(), MergeError> {
        for instance in &index.instances {
            self.output_index.insert(instance.clone()).map_err(|err| match err {
                ValidationError::DuplicateMoniker(moniker) => {
                    let previous_source_path =
                        self.moniker_to_source_path.get(&moniker).cloned().unwrap_or_default();
                    MergeError::DuplicateMoniker {
                        moniker,
                        source1: previous_source_path,
                        source2: source_index_path.to_owned(),
                    }
                }
                ValidationError::DuplicateId(instance_id) => {
                    let previous_source_path = self
                        .instance_id_to_source_path
                        .get(&instance_id)
                        .cloned()
                        .unwrap_or_default();
                    MergeError::DuplicateId {
                        instance_id,
                        source1: previous_source_path,
                        source2: source_index_path.to_owned(),
                    }
                }
            })?;
            self.instance_id_to_source_path
                .insert(instance.instance_id.clone(), source_index_path.to_owned());
            self.moniker_to_source_path
                .insert(instance.moniker.clone(), source_index_path.to_owned());
        }
        Ok(())
    }

    // Return the accumulated index from calls to merge().
    pub fn output(self) -> Index {
        self.output_index
    }
}

impl Default for MergeContext {
    fn default() -> Self {
        MergeContext {
            output_index: Index::default(),
            instance_id_to_source_path: HashMap::new(),
            moniker_to_source_path: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    #[test]
    fn merge_empty_index() {
        let ctx = MergeContext::default();
        assert_eq!(ctx.output(), Index::default());
    }

    #[test]
    fn merge_single_index() -> Result<()> {
        let mut ctx = MergeContext::default();

        let mut index = Index::default();
        let moniker = ["foo"].try_into().unwrap();
        let instance_id = InstanceId::new_random(&mut rand::rng());
        index.insert(IndexEntry { moniker, instance_id, ignore_duplicate_id: false }).unwrap();

        ctx.merge(Utf8Path::new("/random/file/path"), &index)?;
        assert_eq!(ctx.output(), index.clone());
        Ok(())
    }

    #[test]
    fn merge_duplicate_id() -> Result<()> {
        let source1 = Utf8Path::new("/a/b/c");
        let source2 = Utf8Path::new("/d/e/f");

        let id = InstanceId::new_random(&mut rand::rng());
        let index1 = {
            let mut index = Index::default();
            let moniker = ["foo"].try_into().unwrap();
            index
                .insert(IndexEntry { moniker, instance_id: id.clone(), ignore_duplicate_id: false })
                .unwrap();
            index
        };
        let index2 = {
            let mut index = Index::default();
            let moniker = ["bar"].try_into().unwrap();
            index
                .insert(IndexEntry { moniker, instance_id: id.clone(), ignore_duplicate_id: false })
                .unwrap();
            index
        };

        let mut ctx = MergeContext::default();
        ctx.merge(source1, &index1)?;

        let err = ctx.merge(source2, &index2).unwrap_err();
        assert_eq!(
            err,
            MergeError::DuplicateId {
                instance_id: id,
                source1: source1.to_owned(),
                source2: source2.to_owned()
            }
        );

        Ok(())
    }

    #[test]
    fn merge_duplicate_id_with_ignore_flag() {
        let source1 = Utf8Path::new("/a/b/c");
        let source2 = Utf8Path::new("/d/e/f");

        let id = InstanceId::new_random(&mut rand::rng());
        let index1 = {
            let mut index = Index::default();
            let moniker = ["foo"].try_into().unwrap();
            index
                .insert(IndexEntry { moniker, instance_id: id.clone(), ignore_duplicate_id: false })
                .unwrap();
            index
        };
        let index2 = {
            let mut index = Index::default();
            let moniker = ["bar"].try_into().unwrap();
            index
                .insert(IndexEntry { moniker, instance_id: id.clone(), ignore_duplicate_id: true })
                .unwrap();
            index
        };

        let mut ctx = MergeContext::default();
        ctx.merge(source1, &index1).expect("unexpected merge error");
        ctx.merge(source2, &index2).expect("unexpected merge error");
    }

    #[test]
    fn merge_duplicate_moniker() -> Result<()> {
        let source1 = Utf8Path::new("/a/b/c");
        let source2 = Utf8Path::new("/d/e/f");

        let moniker: Moniker = ["foo"].try_into().unwrap();
        let index1 = {
            let mut index = Index::default();
            let instance_id = InstanceId::new_random(&mut rand::rng());
            index
                .insert(IndexEntry {
                    moniker: moniker.clone(),
                    instance_id,
                    ignore_duplicate_id: false,
                })
                .unwrap();
            index
        };
        let index2 = {
            let mut index = Index::default();
            let instance_id = InstanceId::new_random(&mut rand::rng());
            index
                .insert(IndexEntry {
                    moniker: moniker.clone(),
                    instance_id,
                    ignore_duplicate_id: false,
                })
                .unwrap();
            index
        };

        let mut ctx = MergeContext::default();
        ctx.merge(source1, &index1)?;

        let err = ctx.merge(source2, &index2).unwrap_err();
        assert_eq!(
            err,
            MergeError::DuplicateMoniker {
                moniker,
                source1: source1.to_owned(),
                source2: source2.to_owned()
            }
        );

        Ok(())
    }

    #[cfg(feature = "serde")]
    #[test]
    fn merged_from_json5_files() {
        use std::io::Write;

        let mut index_file_1 = tempfile::NamedTempFile::new().unwrap();
        index_file_1
            .write_all(
                r#"{
            // Here is a comment.
            instances: [
                {
                    instance_id: "fb94044d62278b37c221c7fdeebdcf1304262f3e11416f68befa5ef88b7a2163",
                    moniker: "/a/b"
                }
            ]
        }"#
                .as_bytes(),
            )
            .unwrap();

        let mut index_file_2 = tempfile::NamedTempFile::new().unwrap();
        index_file_2
            .write_all(
                r#"{
            // Here is a comment.
            instances: [
                {
                    instance_id: "4f915af6c4b682867ab7ad2dc9cbca18342ddd9eec61724f19d231cf6d07f122",
                    moniker: "/c/d"
                }
            ]
        }"#
                .as_bytes(),
            )
            .unwrap();

        let expected_index = {
            let mut index = Index::default();
            index
                .insert(IndexEntry {
                    moniker: "/a/b".parse::<Moniker>().unwrap(),
                    instance_id: "fb94044d62278b37c221c7fdeebdcf1304262f3e11416f68befa5ef88b7a2163"
                        .parse::<InstanceId>()
                        .unwrap(),
                    ignore_duplicate_id: false,
                })
                .unwrap();
            index
                .insert(IndexEntry {
                    moniker: "/c/d".parse::<Moniker>().unwrap(),
                    instance_id: "4f915af6c4b682867ab7ad2dc9cbca18342ddd9eec61724f19d231cf6d07f122"
                        .parse::<InstanceId>()
                        .unwrap(),
                    ignore_duplicate_id: false,
                })
                .unwrap();
            index
        };

        // only checking that we parsed successfully.
        let files = [
            Utf8PathBuf::from_path_buf(index_file_1.path().to_path_buf()).unwrap(),
            Utf8PathBuf::from_path_buf(index_file_2.path().to_path_buf()).unwrap(),
        ];
        assert_eq!(expected_index, Index::merged_from_json5_files(&files).unwrap());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serialize_deserialize() -> Result<()> {
        let expected_index = {
            let mut index = Index::default();
            for i in 0..5 {
                let moniker: Moniker = [i.to_string().as_str()].try_into().unwrap();
                let instance_id = InstanceId::new_random(&mut rand::rng());
                index
                    .insert(IndexEntry { moniker, instance_id, ignore_duplicate_id: false })
                    .unwrap();
            }
            index
        };

        let json_index = serde_json5::to_string(&expected_index)?;
        let actual_index = serde_json5::from_str(&json_index)?;
        assert_eq!(expected_index, actual_index);

        Ok(())
    }
}
