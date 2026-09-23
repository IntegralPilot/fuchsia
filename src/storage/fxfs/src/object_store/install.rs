// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Allows installation of an fxfs volume contained by a file handle into its owning volume.
//!
//! *WARNING*: This is still under active development and is not yet ready for production use.
//! Use with caution.

use crate::errors::FxfsError;
use crate::filesystem::FxFilesystemBuilder;
use crate::lsm_tree::persistent_layer::PersistentLayerWriter;
use crate::lsm_tree::skip_list_layer::SkipListLayer;
use crate::lsm_tree::types::{Item, ItemRef, Layer, LayerIterator, LayerWriter as _};
use crate::lsm_tree::{LayerSet, Query, layers_from_handles};
use crate::object_store::extent_mapping_iterator::ExtentMappingIterator;
use crate::object_store::extent_record::{ExtentMode, ExtentValue};
use crate::object_store::object_manager::ReservationUpdate;
use crate::object_store::object_record::{
    AttributeKey, ObjectAttributes, ObjectKey, ObjectKeyData, ObjectKind, ObjectValue,
};
use crate::object_store::transaction::{AssocObj, LockKey, Mutation, Options, lock_keys};
use crate::object_store::tree::reservation_amount_from_layer_size;
use crate::object_store::volume::{RootVolume, root_volume};
use crate::object_store::{
    AttributeId, DataObjectHandle, DirectWriter, Directory, FileExtent, HandleOptions, HandleOwner,
    LastObjectId, LastObjectIdInfo, ObjectHandle, ObjectStore, ReadObjectHandle as _, ReservedId,
    StoreInfo, StoreOptions, merge,
};
use crate::range::RangeExt;
use crate::virtual_device::ReadOnlyDevice;
use anyhow::{Context, Error, bail};
use std::sync::Arc;
use storage_device::DeviceHolder;

impl ObjectStore {
    /// Installs `src`, replacing `dst` if it exists. `src` must contain a file named `image_file`,
    /// which must be a file containing an fxfs filesystem. The filesystem should contain a volume
    /// named `dst`, the contents of which will replace the existing `dst` volume. Installation
    /// takes place across several transactions to ensure that the filesystem remains consistent,
    /// and that no volumes are modified until they are atomically swapped. Neither `src` nor `dst`
    /// can be encrypted.
    ///
    /// *NOTE*: It is the responsibility of the caller to ensure that neither `src` nor `dest` are
    /// mounted or otherwise in use during this process.
    pub(super) async fn install_volume(
        root: &RootVolume,
        src: &str,
        image_file: &str,
        dst: &str,
    ) -> Result<(), Error> {
        // If we're going to replace an existing volume, make sure it isn't encrypted.
        if root.volume_directory().lookup(dst).await?.is_some() {
            let dst_vol = root.volume(dst, StoreOptions::default()).await?;
            if dst_vol.is_encrypted() {
                return Err(FxfsError::AccessDenied)
                    .context("cannot install volume: dst volume is encrypted");
            }
        }

        let src_vol =
            root.volume(src, StoreOptions::default()).await.context("could not open src volume")?;
        if src_vol.is_encrypted() {
            return Err(FxfsError::AccessDenied)
                .context("cannot install volume: src volume is encrypted");
        }

        let image_handle = {
            let src_root_dir =
                Directory::open(&src_vol, src_vol.root_directory_object_id()).await?;
            let (object_id, _, _) = src_root_dir
                .lookup(image_file)
                .await?
                .ok_or(FxfsError::NotFound)
                .context("could not find image file in pending volume")?;
            ObjectStore::open_object(&src_vol, object_id, Default::default(), None).await?
        };

        // *NOTE*: We must ensure that no other objects in the pending volume have extent records,
        // otherwise installation process will leave the filesystem with orphaned allocations!
        // To guard against this, we ensure only the object ID from `handle` has any extent records.
        if !item_is_unique(src_vol.as_ref(), &image_handle).await? {
            return Err(FxfsError::Internal)
                .context("Cannot install volume: outer volume contains extra extent records");
        }

        // Extract the extents from the backing handle, and then mount the inner filesystem.
        let extents = image_handle.device_extents().await?;
        // TODO(https://fxbug.dev/415300916): We are currently assuming the block size of the image
        // matches the filesystem as we have no way of verifying this. We should add a field to the
        // superblock to guard against this, and also prevent mounting an fxfs partition on a device
        // with the wrong block size.
        let device = DeviceHolder::new(ReadOnlyDevice::new(image_handle)?);
        let inner_fs = FxFilesystemBuilder::new().read_only(true).open(device).await?;

        {
            let inner_root = root_volume(inner_fs.clone()).await?;
            let inner_volume = inner_root
                .volume(dst, StoreOptions::default())
                .await
                .context("could not open target volume in mounted image")?;
            if inner_volume.is_encrypted() {
                return Err(FxfsError::AccessDenied)
                    .context("cannot install volume: volume in image encrypted");
            }
            src_vol.install_volume_impl(root, src, dst, inner_volume, extents).await?;
        }

        // Ensure we have no remaining open handles to the inner device before returning.
        let _ = inner_fs.take_device().await;
        Ok(())
    }

