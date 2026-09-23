// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::FxfsError;
use crate::lsm_tree::Query;
use crate::lsm_tree::types::{ItemRef, LayerIterator};
use crate::object_handle::ObjectHandle;
use crate::object_store::object_record::{AttributeKey, ObjectKey, ObjectKeyData, ObjectValue};
use crate::object_store::{AttributeId, DataObjectHandle, HandleOwner, StoreObjectHandle};
use crate::serialized_types::{Versioned, VersionedLatest};
use anyhow::{Context, Error};
use fuchsia_merkle::{Hash, LeafHashCollector, MerkleVerifier};

pub use blob_metadata::{
    BlobFormat, BlobFormatV53, BlobMetadata, BlobMetadataUnversioned, BlobMetadataV53, MerkleLeaves,
};

pub trait FxfsBlobMetadataExt: Sized {
    /// Reads the blob metadata from an attribute on `blob_object`. If the attribute doesn't exist
    /// then it's assumed to be `BlobMetadata::empty()`.
    fn read_from<S: HandleOwner>(
        blob_object: &StoreObjectHandle<S>,
    ) -> impl std::future::Future<Output = Result<Self, Error>> + Send;

    /// Writes the metadata to the `AttributeId::BLOB_METADATA` attribute on `blob_object`. If the
    /// metadata is equal to `BlobMetadata::empty()` then the attribute isn't written.
    fn write_to<S: HandleOwner>(
        &self,
        blob_object: &DataObjectHandle<S>,
    ) -> impl std::future::Future<Output = Result<(), Error>> + Send;

    /// Returns the size of the serialized metadata. If the metadata is equal to
    /// `BlobMetadata::empty()` then the metadata won't get written, so 0 is returned.
    fn serialized_size(&self) -> Result<usize, Error>;

    /// Consumes the metadata and turns it into a `MerkleVerifier`.
    fn into_merkle_verifier(self, root: Hash) -> Result<MerkleVerifier, Error>;
}

impl FxfsBlobMetadataExt for BlobMetadata {
    fn read_from<S: HandleOwner>(
        blob_object: &StoreObjectHandle<S>,
    ) -> impl std::future::Future<Output = Result<Self, Error>> + Send {
        async move {
            let store = blob_object.store();
            let layer_set = store.tree().layer_set();
            let mut merger = layer_set.merger();
            // A blob should never have both attributes and also should never have the fs-verity
            // attribute which is ordered between them. Querying for `AttributeId::BLOB_MERKLE` will
            // have the iterator point to that attribute if it exists. If it doesn't exist then the
            // iterator will point the next item which will be the `AttributeId::BLOB_METADATA`
            // attribute if it exists.
            static_assertions::const_assert!(
                AttributeId::BLOB_MERKLE.raw() < AttributeId::BLOB_METADATA.raw()
            );
            let key = ObjectKey::attribute(
                blob_object.object_id(),
                AttributeId::BLOB_MERKLE,
                AttributeKey::Attribute,
            );
            let iter = merger.query(Query::FullRange(&key)).await?;
            match iter.get() {
                Some(ItemRef {
                    key:
                        ObjectKey {
                            object_id,
                            data:
                                ObjectKeyData::Attribute(
                                    AttributeId::BLOB_MERKLE,
                                    AttributeKey::Attribute,
                                ),
                        },
                    value,
                    ..
                }) if *object_id == blob_object.object_id() => match value {
                    ObjectValue::Attribute { .. } => {
                        let serialized_metadata = blob_object.read_attr_from_iter(iter).await?;
                        let old_metadata: BlobMetadataUnversioned =
                            bincode::deserialize_from(&*serialized_metadata)?;
                        Ok(Self::from(old_metadata))
                    }
                    _ => Err(FxfsError::Inconsistent.into()),
                },
                Some(ItemRef {
                    key:
                        ObjectKey {
                            object_id,
                            data:
                                ObjectKeyData::Attribute(
                                    AttributeId::BLOB_METADATA,
                                    AttributeKey::Attribute,
                                ),
                        },
                    value,
                    ..
                }) if *object_id == blob_object.object_id() => match value {
                    ObjectValue::Attribute { .. } => {
                        let serialized_metadata = blob_object.read_attr_from_iter(iter).await?;
                        Ok(Self::deserialize_with_version(&mut &*serialized_metadata)?.0)
                    }
                    _ => Err(FxfsError::Inconsistent.into()),
                },
                Some(ItemRef {
                    key:
                        ObjectKey {
                            object_id,
                            data:
                                ObjectKeyData::Attribute(
                                    AttributeId::FSVERITY_MERKLE,
                                    AttributeKey::Attribute,
                                ),
                        },
                    ..
                }) if *object_id == blob_object.object_id() => {
                    // Blobs should not have the fs-verity attribute. This is explicitly checked
                    // because the fs-verity attribute is ordered between the 2 blob metadata
                    // attributes.  `AttributeId::BLOB_MERKLE` was queried for with the expectation
                    // of finding either blob attribute. Finding the fs-verity attribute could be
                    // hiding the `AttributeId::BLOB_METADATA` attribute.
                    Err(FxfsError::Inconsistent.into())
                }
                // Neither attribute exists.
                _ => Ok(Self::empty()),
            }
        }
    }

