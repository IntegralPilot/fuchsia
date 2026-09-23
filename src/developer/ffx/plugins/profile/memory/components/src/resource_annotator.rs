// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use attribution_processing::{
    BlobAnnotation, ProcessedAttributionData, ResourceAnnotation, ZXName,
};
use std::collections::HashMap;

use fxfs_platform_constants::{BLOB_NAME_HASH_LENGTH, BLOB_NAME_PREFIX};

/// Annotator for resources using information from the assembled system.
///
/// This struct holds a mapping from merkle root hashes to the corresponding blob information,
/// allowing it to annotate resources (like VMOs) with the blob they represent.
#[derive(Default)]
pub struct ResourceAnnotator {
    merkle_to_blob_info: HashMap<[u8; BLOB_NAME_HASH_LENGTH], Vec<ResourceAnnotation>>,
}

impl ResourceAnnotator {
    /// Creates a new [ResourceAnnotator] from an [AssembledSystem].
    ///
    /// This iterates over the images in the assembled system to find BlobFS entries
    /// and builds the mapping from merkle root to blob metadata.
    pub fn new_from(
        assembled_system: assembled_system::AssembledSystem,
    ) -> Result<ResourceAnnotator> {
        let mut merkle_to_blob_info: HashMap<[u8; BLOB_NAME_HASH_LENGTH], Vec<ResourceAnnotation>> =
            HashMap::new();
        for image in assembled_system.images.iter() {
            let blobfs_contents = match image {
                assembled_system::Image::BlobFS { contents, .. } => contents,
                assembled_system::Image::FxfsSparse { contents, .. } => contents,
                _ => {
                    continue;
                }
            };
            for metadata in blobfs_contents
                .packages
                .base
                .metadata
                .iter()
                .chain(blobfs_contents.packages.cache.metadata.iter())
            {
                for blob in &metadata.blobs {
                    merkle_to_blob_info
                        .entry(blob.merkle.as_bytes()[..BLOB_NAME_HASH_LENGTH].try_into()?)
                        .or_insert_with(Vec::new)
                        .push(ResourceAnnotation::Blob(BlobAnnotation {
                            manifest: metadata.manifest.to_string(),
                            path: blob.path.clone(),
                        }));
                }
            }
        }
        Ok(ResourceAnnotator { merkle_to_blob_info })
    }

    /// Annotates the resources in the provided [ProcessedAttributionData] with blob information.
    pub fn annotate(
        &self,
        mut attribution_data: ProcessedAttributionData,
    ) -> ProcessedAttributionData {
        if self.merkle_to_blob_info.is_empty() {
            return attribution_data;
        }
        for (_, resource) in attribution_data.resources.iter_mut() {
            if let Some(blob_hash) =
                get_blob_hash(&attribution_data.resource_names[resource.resource.name_index])
            {
                if let Some(blob_info) = self.merkle_to_blob_info.get(blob_hash) {
                    resource.annotations.extend(blob_info.iter().cloned());
                }
            }
        }
        attribution_data
    }
}

/// Returns the blob hash suffix from a VMO name if it starts with the blob prefix.
///
/// VMOs holding blobs served by fxfs start with "blob-". This function extracts
/// the hex hash following that prefix.
fn get_blob_hash(vmo_name: &ZXName) -> Option<&[u8]> {
    let vmo_name = vmo_name.as_bytes();
    if vmo_name.starts_with(BLOB_NAME_PREFIX.as_bytes()) {
        Some(&vmo_name[BLOB_NAME_PREFIX.len()..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attribution_processing::{InflatedResource, ProcessedAttributionData, Resource};
    use fdomain_fuchsia_memory_attribution_plugin as fplugin;
    use rustc_hash::FxHashMap;

    #[test]
    fn test_get_blob_hash() {
        assert_eq!(
            get_blob_hash(&ZXName::from_string_lossy("blob-12345678")),
            Some(&b"12345678"[..])
        );
        assert_eq!(get_blob_hash(&ZXName::from_string_lossy("other")), None);
        assert_eq!(get_blob_hash(&ZXName::from_string_lossy("blo-1234")), None);
    }

    #[test]
    fn test_annotate() {
        let mut annotator = ResourceAnnotator::default();
        annotator.merkle_to_blob_info.insert(
            *b"12345678",
            vec![ResourceAnnotation::Blob(BlobAnnotation {
                manifest: "manifest".to_string(),
                path: "path".to_string(),
            })],
        );

        let mut resources = FxHashMap::default();
        // VMO with a blob name
        resources.insert(
            1,
            InflatedResource {
                resource: Resource {
                    koid: 1,
                    name_index: 0,
                    resource_type: fplugin::ResourceType::Vmo(Default::default()),
                },
                annotations: Default::default(),
                claims: Default::default(),
            },
        );
        // VMO without a blob name
        resources.insert(
            2,
            InflatedResource {
                resource: Resource {
                    koid: 2,
                    name_index: 1,
                    resource_type: fplugin::ResourceType::Vmo(Default::default()),
                },
                annotations: Default::default(),
                claims: Default::default(),
            },
        );

        let resource_names =
            vec![ZXName::from_string_lossy("blob-12345678"), ZXName::from_string_lossy("other")];

        let attribution_data = ProcessedAttributionData {
            principals: FxHashMap::default(),
            resources,
            resource_names,
        };

        let result = annotator.annotate(attribution_data);
        let resource = result.resources.get(&1).unwrap();
        assert_eq!(resource.annotations.len(), 1);
        match &resource.annotations[0] {
            ResourceAnnotation::Blob(blob) => {
                assert_eq!(blob.manifest, "manifest");
                assert_eq!(blob.path, "path");
            }
        }

        let resource2 = result.resources.get(&2).unwrap();
        assert!(resource2.annotations.is_empty());
    }
}