    /// Installs the contents of `inner_store` into this [`ObjectStore`]. On success, the volume
    /// will be renamed to `dst`, replacing an existing volume with the same name if one
    /// exists. `extents` are used to remap extent records so data does not need to be copied.
    ///
    ///   1. We create a new object in-memory that references the portions of the inner filesystem
    ///      that we wish to discard when the volume has been installed.
    ///   2. We create a new object in the parent store holding the desired object tree layer once
    ///      the volume is installed. This includes the objects currently inside the inner volume
    ///      as well as the additional file that owns the metadata allocations created in step 1.
    ///   3. We write out the new object tree layer, but remapping the extents of the inner
    ///      filesystem to match those of the storage backing it.
    ///   4. Update the on-disk and in-memory state of the object store to be consistent with the
    ///      new object tree layer, and swap volumes atomically.
    ///   5. Delete (tombstone) the unused metadata extents and the old layer files/deleted volume.
    async fn install_volume_impl(
        &self,
        root: &RootVolume,
        src: &str,
        dst: &str,
        inner_volume: Arc<ObjectStore>,
        extents: Vec<FileExtent>,
    ) -> Result<(), Error> {
        let fs = self.filesystem();
        let keys = lock_keys![LockKey::flush(self.store_object_id())];
        let _guard = Some(fs.lock_manager().write_lock(keys).await);

        // Step 1: Create a new object that owns the portions of the backing file that we want to
        // discard once the volume has been installed.
        let metadata_object_reserved_id = inner_volume.maybe_get_next_object_id().unwrap();
        let mut inner_layer_set = inner_volume.tree().layer_set();
        let metadata_object_id = metadata_object_reserved_id.get();
        let layer_with_metadata = create_metadata_ownership_layer(
            &inner_layer_set,
            metadata_object_reserved_id,
            &extents,
        )
        .await?;
        // Add the file to the graveyard so it's cleaned up if we crash.
        layer_with_metadata.insert(Item {
            key: ObjectKey::graveyard_entry(
                inner_volume.graveyard_directory_object_id(),
                metadata_object_id,
            ),
            value: ObjectValue::Some,
        })?;
        let locked = (layer_with_metadata as Arc<dyn Layer<_, _>>).into();
        inner_layer_set.layers.push(locked);

        let object_manager = fs.object_manager();
        let reservation = object_manager.metadata_reservation();
        let txn_options = Options {
            skip_journal_checks: true,
            borrow_metadata_space: true,
            allocator_reservation: Some(reservation),
            ..Default::default()
        };

        let mut transaction = self.new_transaction(lock_keys![], txn_options).await?;
        transaction.add(self.store_object_id(), Mutation::BeginFlush);
        transaction.commit().await?;

        // Step 2: Create an object for the new layer we will create for the installed volume.
        // For this, we need two transactions: one to create the new layer (graveyarded so it's
        // cleaned up if we crash), and another that atomically swaps in the new layer and
        // graveyards the old ones.
        let mut create_new_layer_txn = self.new_transaction(lock_keys![], txn_options).await?;
        let reservation_update: ReservationUpdate; // Must outlive `activate_volume_txn`.
        let parent_store = self.parent_store.as_ref().unwrap();

        let mut activate_volume_txn = root
            .acquire_transaction_for_remove_volume(
                dst,
                [LockKey::object(
                    parent_store.store_object_id(),
                    self.store_info_handle_object_id().unwrap(),
                )],
                true,
            )
            .await?
            .1;

        let new_layer = {
            let handle_options = HandleOptions { skip_journal_checks: true, ..Default::default() };
            let new_layer = ObjectStore::create_object(
                parent_store,
                &mut create_new_layer_txn,
                handle_options,
                None,
            )
            .await?;
            parent_store.add_to_graveyard(&mut create_new_layer_txn, new_layer.object_id());
            parent_store.remove_from_graveyard(&mut activate_volume_txn, new_layer.object_id());
            create_new_layer_txn.commit().await?;
            new_layer
        };

        // Step 3: Write out the new object tree layer but with remapped extent records.
        {
            let start_time = std::time::Instant::now();
            let writer = DirectWriter::new(&new_layer, txn_options).await;
            // TODO(https://fxbug.dev/415300916): Our extent mapping iterator may emit more items
            // than are in the current layer set so `num_items` will likely be too small here. We
            // should calculate the correct amount ahead of time before emitting the object records.
            let num_items = inner_layer_set.sum_len();
            let mut writer = PersistentLayerWriter::new(writer, num_items, fs.block_size()).await?;
            let mut merger = inner_layer_set.merger();
            let mut iter =
                ExtentMappingIterator::new(merger.query(Query::FullScan).await?, extents)?;
            while let Some(item_ref) = iter.get() {
                writer.write(item_ref).await?;
                iter.advance().await?;
            }
            let bytes_written = writer.complete().await?;
            self.tree.report_compaction_metrics(
                bytes_written,
                start_time.elapsed(),
                inner_layer_set.layers.len(),
            );
        }

        // Move the old layers to the graveyard at the end.
        let old_layers = self.tree.immutable_layer_set().layers;
        for layer in &old_layers {
            if let Some(handle) = layer.handle() {
                parent_store.add_to_graveyard(&mut activate_volume_txn, handle.object_id());
            }
        }

        let total_layer_size = new_layer.get_size();
        let new_layer_object_id = new_layer.object_id();
        let new_layers = layers_from_handles([new_layer]).await?;

        let old_store_info = inner_volume.store_info().unwrap();
        let new_store_info = StoreInfo {
            layers: vec![new_layer_object_id],
            last_object_id: LastObjectIdInfo::Unencrypted { id: metadata_object_id },
            object_count: old_store_info.object_count + 1, // Update for the metadata file we added
            ..old_store_info
        };

        // Step 4: Update the on-disk and in-memory state of the object store to be consistent with
        // the new object tree layer, and swap volumes atomically.
        self.write_store_info(&mut activate_volume_txn, &new_store_info).await?;
        let replaced_objects = root.replace_volume(&mut activate_volume_txn, src, dst).await?;
        reservation_update =
            ReservationUpdate::new(reservation_amount_from_layer_size(total_layer_size));
        activate_volume_txn.add_with_object(
            self.store_object_id(),
            Mutation::EndFlush,
            AssocObj::Borrowed(&reservation_update),
        );
        activate_volume_txn
            .commit_with_callback(|_| {
                *self.store_info.lock().as_mut().unwrap() = new_store_info;
                match &mut *self.last_object_id.lock() {
                    LastObjectId::Unencrypted { id } => *id = metadata_object_id,
                    _ => unreachable!(),
                }
                self.tree.set_layers(new_layers);
            })
            .await?;

        // Step 5: Delete (tombstone) everything we don't need to keep around anymore. Similar to
        // the logic we use when deleting volumes or flushing object stores however, we ensure
        // everything is purged before returning so that there are no side effects of the install
        // process remaining in the graveyard. If we lose power at this point, this will be done the
        // next time the filesystem is mounted.

        // Close and delete the old layers from the source volume pre-installation.
        for layer in old_layers {
            let object_id = layer.handle().map(|h| h.object_id());
            layer.close_layer().await;
            if let Some(object_id) = object_id {
                parent_store.tombstone_object(object_id, txn_options, None).await?;
            }
        }
        // Delete objects from the destination volume we replaced, if any.
        if let Some(replaced_objects) = replaced_objects {
            for object_id in replaced_objects {
                fs.root_store().tombstone_object(object_id, Options::default(), None).await?;
            }
        }
        // Delete the metadata ownership file.
        self.tombstone_object(metadata_object_id, txn_options, None).await?;

        Ok(())
    }
}