    fn write_to<S: HandleOwner>(
        &self,
        blob_object: &DataObjectHandle<S>,
    ) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            // Don't write the attribute when there's no metadata.
            if self.is_empty() {
                return Ok(());
            }
            let mut buf = Vec::new();
            self.serialize_with_version(&mut buf)?;
            blob_object
                .write_attr(AttributeId::BLOB_METADATA, &buf)
                .await
                .context("Failed to write blob metadata attribute.")
        }
    }

    fn serialized_size(&self) -> Result<usize, Error> {
        if self.is_empty() {
            return Ok(0);
        }
        struct CountingWriter(usize);
        impl std::io::Write for CountingWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0 += buf.len();
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = CountingWriter(0);
        self.serialize_with_version(&mut writer)?;
        Ok(writer.0)
    }

    fn into_merkle_verifier(self, root: Hash) -> Result<MerkleVerifier, Error> {
        Ok(BlobMetadataV53::into_merkle_verifier(self, root)?)
    }
}

impl Versioned for BlobMetadataV53 {
    fn max_serialized_size() -> Option<u64> {
        // There's no restriction on the size of the blob metadata.
        None
    }
}

#[derive(Default)]
pub struct BlobMetadataLeafHashCollector(MerkleLeaves);

impl BlobMetadataLeafHashCollector {
    pub fn new() -> Self {
        Self(Vec::new())
    }
}

impl LeafHashCollector for BlobMetadataLeafHashCollector {
    type Output = (Hash, MerkleLeaves);

    fn add_leaf_hash(&mut self, hash: Hash) {
        self.0.push(hash.into())
    }