/// Creates an in-memory layer containing a single file which owns the extents that are NOT
/// referenced by `inner_layer_set`. If this file is deleted, the extent records referenced by
/// `inner_layer_set` will still exist on disk. However, the remaining allocations for the volume,
/// including the current layers for `inner_layer_set`, will be deleted. This is only safe to do so
/// after the new volume has been installed (e.g. the object records have been remapped).
async fn create_metadata_ownership_layer(
    inner_layer_set: &LayerSet<ObjectKey, ObjectValue>,
    object_id: ReservedId<'_>,
    extents: &[FileExtent],
) -> Result<Arc<SkipListLayer<ObjectKey, ObjectValue>>, Error> {
    if extents.is_empty() {
        return Err(FxfsError::Inconsistent).context("store has no backing extents");
    }
    // Create an in in-memory layer containing a file to be deleted that contains all the other
    // extents we don't need.
    // TODO(https://fxbug.dev/415300916): Calculate the exact amount of items we need.
    let layer = SkipListLayer::new(1000);

    // NOTE: Backing extents are assumed to be sorted by logical offset, so the last extent's end
    // offset should be equal to the size of the backing file.
    let mut size = extents.last().unwrap().logical_range().end;
    layer.insert(Item {
        key: ObjectKey::extent(object_id.get(), AttributeId::DATA, 0..size),
        value: ObjectValue::Extent(ExtentValue::Some {
            device_offset: 0, // Will be remapped below
            mode: ExtentMode::Raw,
            key_id: 0,
        }),
    })?;

    // Iterate over all the extent records in the layer set, and punch holes in the new file we
    // created for each record.
    {
        let mut merger = inner_layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        while let Some(item_ref) = iter.get() {
            if let ObjectValue::Extent(ExtentValue::Some { device_offset, .. }) = item_ref.value {
                // NOTE: `device_offset` refers to the logical offset within the backing file.
                // This is equivalent to the *logical* offset within `extents`. Thus we need to use
                // the device range of the extent records as the logical range we want to remove.
                let ObjectKey {
                    data: ObjectKeyData::Attribute(_, AttributeKey::Extent(extent)),
                    ..
                } = item_ref.key
                else {
                    bail!(FxfsError::Inconsistent);
                };
                let len = extent.length()?;
                let item = Item {
                    key: ObjectKey::extent(
                        object_id.get(),
                        AttributeId::DATA,
                        *device_offset..*device_offset + len,
                    ),
                    value: ObjectValue::Extent(ExtentValue::None),
                };
                size = size
                    .checked_sub(len)
                    .ok_or(FxfsError::Inconsistent)
                    .context("blob extents are larger than written image")?;
                let lower_bound = item.key.key_for_merge_into();
                layer.merge_into(item, &lower_bound, merge::merge);
            }
            iter.advance().await?;
        }
    }

    // Add the extra object records we need for it to be a valid file.

    // The transaction takes ownership of the ID.
    let object_id = object_id.release().get();
    layer.insert(Item {
        key: ObjectKey::object(object_id),
        value: ObjectValue::Object {
            kind: ObjectKind::File { refs: 1 },
            attributes: ObjectAttributes { allocated_size: size, ..Default::default() },
        },
    })?;

    layer.insert(Item {
        key: ObjectKey::attribute(object_id, AttributeId::DATA, AttributeKey::Attribute),
        value: ObjectValue::Attribute { size, has_overwrite_extents: false },
    })?;

    Ok(layer)
}

/// Returns true if all extent records within `owner` are those referenced by `item`.
async fn item_is_unique<H: HandleOwner>(
    owner: &H,
    handle: &DataObjectHandle<H>,
) -> Result<bool, Error> {
    let layer_set = owner.as_ref().tree().layer_set();
    let mut merger = layer_set.merger();
    let mut iter = merger.query(Query::FullScan).await?;
    while let Some(item) = iter.get() {
        if let ItemRef {
            key:
                ObjectKey {
                    object_id,
                    data: ObjectKeyData::Attribute(attribute_id, AttributeKey::Extent(_)),
                },
            value: ObjectValue::Extent(_),
        } = item
        {
            if *object_id != handle.object_id() || *attribute_id != handle.attribute_id() {
                return Ok(false);
            }
        }
        iter.advance().await?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::filesystem::{FxFilesystem, SyncOptions};
    use crate::fsck::{FsckOptions, fsck_with_options};
    use crate::object_handle::WriteObjectHandle as _;
    use crate::object_store::NewChildStoreOptions;
    use crate::object_store::directory::Directory;
    use fxfs_crypto::Crypt;
    use fxfs_insecure_crypto::new_insecure_crypt;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;

    const DST_NAME: &str = "vol";
    const SRC_NAME: &str = "vol-pending";
    const INSTALL_FILE: &str = "install";

    /// Creates an in-memory fxfs filesystem with the given contents.
    async fn create_test_filesystem(
        block_count: u64,
        block_size: u32,
        volume_name: &str,
        files: &[(&str, &str)],
    ) -> DeviceHolder {
        let device = DeviceHolder::new(FakeDevice::new(block_count, block_size));
        let fs = FxFilesystem::new_empty(device).await.unwrap();
        {
            let root = root_volume(fs.clone()).await.unwrap();
            let volume =
                root.new_volume(volume_name, NewChildStoreOptions::default()).await.unwrap();
            let inner_dir =
                Directory::open(&volume, volume.root_directory_object_id()).await.unwrap();
            // Let's create some files that contain some data we'll read back.
            let mut handles = vec![];
            let keys = lock_keys![LockKey::object(volume.store_object_id(), inner_dir.object_id())];
            let mut transaction = volume
                .filesystem()
                .root_store()
                .new_transaction(keys, Default::default())
                .await
                .unwrap();
            for (name, contents) in files.iter() {
                let handle = inner_dir.create_child_file(&mut transaction, *name).await.unwrap();
                handles.push((handle, *contents));
            }
            transaction.commit().await.unwrap();
            {
                let device = fs.device();
                for (handle, contents) in handles {
                    let mut buf = device.allocate_buffer(contents.len()).await;
                    buf.copy_from_slice(contents.as_bytes());
                    handle.write_or_append(None, buf.as_ref()).await.unwrap();
                }
            }
        }
        fs.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
        fs.close().await.unwrap();
        let device = fs.take_device().await;
        device.reopen(/*read_only*/ false);
        device
    }

    async fn write_image_to_file(
        volume: &Arc<ObjectStore>,
        image: &DeviceHolder,
        install_file: &str,
    ) {
        let dir = Directory::open(volume, volume.root_directory_object_id()).await.unwrap();
        let handle = {
            let keys: crate::object_store::transaction::LockKeys =
                lock_keys![LockKey::object(volume.store_object_id(), dir.object_id())];
            let mut transaction = volume
                .filesystem()
                .root_store()
                .new_transaction(keys, Default::default())
                .await
                .unwrap();
            let handle = dir.create_child_file(&mut transaction, install_file).await.unwrap();
            transaction.commit().await.unwrap();
            handle
        };

        const CHUNK_READ_SIZE: usize = 131_072; /* 128 KiB */
        let mut inner_buff = image.allocate_buffer(CHUNK_READ_SIZE).await;
        let mut outer_buff = handle.allocate_buffer(CHUNK_READ_SIZE).await;
        let total = image.size();
        let mut offset = 0;
        while offset < total {
            let amount = std::cmp::min(total - offset, CHUNK_READ_SIZE as u64);
            image.read(offset, inner_buff.as_mut()).await.unwrap();
            outer_buff.copy_from_buffer(inner_buff.as_ref());
            handle
                .write_or_append(Some(offset), outer_buff.subslice(0..amount as usize))
                .await
                .unwrap();
            offset += amount;
        }
        assert_eq!(offset, total);
        handle.flush().await.unwrap();
    }

    async fn verify_volume_contents(root: &RootVolume, volume_name: &str, files: &[(&str, &str)]) {
        let volume = root.volume(volume_name, StoreOptions::default()).await.unwrap();
        let dir = Directory::open(&volume, volume.root_directory_object_id()).await.unwrap();

        for (name, expected_contents) in files.iter() {
            let (object_id, _, _) =
                dir.lookup(name).await.expect("lookup failed").expect("missing expected file");
            let file = ObjectStore::open_object(&volume, object_id, Default::default(), None)
                .await
                .unwrap();
            let contents = file.contents(usize::MAX).await.unwrap();
            assert_eq!(contents.as_ref(), expected_contents.as_bytes());
        }
    }

    async fn ensure_volume_graveyard_is_empty(root: &RootVolume, volume_name: &str) {
        let volume = root.volume(volume_name, StoreOptions::default()).await.unwrap();
        let layer_set = volume.tree().layer_set();
        let mut merger = layer_set.merger();
        let iter = crate::object_store::Graveyard::iter(
            volume.graveyard_directory_object_id(),
            &mut merger,
        )
        .await
        .unwrap();
        assert!(iter.get().is_none(), "graveyard was not empty!");
    }

    async fn do_fsck(fs: &Arc<FxFilesystem>, volume_name: &str) {
        let fsck_options = FsckOptions {
            fail_on_warning: true,
            on_error: Box::new(|err| eprintln!("fsck error: {:?}", err)),
            ..Default::default()
        };
        fsck_with_options(fs.clone(), &fsck_options).await.expect("fsck filesystem");
        let root = root_volume(fs.clone()).await.unwrap();
        let vol = root.volume(volume_name, StoreOptions::default()).await.expect("missing volume");
        crate::fsck::fsck_volume_with_options(&fs, &fsck_options, vol.store_object_id(), None)
            .await
            .expect("fsck volume");
    }

    /// Test volume installation into an existing but otherwise empty filesystem.
    #[fuchsia::test]
    async fn test_install_volume() {
        let files = [
            ("file_1", "Hello, world!"),
            ("file_2", "Goodbye, stranger!"),
            ("file_3", "Its been nice..."),
        ];
        let image = create_test_filesystem(512, 4096, DST_NAME, &files).await;

        // Create a new empty filesystem, write our image, and install the inner volume.
        let fs =
            FxFilesystem::new_empty(DeviceHolder::new(FakeDevice::new(1024, 4096))).await.unwrap();
        {
            // Write the image into a file in a new volume.
            let root = root_volume(fs.clone()).await.unwrap();
            let src = root.new_volume(SRC_NAME, NewChildStoreOptions::default()).await.unwrap();
            write_image_to_file(&src, &image, INSTALL_FILE).await;
            // Install the pending volume, after which it should be gone and we should be able to
            // access the files we expect.
            ObjectStore::install_volume(&root, SRC_NAME, INSTALL_FILE, DST_NAME).await.unwrap();
            assert!(root.volume_directory().lookup(SRC_NAME).await.unwrap().is_none());
            verify_volume_contents(&root, DST_NAME, &files).await;
            ensure_volume_graveyard_is_empty(&root, DST_NAME).await;
            do_fsck(&fs, DST_NAME).await;
        };

        // Close and re-fsck the filesystem.
        fs.close().await.unwrap();
        let device = fs.take_device().await;
        device.reopen(/*read_only*/ true);
        let fs = FxFilesystem::open(device).await.unwrap();
        do_fsck(&fs, DST_NAME).await;
    }

    /// Test volume installation gracefully replaces an existing volume.
    #[fuchsia::test]
    async fn test_install_volume_replaces_existing() {
        let existing_files = [
            ("file_1", "Hello, world!"),
            ("file_2", "Goodbye, stranger!"),
            ("file_3", "Its been nice..."),
        ];
        let fs =
            FxFilesystem::open(create_test_filesystem(1024, 4096, DST_NAME, &existing_files).await)
                .await
                .unwrap();

        let new_files = [
            ("file_4", "So goodbye Mary"),
            ("file_5", "So goodbye Jane"),
            ("file_6", "Will we ever meet again?"),
        ];
        let image = create_test_filesystem(512, 4096, DST_NAME, &new_files).await;

        {
            let root = root_volume(fs.clone()).await.unwrap();
            let src = root.new_volume(SRC_NAME, NewChildStoreOptions::default()).await.unwrap();
            write_image_to_file(&src, &image, INSTALL_FILE).await;
            // We should see a different set of files before/after installation.
            verify_volume_contents(&root, DST_NAME, &existing_files).await;
            ObjectStore::install_volume(&root, SRC_NAME, INSTALL_FILE, DST_NAME).await.unwrap();
            assert!(root.volume_directory().lookup(SRC_NAME).await.unwrap().is_none());
            verify_volume_contents(&root, DST_NAME, &new_files).await;
            ensure_volume_graveyard_is_empty(&root, DST_NAME).await;
        };

        do_fsck(&fs, DST_NAME).await;
    }

    /// Ensure that we don't allow installing a volume if the pending volume has additional extent
    /// records. These extra records would be orphaned by the installation process otherwise,
    /// leaving the filesystem corrupted.
    #[fuchsia::test]
    async fn test_install_volume_requires_no_additional_extents() {
        let files = [
            ("file_1", "Hello, world!"),
            ("file_2", "Goodbye, stranger!"),
            ("file_3", "Its been nice..."),
        ];
        let image = create_test_filesystem(512, 4096, DST_NAME, &[]).await;

        // Initialize a filesystem that already contains some files in our pending install volume.
        let fs = FxFilesystem::open(create_test_filesystem(1024, 4096, SRC_NAME, &files).await)
            .await
            .unwrap();

        {
            // Write the image into a file in a new volume.
            let root = root_volume(fs.clone()).await.unwrap();
            let src = root.volume(SRC_NAME, StoreOptions::default()).await.unwrap();
            write_image_to_file(&src, &image, INSTALL_FILE).await;
            // Installation should fail due to the presence of other extent records.
            let err = ObjectStore::install_volume(&root, SRC_NAME, INSTALL_FILE, DST_NAME)
                .await
                .unwrap_err();
            assert_eq!(err.downcast::<FxfsError>().unwrap(), FxfsError::Internal);
            // The pending volume should still exist and the contents should remain untouched, and
            // there should be no installed volume.
            assert!(root.volume_directory().lookup(SRC_NAME).await.unwrap().is_some());
            verify_volume_contents(&root, SRC_NAME, &files).await;
            assert!(root.volume_directory().lookup(DST_NAME).await.unwrap().is_none());
        };

        do_fsck(&fs, SRC_NAME).await;
    }

    /// Test volume installation while running compaction, with both the pending and destination
    /// volumes having additional contents.
    #[fuchsia::test]
    async fn test_install_volume_during_compaction() {
        let files = [
            ("file_1", "Hello, world!"),
            ("file_2", "Goodbye, stranger!"),
            ("file_3", "Its been nice..."),
        ];
        let image = create_test_filesystem(512, 4096, DST_NAME, &files).await;

        let fs = FxFilesystem::open(create_test_filesystem(2048, 4096, DST_NAME, &[]).await)
            .await
            .unwrap();
        {
            let root = root_volume(fs.clone()).await.unwrap();
            let src_volume =
                root.new_volume(SRC_NAME, NewChildStoreOptions::default()).await.unwrap();
            let dst_volume = root.volume(DST_NAME, StoreOptions::default()).await.unwrap();

            // Write the image into a file in the pending volume.
            write_image_to_file(&src_volume, &image, INSTALL_FILE).await;

            // To ensure compactions have work to do, create additional layers in both the pending
            // and destination volumes.
            let pending_dir =
                Directory::open(&src_volume, src_volume.root_directory_object_id()).await.unwrap();
            let dst_dir =
                Directory::open(&dst_volume, dst_volume.root_directory_object_id()).await.unwrap();
            for i in 0..10 {
                let keys = lock_keys![
                    LockKey::object(src_volume.store_object_id(), pending_dir.object_id()),
                    LockKey::object(dst_volume.store_object_id(), dst_dir.object_id())
                ];
                let mut transaction =
                    fs.root_store().new_transaction(keys, Default::default()).await.unwrap();
                pending_dir.create_child_file(&mut transaction, &format!("{}", i)).await.unwrap();
                dst_dir.create_child_file(&mut transaction, &format!("{}", i)).await.unwrap();
                transaction.commit().await.unwrap();
                fs.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
            }

            // Run installation and compaction concurrently.

            let fs_clone = fs.clone();
            let fs_clone2 = fs.clone();
            futures::join!(
                async move {
                    let root = root_volume(fs_clone).await.unwrap();
                    ObjectStore::install_volume(&root, SRC_NAME, INSTALL_FILE, DST_NAME)
                        .await
                        .unwrap();
                },
                async move {
                    fs_clone2.journal().force_compact().await.unwrap();
                },
            );

            assert!(root.volume_directory().lookup(SRC_NAME).await.unwrap().is_none());
            verify_volume_contents(&root, DST_NAME, &files).await;
        }
        do_fsck(&fs, DST_NAME).await;
    }

    /// Test installing a volume repeatedly while also running compaction repeatedly.
    #[fuchsia::test]
    async fn test_install_volume_during_compaction_loop() {
        async fn install_volume(fs: Arc<FxFilesystem>) {
            let files = [
                ("file_1", "Hello, world!"),
                ("file_2", "Goodbye, stranger!"),
                ("file_3", "Its been nice..."),
            ];
            let image = create_test_filesystem(512, 4096, DST_NAME, &files).await;
            let root = root_volume(fs).await.unwrap();
            let src_volume =
                root.new_volume(SRC_NAME, NewChildStoreOptions::default()).await.unwrap();
            write_image_to_file(&src_volume, &image, INSTALL_FILE).await;
            ObjectStore::install_volume(&root, SRC_NAME, INSTALL_FILE, DST_NAME).await.unwrap();
            verify_volume_contents(&root, DST_NAME, &files).await;
        }

        // Create a new empty filesystem.
        let fs =
            FxFilesystem::new_empty(DeviceHolder::new(FakeDevice::new(2048, 4096))).await.unwrap();

        // Run installation and compaction concurrently.
        let fs_clone = fs.clone();
        let fs_clone2 = fs.clone();
        let done = Arc::new(AtomicBool::new(false));
        let done_clone = done.clone();
        futures::join!(
            async move {
                for _ in 0..10 {
                    install_volume(fs_clone.clone()).await;
                }
                done.store(true, Ordering::Relaxed);
            },
            async move {
                while !done_clone.load(Ordering::Relaxed) {
                    fs_clone2.journal().force_compact().await.unwrap();
                }
            },
        );

        do_fsck(&fs, DST_NAME).await;
    }

    /// Volume installation should fail if either the `src` volume or `image_file` is missing.
    #[fuchsia::test]
    async fn test_install_volume_fails_if_sources_missing() {
        let fs =
            FxFilesystem::new_empty(DeviceHolder::new(FakeDevice::new(1024, 4096))).await.unwrap();
        {
            let root = root_volume(fs.clone()).await.unwrap();
            let err = ObjectStore::install_volume(&root, SRC_NAME, INSTALL_FILE, DST_NAME)
                .await
                .expect_err("install_volume should fail if src volume is missing");
            assert_eq!(err.downcast::<FxfsError>().unwrap(), FxfsError::NotFound);
            let _src_volume =
                root.new_volume(SRC_NAME, NewChildStoreOptions::default()).await.unwrap();
            let err = ObjectStore::install_volume(&root, SRC_NAME, INSTALL_FILE, DST_NAME)
                .await
                .expect_err("install_volume should fail if src install file missing");
            assert_eq!(err.downcast::<FxfsError>().unwrap(), FxfsError::NotFound);
        };

        fs.close().await.unwrap();
    }

    /// Volume installation should fail if `src` is encrypted.
    #[fuchsia::test]
    async fn test_install_volume_requires_unencrypted_src_volume() {
        let fs =
            FxFilesystem::new_empty(DeviceHolder::new(FakeDevice::new(1024, 4096))).await.unwrap();
        {
            let crypt = Arc::new(new_insecure_crypt());
            let root = root_volume(fs.clone()).await.unwrap();
            // Write the image into an encrypted source volume.
            let image = create_test_filesystem(512, 4096, DST_NAME, &[]).await;
            let src_volume = root
                .new_volume(
                    SRC_NAME,
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            write_image_to_file(&src_volume, &image, INSTALL_FILE).await;
            let err = ObjectStore::install_volume(&root, SRC_NAME, INSTALL_FILE, DST_NAME)
                .await
                .expect_err("install_volume should fail if src volume is encrypted");
            assert_eq!(err.downcast::<FxfsError>().unwrap(), FxfsError::AccessDenied);
        };

        fs.close().await.unwrap();
    }

    /// Volume installation should fail if `dst` is encrypted.
    #[fuchsia::test]
    async fn test_install_volume_requires_unencrypted_dst_volume() {
        let fs =
            FxFilesystem::new_empty(DeviceHolder::new(FakeDevice::new(1024, 4096))).await.unwrap();
        {
            let crypt = Arc::new(new_insecure_crypt());
            let root = root_volume(fs.clone()).await.unwrap();
            // Create an encrypted destination volume.
            let _dst_volume = root
                .new_volume(
                    DST_NAME,
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            // Write the image to an unencrypted source volume.
            let image = create_test_filesystem(512, 4096, DST_NAME, &[]).await;
            let src_volume =
                root.new_volume(SRC_NAME, NewChildStoreOptions::default()).await.unwrap();
            write_image_to_file(&src_volume, &image, INSTALL_FILE).await;
            let err = ObjectStore::install_volume(&root, SRC_NAME, INSTALL_FILE, DST_NAME)
                .await
                .expect_err("install_volume should fail if existing dst volume is encrypted");
            assert_eq!(err.downcast::<FxfsError>().unwrap(), FxfsError::AccessDenied);
        };

        fs.close().await.unwrap();
    }
}