    fn complete(mut self, root: Hash) -> Self::Output {
        // If the there's only 1 hash then it's the root and doesn't get stored in the metadata.
        if self.0.len() == 1 {
            debug_assert!(*root == self.0[0]);
            self.0 = Vec::new();
        }
        (root, self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::BlobMetadata;
    use crate::blob_metadata::{
        BlobFormat, BlobMetadataLeafHashCollector, BlobMetadataUnversioned, FxfsBlobMetadataExt,
    };
    use crate::filesystem::{FxFilesystem, OpenFxFilesystem};
    use crate::object_store::transaction::{LockKey, Options, lock_keys};
    use crate::object_store::{
        AttributeId, DataObjectHandle, Directory, HandleOptions, ObjectStore,
    };
    use assert_matches::assert_matches;
    use fuchsia_merkle::MerkleRootBuilder;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;

    const TEST_DEVICE_BLOCK_SIZE: u32 = 512;
    const TEST_DEVICE_BLOCK_COUNT: u64 = 16 * 1024;
    const TEST_OBJECT_NAME: &str = "foo";

    async fn test_filesystem() -> OpenFxFilesystem {
        let device =
            DeviceHolder::new(FakeDevice::new(TEST_DEVICE_BLOCK_COUNT, TEST_DEVICE_BLOCK_SIZE));
        FxFilesystem::new_empty(device).await.expect("new_empty failed")
    }

    async fn test_filesystem_and_empty_object() -> (OpenFxFilesystem, DataObjectHandle<ObjectStore>)
    {
        let fs = test_filesystem().await;
        let store = fs.root_store();

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        let object =
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed");

        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        root_directory
            .add_child_file(&mut transaction, TEST_OBJECT_NAME, &object)
            .await
            .expect("add_child_file failed");

        transaction.commit().await.expect("commit failed");

        (fs, object)
    }

    #[fuchsia::test(threads = 3)]
    async fn test_write_read_zstd() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        let metadata = BlobMetadata {
            merkle_leaves: vec![[1; 32], [2; 32], [3; 32], [4; 32]],
            format: BlobFormat::ChunkedZstd {
                uncompressed_size: 128 * 1024,
                chunk_size: 32 * 1024,
                compressed_offsets: vec![0, 100, 200, 400],
            },
        };
        metadata.write_to(&object).await.expect("failed to write attribute");
        let read_metadata =
            BlobMetadata::read_from(&object).await.expect("failed to read attribute");
        assert_eq!(read_metadata, metadata);

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test(threads = 3)]
    async fn test_write_read_lz4() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        let metadata = BlobMetadata {
            merkle_leaves: vec![[1; 32], [2; 32], [3; 32], [4; 32]],
            format: BlobFormat::ChunkedLz4 {
                uncompressed_size: 128 * 1024,
                chunk_size: 32 * 1024,
                compressed_offsets: vec![0, 100, 200, 400],
            },
        };
        metadata.write_to(&object).await.expect("failed to write attribute");
        let read_metadata =
            BlobMetadata::read_from(&object).await.expect("failed to read attribute");
        assert_eq!(read_metadata, metadata);

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test(threads = 3)]
    async fn test_empty_attribute_is_not_written() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        BlobMetadata::empty().write_to(&object).await.expect("failed to write attribute");
        let result = object
            .read_attr(AttributeId::BLOB_METADATA)
            .await
            .expect("reading the attribute failed");
        assert_eq!(result, None);

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test(threads = 3)]
    async fn test_read_corrupt_attribute_fails() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        object
            .write_attr(AttributeId::BLOB_METADATA, b"garbage")
            .await
            .expect("failed to write attribute");
        BlobMetadata::read_from(&object).await.expect_err("reading the metadata should fail");

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test(threads = 3)]
    async fn test_read_unversioned_attribute() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        let unversioned_metadata = BlobMetadataUnversioned {
            hashes: vec![[1; 32], [2; 32]],
            chunk_size: 32 * 1024,
            compressed_offsets: vec![0],
            uncompressed_size: 15 * 1024,
        };
        let mut buf = Vec::new();
        bincode::serialize_into(&mut buf, &unversioned_metadata)
            .expect("failed to serialize metadata");
        object.write_attr(AttributeId::BLOB_MERKLE, &buf).await.expect("failed to write attribute");
        let metadata = BlobMetadata::read_from(&object).await.expect("failed to read attribute");
        assert_eq!(metadata, BlobMetadata::from(unversioned_metadata));

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test(threads = 3)]
    async fn test_read_corrupt_unversioned_attribute_fails() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        object
            .write_attr(AttributeId::BLOB_MERKLE, b"garbage")
            .await
            .expect("failed to write attribute");
        BlobMetadata::read_from(&object).await.expect_err("reading the metadata should fail");

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test(threads = 3)]
    async fn test_fs_verity_hides_blob_metadata() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        let metadata = BlobMetadata {
            merkle_leaves: vec![[1; 32], [2; 32]],
            format: BlobFormat::Uncompressed,
        };
        metadata.write_to(&object).await.expect("failed to write attribute");
        object
            .write_attr(AttributeId::FSVERITY_MERKLE, b"fs-verify")
            .await
            .expect("failed to write fs-verity attribute");
        BlobMetadata::read_from(&object).await.expect_err("fs-verity should have been found");

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn test_serialized_size() {
        assert_matches!(BlobMetadata::empty().serialized_size(), Ok(0));
        assert_matches!(
            BlobMetadata {
                merkle_leaves: vec![[54; 32], [55; 32]],
                format: BlobFormat::Uncompressed,
            }
            .serialized_size(),
            // 4 bytes for the version.
            // 1 byte for the count of merkle leaves.
            // 64 bytes of merkle leaves.
            // 1 byte discriminant for the format.
            Ok(70)
        );
        assert_matches!(
            BlobMetadata {
                merkle_leaves: vec![[54; 32], [55; 32]],
                format: BlobFormat::ChunkedZstd {
                    uncompressed_size: 128 * 1024,
                    chunk_size: 32 * 1024,
                    compressed_offsets: vec![0, 100, 200, 400],
                },
            }
            .serialized_size(),
            // 4 bytes for the version.
            // 1 byte for the count of merkle leaves.
            // 64 bytes of merkle leaves.
            // 1 byte discriminant for the format.
            // 5 bytes for the uncompressed size.
            // 3 bytes for the chunk size.
            // 1 byte for the count of compressed offsets.
            // 6 bytes of compressed offsets.
            Ok(85)
        );
    }

    #[fuchsia::test]
    fn test_leaf_hash_collector_with_only_root() {
        let data = vec![3; 4096];
        let (_root, leaves) =
            MerkleRootBuilder::new(BlobMetadataLeafHashCollector::new()).complete(&data);
        assert!(leaves.is_empty());
    }

    #[fuchsia::test]
    fn test_leaf_hash_collector_with_leaves() {
        let data = vec![3; 12 * 1024];
        let (_root, leaves) =
            MerkleRootBuilder::new(BlobMetadataLeafHashCollector::new()).complete(&data);
        assert_eq!(leaves.len(), 2);
    }

    #[fuchsia::test]
    fn test_into_merkle_verifier_with_only_root() {
        let data = vec![3; 4096];
        let (root, leaves) =
            MerkleRootBuilder::new(BlobMetadataLeafHashCollector::new()).complete(&data);
        let metadata = BlobMetadata { merkle_leaves: leaves, format: BlobFormat::Uncompressed };
        let verifier =
            metadata.into_merkle_verifier(root).expect("failed to create merkle verifier");
        verifier.verify(0, &data).expect("failed to verify data");
    }

    #[fuchsia::test]
    fn test_into_merkle_verifier_with_leaves() {
        let data = vec![3; 12 * 1024];
        let (root, leaves) =
            MerkleRootBuilder::new(BlobMetadataLeafHashCollector::new()).complete(&data);
        let metadata = BlobMetadata { merkle_leaves: leaves, format: BlobFormat::Uncompressed };
        let verifier =
            metadata.into_merkle_verifier(root).expect("failed to create merkle verifier");
        verifier.verify(0, &data).expect("failed to verify data");
    }

    #[fuchsia::test]
    fn test_convert_unversioned_to_versioned() {
        assert_eq!(
            BlobMetadata::from(BlobMetadataUnversioned {
                hashes: vec![[1; 32], [2; 32]],
                chunk_size: 0,
                compressed_offsets: vec![],
                uncompressed_size: 15 * 1024,
            }),
            BlobMetadata {
                merkle_leaves: vec![[1; 32], [2; 32]],
                format: BlobFormat::Uncompressed,
            }
        );

        assert_eq!(
            BlobMetadata::from(BlobMetadataUnversioned {
                hashes: vec![[1; 32], [2; 32], [3; 32], [4; 32]],
                chunk_size: 32 * 1024,
                compressed_offsets: vec![0, 100],
                uncompressed_size: 33 * 1024,
            }),
            BlobMetadata {
                merkle_leaves: vec![[1; 32], [2; 32], [3; 32], [4; 32]],
                format: BlobFormat::ChunkedZstd {
                    uncompressed_size: 33 * 1024,
                    chunk_size: 32 * 1024,
                    compressed_offsets: vec![0, 100]
                },
            }
        );
    }

    #[fuchsia::test]
    fn test_merkle_serialization() {}
}
