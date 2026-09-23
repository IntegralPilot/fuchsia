// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::filesystem::{FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem, SyncOptions};
use crate::fsck::errors::{FsckError, FsckFatal, FsckIssue, FsckWarning};
use crate::fsck::{FsckOptions, fsck_volume_with_options, fsck_with_options};
use crate::lsm_tree::Query;
use crate::lsm_tree::persistent_layer::PersistentLayerWriter;
use crate::lsm_tree::types::{Item, ItemRef, Key, LayerIterator, LayerWriter, Value};
use crate::object_handle::{INVALID_OBJECT_ID, ObjectHandle, ReadObjectHandle, WriteObjectHandle};
use crate::object_store::allocator::{AllocatorKey, AllocatorValue, CoalescingIterator};
use crate::object_store::data_object_handle::OverwriteOptions;
use crate::object_store::directory::{
    self, Directory, MutableAttributesInternal, encrypt_filename,
};
use crate::object_store::extent::MIN_BLOCK_SIZE;
use crate::object_store::transaction::{self, LockKey, ObjectStoreMutation, Options, lock_keys};
use crate::object_store::volume::root_volume;
use crate::object_store::{
    AttributeId, AttributeKey, ChildValue, DirType, EncryptionKeys, ExtentMode, ExtentValue,
    FsverityMetadata, HandleOptions, Mutation, NewChildStoreOptions, ObjectAttributes,
    ObjectDescriptor, ObjectKey, ObjectKeyData, ObjectKind, ObjectStore, ObjectValue, ProjectId,
    RootDigest, StoreInfo, StoreOptions, Timestamp, VOLUME_DATA_KEY_ID,
};
use crate::serialized_types::VersionedLatest;
use crate::testing::writer::Writer;
use anyhow::{Context, Error};
use assert_matches::assert_matches;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::join;
use fxfs_crypt_common::CryptBase;
use fxfs_crypto::{Crypt, EncryptionKey, KeyPurpose, WrappingKeyId};
use fxfs_insecure_crypto::new_insecure_crypt;
use mundane::hash::{Digest, Hasher, Sha256};
use std::ops::Deref;
use std::sync::Arc;
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;
use storage_units::BlockSize;
use test_case::test_case;

const TEST_DEVICE_BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
const TEST_DEVICE_BLOCK_COUNT: u64 = 8192;
const WRAPPING_KEY_ID: WrappingKeyId = u128::to_le_bytes(2);

struct FsckTest {
    filesystem: Option<OpenFxFilesystem>,
    errors: Mutex<Vec<FsckIssue>>,
    crypt: Option<Arc<CryptBase>>,
}

#[derive(Default)]
struct TestOptions {
    halt_on_error: bool,
    skip_system_fsck: bool,
    volume_store_id: Option<u64>,
}

impl FsckTest {
    async fn new() -> Self {
        let filesystem = FxFilesystem::new_empty(DeviceHolder::new(FakeDevice::new(
            TEST_DEVICE_BLOCK_COUNT,
            TEST_DEVICE_BLOCK_SIZE.get() as u32,
        )))
        .await
        .expect("new_empty failed");

        Self { filesystem: Some(filesystem), errors: Mutex::new(vec![]), crypt: None }
    }
    async fn remount(&mut self) -> Result<(), Error> {
        let fs = self.filesystem.take().unwrap();
        fs.close().await.expect("Failed to close FS");
        let device = fs.take_device().await;
        device.reopen(true);
        self.filesystem = Some(
            FxFilesystemBuilder::new()
                .read_only(true)
                .open(device)
                .await
                .context("Failed to open FS")?,
        );
        Ok(())
    }
    async fn remount_rw(&mut self) -> Result<(), Error> {
        let fs = self.filesystem.take().unwrap();
        fs.close().await.expect("Failed to close FS");
        let device = fs.take_device().await;
        device.reopen(true);
        self.filesystem = Some(
            FxFilesystemBuilder::new()
                .read_only(false)
                .open(device)
                .await
                .context("Failed to open FS")?,
        );
        Ok(())
    }
    async fn run(&self, test_options: TestOptions) -> Result<(), Error> {
        let options = FsckOptions {
            fail_on_warning: true,
            halt_on_error: test_options.halt_on_error,
            on_error: Box::new(|err| {
                if err.is_error() {
                    eprintln!("Fsck error: {:?}", &err);
                } else {
                    println!("Fsck warning: {:?}", &err);
                }
                self.errors.lock().push(err.clone());
            }),
            ..Default::default()
        };
        if !test_options.skip_system_fsck {
            fsck_with_options(self.filesystem(), &options).await?;
        }
        if let Some(store_id) = test_options.volume_store_id {
            fsck_volume_with_options(
                self.filesystem().as_ref(),
                &options,
                store_id,
                self.crypt.clone().map(|x| x as Arc<dyn Crypt>),
            )
            .await?;
        }
        Ok(())
    }
    fn filesystem(&self) -> Arc<FxFilesystem> {
        self.filesystem.as_ref().unwrap().deref().clone()
    }
    fn errors(&self) -> Vec<FsckIssue> {
        self.errors.lock().clone()
    }
    fn get_crypt(&mut self) -> Arc<CryptBase> {
        self.crypt.get_or_insert_with(|| Arc::new(new_insecure_crypt())).clone()
    }
}

// Creates a new layer file containing |items| and writes them in order into |store|, skipping all
// normal validation.  This allows bad records to be inserted into the object store (although they
// will still be subject to merging).
// Doing this in the root store might cause a variety of unrelated failures.
async fn install_items_in_store<K: Key, V: Value>(
    filesystem: &Arc<FxFilesystem>,
    store: &ObjectStore,
    items: impl AsRef<[Item<K, V>]>,
) {
    let device = filesystem.device();
    let parent_store = store.parent_store().unwrap();
    let mut transaction = filesystem
        .root_store()
        .new_transaction(lock_keys![], Options::default())
        .await
        .expect("new_transaction failed");
    let layer_handle = if let Some(crypt) = store.crypt().as_deref() {
        let object_id = store.get_next_object_id().await.unwrap();
        let (key, unwrapped_key) =
            crypt.create_key(object_id.get(), KeyPurpose::Data).await.unwrap();
        ObjectStore::create_object_with_key(
            parent_store,
            &mut transaction,
            object_id,
            HandleOptions::default(),
            EncryptionKey::Fxfs(key),
            unwrapped_key,
        )
        .await
        .expect("create_object failed")
    } else {
        ObjectStore::create_object(parent_store, &mut transaction, HandleOptions::default(), None)
            .await
            .expect("create_object failed")
    };
    transaction.commit().await.expect("commit failed");

    {
        let mut writer = PersistentLayerWriter::<_, K, V>::new(
            Writer::new(&layer_handle).await,
            items.as_ref().len(),
            filesystem.block_size(),
        )
        .await
        .expect("writer new");
        for item in items.as_ref() {
            writer.write(item.as_item_ref()).await.expect("write failed");
        }
        writer.complete().await.expect("flush failed");
    }

    // store.store_info() holds the current state of the store including unflushed mods.
    // The on-disk version should represent the state of the store at the time the layer files
    // were written (i.e. excluding any entries pending in the journal) so we read it and modify
    // it's layer files.
    let store_info_handle = ObjectStore::open_object(
        parent_store,
        store.store_info_handle_object_id().unwrap(),
        HandleOptions::default(),
        None,
    )
    .await
    .expect("open store info handle failed");

    let mut store_info = if store_info_handle.get_size() == 0 {
        StoreInfo::default()
    } else {
        let mut cursor = std::io::Cursor::new(
            store_info_handle.contents(1000).await.expect("error reading content"),
        );
        StoreInfo::deserialize_with_version(&mut cursor).expect("deserialize_error").0
    };
    store_info.layers.push(layer_handle.object_id());
    let mut store_info_vec = vec![];
    store_info.serialize_with_version(&mut store_info_vec).expect("serialize failed");
    let mut buf = device.allocate_buffer(store_info_vec.len()).await;
    buf.copy_from_slice(&store_info_vec[..]);

    let mut transaction =
        store_info_handle.new_transaction().await.expect("new_transaction failed");
    store_info_handle.txn_write(&mut transaction, 0, buf.as_ref()).await.expect("txn_write failed");
    transaction.commit().await.expect("commit failed");
}

/* TODO(https://fxbug.dev/42173686): Fix this test
#[fuchsia::test]
async fn test_missing_graveyard() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let root_store = fs.root_store();
        let mut transaction = fs.root_store().new_transaction(
                lock_keys![],
                transaction::Options {
                    skip_journal_checks: true,
                    borrow_metadata_space: true,
                    ..Default::default()
                },
            )
            .await
            .expect("New transaction failed");
        transaction.add(root_store.store_object_id, Mutation::graveyard_directory(u64::MAX - 1));
        transaction.commit().await.expect("Commit transaction failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [
            FsckIssue::Error(FsckError::ExtraAllocations(_)),
            FsckIssue::Error(FsckError::AllocatedBytesMismatch(..))
        ]
    );
}
*/

#[fuchsia::test]
async fn test_bad_graveyard_value() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let root_store = fs.root_store();
        let graveyard_id = root_store.graveyard_directory_object_id();
        transaction.add(
            root_store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::graveyard_entry(graveyard_id, 1000),
                ObjectValue::Attribute { size: 500, has_overwrite_extents: false },
            ),
        );
        transaction.commit().await.expect("commit failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Error(FsckError::BadGraveyardValue(_, object_id))] if object_id == 1000
    );
}

#[fuchsia::test]
async fn test_extra_allocation() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        // We need a discontiguous allocation, and some blocks will have been used up by other
        // things, so allocate the very last block.  Note that changing our allocation strategy
        // might break this test.
        let end = fs.block_size().align_down(TEST_DEVICE_BLOCK_SIZE * TEST_DEVICE_BLOCK_COUNT);
        fs.allocator()
            .mark_allocated(&mut transaction, 4, end - fs.block_size()..end)
            .expect("mark_allocated failed");
        transaction.commit().await.expect("commit failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Error(FsckError::ExtraAllocations(_)), ..]);
}

#[fuchsia::test]
async fn test_misaligned_allocation() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        // We need a discontiguous allocation, and some blocks will have been used up by other
        // things, so allocate the very last block.  Note that changing our allocation strategy
        // might break this test.
        let end = fs.block_size().align_down(TEST_DEVICE_BLOCK_SIZE * TEST_DEVICE_BLOCK_COUNT);
        fs.allocator()
            .mark_allocated(&mut transaction, 99, end - fs.block_size() + 1..end)
            .expect("mark_allocated failed");
        transaction.commit().await.expect("commit failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { halt_on_error: true, ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Error(FsckError::MisalignedAllocation(..))]);
}

#[fuchsia::test]
async fn test_malformed_allocation() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let root_store = fs.root_store();
        let device = fs.device();
        // We need to manually insert the record into the allocator's LSM tree directly, since the
        // allocator code checks range validity.

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let layer_handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await
        .expect("create_object failed");
        transaction.commit().await.expect("commit failed");

        {
            let mut writer = PersistentLayerWriter::<_, AllocatorKey, AllocatorValue>::new(
                Writer::new(&layer_handle).await,
                1,
                fs.block_size(),
            )
            .await
            .expect("writer new");
            // We also need a discontiguous allocation, and some blocks will have been used up by
            // other things, so allocate the very last block.  Note that changing our allocation
            // strategy might break this test.
            let end = fs.block_size().align_down(TEST_DEVICE_BLOCK_SIZE * TEST_DEVICE_BLOCK_COUNT);
            let item = Item::new(
                AllocatorKey { device_range: (end..end).into() },
                AllocatorValue::Abs { count: 2, owner_object_id: 9 },
            );
            writer.write(item.as_item_ref()).await.expect("write failed");
            writer.complete().await.expect("flush failed");
        }
        let mut allocator_info = fs.allocator().info();
        allocator_info.layers.push(layer_handle.object_id());
        let mut allocator_info_vec = vec![];
        allocator_info.serialize_with_version(&mut allocator_info_vec).expect("serialize failed");
        let mut buf = device.allocate_buffer(allocator_info_vec.len()).await;
        buf.copy_from_slice(&allocator_info_vec[..]);

        let handle = ObjectStore::open_object(
            &root_store,
            fs.allocator().object_id(),
            HandleOptions::default(),
            None,
        )
        .await
        .expect("open allocator handle failed");
        let mut transaction = handle.new_transaction().await.expect("new_transaction failed");
        handle.txn_write(&mut transaction, 0, buf.as_ref()).await.expect("txn_write failed");
        transaction.commit().await.expect("commit failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { halt_on_error: true, ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Error(FsckError::MalformedAllocation(..))]);
}

#[fuchsia::test]
async fn test_misaligned_extent_in_child_store() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        transaction.add(
            store.store_object_id(),
            Mutation::insert_object(
                ObjectKey::extent(
                    555,
                    AttributeId::TEST_ID,
                    MIN_BLOCK_SIZE.get()..fs.block_size().get(),
                ),
                ObjectValue::Extent(ExtentValue::new_raw(1, VOLUME_DATA_KEY_ID)),
            ),
        );
        transaction.commit().await.expect("commit failed");
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions {
        halt_on_error: true,
        volume_store_id: Some(store_id),
        ..Default::default()
    })
    .await
    .expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Error(FsckError::MisalignedExtent(..))]);
}

#[fuchsia::test]
async fn test_malformed_extent_in_child_store() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        transaction.add(
            store.store_object_id(),
            Mutation::insert_object(
                ObjectKey::extent(555, AttributeId::TEST_ID, fs.block_size().get()..0),
                ObjectValue::Extent(ExtentValue::new_raw(1, VOLUME_DATA_KEY_ID)),
            ),
        );
        transaction.commit().await.expect("commit failed");
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions {
        halt_on_error: true,
        volume_store_id: Some(store_id),
        ..Default::default()
    })
    .await
    .expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Error(FsckError::MalformedExtent(..))]);
}

#[fuchsia::test]
async fn test_allocation_mismatch() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let allocator = fs.allocator();
        let range = {
            let layer_set = allocator.tree().layer_set();
            let mut merger = layer_set.merger();
            let iter = allocator
                .filter(merger.query(Query::FullScan).await.expect("seek failed"), false)
                .await
                .expect("iter failed");
            let ItemRef { key: AllocatorKey { device_range }, .. } =
                iter.get().expect("missing item");
            device_range.clone()
        };
        // Replace owner_object_id with a different owner and bump count to something impossible.
        allocator.tree().replace_or_insert(Item::new(
            AllocatorKey { device_range: range.clone() },
            AllocatorValue::Abs { count: 2, owner_object_id: 10 },
        ));
        allocator.flush().await.expect("flush failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [
            FsckIssue::Error(FsckError::AllocationForNonexistentOwner(..)),
            FsckIssue::Error(FsckError::MissingAllocation(..)),
            FsckIssue::Error(FsckError::AllocatedBytesMismatch(..)),
        ]
    );
}

#[fuchsia::test]
async fn test_volume_allocation_mismatch() {
    let mut test = FsckTest::new().await;
    let store_id = {
        let fs = test.filesystem();
        let store_id = {
            let root_volume = root_volume(fs.clone()).await.unwrap();
            let volume = root_volume
                .new_volume(
                    "vol",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(test.get_crypt()),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            let root_directory = Directory::open(&volume, volume.root_directory_object_id())
                .await
                .expect("open failed");

            let mut transaction = volume
                .new_transaction(
                    lock_keys![LockKey::object(
                        volume.store_object_id(),
                        root_directory.object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let handle = root_directory
                .create_child_file(&mut transaction, "child_file")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            let mut transaction = volume
                .new_transaction(
                    lock_keys![LockKey::object(volume.store_object_id(), handle.object_id())],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let buf = handle.allocate_buffer(1).await;
            handle
                .txn_write(&mut transaction, 1_048_576, buf.as_ref())
                .await
                .expect("write failed");
            transaction.commit().await.expect("commit failed");
            volume.flush().await.expect("Flush store failed");
            volume.store_object_id()
        };

        // Find and break first allocation record for the child store.
        let allocator = fs.allocator();
        let range = {
            let layer_set = allocator.tree().layer_set();
            let mut merger = layer_set.merger();
            let mut iter = allocator
                .filter(merger.query(Query::FullScan).await.expect("seek failed"), false)
                .await
                .expect("iter failed");
            loop {
                if let ItemRef {
                    key: AllocatorKey { device_range },
                    value: AllocatorValue::Abs { owner_object_id, .. },
                    ..
                } = iter.get().expect("no allocations found")
                {
                    if *owner_object_id == store_id {
                        break device_range.clone();
                    }
                }
                iter.advance().await.expect("advance failed");
            }
        };
        allocator.tree().replace_or_insert(Item::new(
            AllocatorKey { device_range: range },
            AllocatorValue::Abs { count: 2, owner_object_id: 42 },
        ));
        allocator.flush().await.expect("flush failed");
        store_id
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions {
        skip_system_fsck: true,
        volume_store_id: Some(store_id),
        ..Default::default()
    })
    .await
    .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [
            FsckIssue::Error(FsckError::AllocationForNonexistentOwner(..)),
            FsckIssue::Error(FsckError::MissingAllocation(..)),
            FsckIssue::Error(FsckError::AllocatedBytesMismatch(..)),
        ]
    );
}

#[fuchsia::test]
async fn test_missing_allocation() {
    let test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let allocator = fs.allocator();
        let key = {
            let layer_set = allocator.tree().layer_set();
            let mut merger = layer_set.merger();
            let iter = allocator
                .filter(merger.query(Query::FullScan).await.expect("seek failed"), false)
                .await
                .expect("iter failed");
            let iter = CoalescingIterator::new(iter).await.expect("filter failed");
            let ItemRef { key, .. } = iter.get().expect("missing item");
            // 'key' points at the first allocation record, which will be for the super blocks.
            key.clone()
        };
        let lower_bound = key.lower_bound_for_merge_into();
        allocator.tree().merge_into(Item::new(key, AllocatorValue::None), &lower_bound);
    }
    // We intentionally don't remount here, since the above tree mutation wouldn't persist
    // otherwise.
    // Structuring this test to actually persist a bad allocation layer file is possible but tricky
    // since flushing or committing transactions might itself perform allocations, and it isn't that
    // important.
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [
            FsckIssue::Error(FsckError::MissingAllocation(..)),
            FsckIssue::Error(FsckError::AllocatedBytesMismatch(..)),
        ]
    );
}

#[fuchsia::test]
async fn test_too_many_object_refs() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();

        let root_store = fs.root_store();
        let root_directory = Directory::open(&root_store, root_store.root_directory_object_id())
            .await
            .expect("open failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    root_store.store_object_id(),
                    root_directory.object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let child_file = root_directory
            .create_child_file(&mut transaction, "child_file")
            .await
            .expect("create_child_file failed");
        let child_dir = root_directory
            .create_child_dir(&mut transaction, "child_dir")
            .await
            .expect("create_child_directory failed");

        // Add an extra reference to the child file.
        child_dir
            .insert_child(&mut transaction, "test", child_file.object_id(), ObjectDescriptor::File)
            .await
            .expect("insert_child failed");
        transaction.commit().await.expect("commit failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Error(FsckError::RefCountMismatch(..)),]);
}

#[fuchsia::test]
async fn test_too_few_object_refs() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let root_store = fs.root_store();

        // Create an object but no directory entry referencing that object, so it will end up with a
        // reference count of one, but zero references.
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        ObjectStore::create_object(&root_store, &mut transaction, HandleOptions::default(), None)
            .await
            .expect("create_object failed");
        transaction.commit().await.expect("commit failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Warning(FsckWarning::OrphanedObject(..))]);
}

#[fuchsia::test]
async fn test_missing_object_tree_layer_file() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let volume = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let mut transaction = volume
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        ObjectStore::create_object(&volume, &mut transaction, HandleOptions::default(), None)
            .await
            .expect("create_object failed");
        transaction.commit().await.expect("commit failed");
        volume.flush().await.expect("Flush store failed");
        let id = {
            let layers = volume.tree().immutable_layer_set();
            assert!(!layers.layers.is_empty());
            layers.layers[0].handle().unwrap().object_id()
        };
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        fs.root_store().add_to_graveyard(&mut transaction, id);
        transaction.commit().await.expect("commit failed");
        fs.root_store()
            .tombstone_object(id, transaction::Options::default(), None)
            .await
            .expect("tombstone failed");
    }

    test.remount().await.expect_err("Remount succeeded");
}

#[fuchsia::test]
async fn test_missing_object_store_handle() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store_id = {
            let volume = root_volume
                .new_volume(
                    "vol",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(test.get_crypt()),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            volume.store_object_id()
        };
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        fs.root_store().add_to_graveyard(&mut transaction, store_id);
        transaction.commit().await.unwrap();

        fs.root_store()
            .tombstone_object(store_id, transaction::Options::default(), None)
            .await
            .expect("tombstone failed");
    }

    test.remount().await.expect_err("Remount succeeded");
}

#[fuchsia::test]
async fn test_misordered_layer_file() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![
                Item::new(
                    ObjectKey::extent(
                        5,
                        AttributeId::TEST_ID,
                        MIN_BLOCK_SIZE.get()..2 * MIN_BLOCK_SIZE,
                    ),
                    ObjectValue::deleted_extent(),
                ),
                Item::new(
                    ObjectKey::extent(1, AttributeId::TEST_ID, 0..MIN_BLOCK_SIZE.get()),
                    ObjectValue::deleted_extent(),
                ),
            ],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Fatal(FsckFatal::MisOrderedLayerFile(..))]);
}

#[fuchsia::test]
async fn test_overlapping_keys_in_layer_file() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![
                Item::new(
                    ObjectKey::extent(1, AttributeId::TEST_ID, 0..2 * MIN_BLOCK_SIZE),
                    ObjectValue::deleted_extent(),
                ),
                Item::new(
                    ObjectKey::extent(
                        1,
                        AttributeId::TEST_ID,
                        MIN_BLOCK_SIZE.get()..3 * MIN_BLOCK_SIZE,
                    ),
                    ObjectValue::deleted_extent(),
                ),
                Item::new(
                    ObjectKey::extent(
                        1,
                        AttributeId::TEST_ID,
                        2 * MIN_BLOCK_SIZE..4 * MIN_BLOCK_SIZE,
                    ),
                    ObjectValue::deleted_extent(),
                ),
            ],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Fatal(FsckFatal::OverlappingKeysInLayerFile(..))]
    );
}

#[fuchsia::test]
async fn test_overlapping_keys_in_root_store_layer_file() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        install_items_in_store(
            &fs,
            &fs.root_store(),
            vec![
                Item::new(
                    ObjectKey::extent(1, AttributeId::TEST_ID, 0..2 * MIN_BLOCK_SIZE),
                    ObjectValue::deleted_extent(),
                ),
                Item::new(
                    ObjectKey::extent(
                        1,
                        AttributeId::TEST_ID,
                        MIN_BLOCK_SIZE.get()..3 * MIN_BLOCK_SIZE,
                    ),
                    ObjectValue::deleted_extent(),
                ),
                Item::new(
                    ObjectKey::extent(
                        1,
                        AttributeId::TEST_ID,
                        2 * MIN_BLOCK_SIZE..4 * MIN_BLOCK_SIZE,
                    ),
                    ObjectValue::deleted_extent(),
                ),
            ],
        )
        .await;
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Fatal(FsckFatal::OverlappingKeysInLayerFile(..))]
    );
}

impl Value for u32 {
    const DELETED_MARKER: Self = 0;
}

#[fuchsia::test]
async fn test_unexpected_record_in_layer_file() {
    let mut test = FsckTest::new().await;
    // This test relies on the value below being something that doesn't deserialize to a valid
    // ObjectValue.
    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![Item::new(ObjectKey::object(0), 0xffffffffu32)],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Fatal(FsckFatal::MalformedLayerFile(..))]);
}

#[fuchsia::test]
async fn test_mismatched_key_and_value() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![Item::new(
                ObjectKey::object(10),
                ObjectValue::Attribute { size: 100, has_overwrite_extents: false },
            )],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Error(FsckError::MalformedObjectRecord(..)), ..]
    );
}

#[fuchsia::test]
async fn test_link_to_root_directory() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let store = fs.root_store();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        root_directory
            .insert_child(
                &mut transaction,
                "a",
                store.root_directory_object_id(),
                ObjectDescriptor::Directory,
            )
            .await
            .expect("insert_child failed");
        transaction.commit().await.expect("commit transaction failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Error(FsckError::RootObjectHasParent(..)), ..]);
}

#[fuchsia::test]
async fn test_multiple_links_to_directory() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let child_dir = root_directory
            .create_child_dir(&mut transaction, "a")
            .await
            .expect("create_child_dir failed");
        root_directory
            .insert_child(&mut transaction, "b", child_dir.object_id(), ObjectDescriptor::Directory)
            .await
            .expect("insert_child failed");
        transaction.commit().await.expect("commit transaction failed");
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Error(FsckError::MultipleLinksToDirectory(..)), ..]
    );
}

#[fuchsia::test]
async fn test_conflicting_link_types() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let store = fs.root_store();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let child_dir = root_directory
            .create_child_dir(&mut transaction, "a")
            .await
            .expect("create_child_dir failed");
        root_directory
            .insert_child(&mut transaction, "b", child_dir.object_id(), ObjectDescriptor::File)
            .await
            .expect("insert_child failed");
        transaction.commit().await.expect("commit transaction failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Error(FsckError::ConflictingTypeForLink(..)), ..]
    );
}

#[fuchsia::test]
async fn test_volume_in_child_store() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        root_directory
            .insert_child(&mut transaction, "a", 10, ObjectDescriptor::Volume)
            .await
            .expect("Create child failed");
        transaction.commit().await.expect("commit transaction failed");
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Error(FsckError::VolumeInChildStore(..)), ..]);
}

#[fuchsia::test]
async fn test_children_on_file() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let object_id = root_directory
            .create_child_file(&mut transaction, "a'")
            .await
            .expect("Create child failed")
            .object_id();
        transaction.commit().await.expect("commit transaction failed");

        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![Item::new(
                ObjectKey::child(object_id, "foo", DirType::Normal),
                ObjectValue::Child(ChildValue {
                    object_id,
                    object_descriptor: ObjectDescriptor::File,
                }),
            )],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Error(FsckError::ObjectHasChildren(..)), ..]);
}

#[fuchsia::test]
async fn test_non_file_marked_as_verified() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume.new_volume("vol", NewChildStoreOptions::default()).await.unwrap();

        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![
                Item::new(
                    ObjectKey::object(10),
                    ObjectValue::Object {
                        kind: ObjectKind::Directory { sub_dirs: 0, dir_type: DirType::Normal },
                        attributes: ObjectAttributes { ..Default::default() },
                    },
                ),
                Item::new(
                    ObjectKey::attribute(10, AttributeId::DATA, AttributeKey::Attribute),
                    ObjectValue::verified_attribute(
                        0,
                        FsverityMetadata::Internal(RootDigest::Sha256([0; 32]), vec![]),
                    ),
                ),
            ],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Error(FsckError::NonFileMarkedAsVerified(..)), ..]
    );
}

#[fuchsia::test]
async fn test_verified_file_merkle_attribute_missing() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume.new_volume("vol", NewChildStoreOptions::default()).await.unwrap();

        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![
                Item::new(
                    ObjectKey::object(10),
                    ObjectValue::Object {
                        kind: ObjectKind::File { refs: 1 },
                        attributes: ObjectAttributes { ..Default::default() },
                    },
                ),
                Item::new(
                    ObjectKey::attribute(10, AttributeId::DATA, AttributeKey::Attribute),
                    ObjectValue::verified_attribute(
                        0,
                        FsverityMetadata::Internal(RootDigest::Sha256([0; 32]), vec![]),
                    ),
                ),
            ],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Error(FsckError::VerifiedFileDoesNotHaveAMerkleAttribute(..)), ..]
    );
}

#[fuchsia::test]
async fn test_orphaned_extended_attribute_record() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();

        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![Item::new(
                ObjectKey::extended_attribute(10, b"foo".to_vec()),
                ObjectValue::inline_extended_attribute(b"value".to_vec()),
            )],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Warning(FsckWarning::OrphanedExtendedAttributeRecord(..)), ..]
    );
}

#[fuchsia::test]
async fn test_orphaned_large_extended_attribute_record() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();

        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![Item::new(
                ObjectKey::extended_attribute(10, b"foo".to_vec()),
                ObjectValue::extended_attribute(AttributeId::XATTR_RANGE_START),
            )],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Warning(FsckWarning::OrphanedExtendedAttributeRecord(..)), ..]
    );
}

#[fuchsia::test]
async fn test_large_extended_attribute_nonexistent_attribute() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();

        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![Item::new(
                ObjectKey::extended_attribute(store.root_directory_object_id(), b"foo".to_vec()),
                ObjectValue::extended_attribute(AttributeId::XATTR_RANGE_START),
            )],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [.., FsckIssue::Error(FsckError::MissingAttributeForExtendedAttribute(..))]
    );
}

#[fuchsia::test]
async fn test_invalid_extended_attribute_id() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();

        let invalid_attr_id = AttributeId(1000);
        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![
                Item::new(
                    ObjectKey::attribute(
                        store.root_directory_object_id(),
                        invalid_attr_id,
                        AttributeKey::Attribute,
                    ),
                    ObjectValue::attribute(0, false),
                ),
                Item::new(
                    ObjectKey::extended_attribute(
                        store.root_directory_object_id(),
                        b"foo".to_vec(),
                    ),
                    ObjectValue::extended_attribute(invalid_attr_id),
                ),
            ],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [.., FsckIssue::Error(FsckError::InvalidExtendedAttributeId(..))]
    );
}

#[fuchsia::test]
async fn test_orphaned_extended_attribute() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();

        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![Item::new(
                ObjectKey::attribute(
                    store.root_directory_object_id(),
                    AttributeId::XATTR_RANGE_START,
                    AttributeKey::Attribute,
                ),
                ObjectValue::attribute(100, false),
            )],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Warning(FsckWarning::OrphanedExtendedAttribute(..)), ..]
    );
}

#[fuchsia::test]
async fn test_orphaned_attribute() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();

        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![Item::new(
                ObjectKey::attribute(10, AttributeId::TEST_ID, AttributeKey::Attribute),
                ObjectValue::attribute(100, false),
            )],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Warning(FsckWarning::OrphanedAttribute(..)), ..]
    );
}

#[fuchsia::test]
async fn test_records_for_tombstoned_object() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();

        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![
                Item::new(ObjectKey::object(10), ObjectValue::None),
                Item::new(
                    ObjectKey::attribute(10, AttributeId::TEST_ID, AttributeKey::Attribute),
                    ObjectValue::attribute(100, false),
                ),
            ],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Error(FsckError::TombstonedObjectHasRecords(..)), ..]
    );
}

#[fuchsia::test]
async fn test_invalid_value_graveyard_attribute_entry() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();

        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![
                Item::new(
                    ObjectKey::object(10),
                    ObjectValue::Object {
                        kind: ObjectKind::File { refs: 1 },
                        attributes: ObjectAttributes { ..Default::default() },
                    },
                ),
                Item::new(
                    ObjectKey::attribute(10, AttributeId::TEST_ID, AttributeKey::Attribute),
                    ObjectValue::attribute(100, false),
                ),
                Item::new(
                    ObjectKey::graveyard_attribute_entry(
                        store.graveyard_directory_object_id(),
                        10,
                        AttributeId::TEST_ID,
                    ),
                    ObjectValue::Trim,
                ),
            ],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [
            FsckIssue::Error(FsckError::MissingEncryptionKeys(..)),
            FsckIssue::Error(FsckError::TrimValueForGraveyardAttributeEntry(
                ..,
                10,
                AttributeId::TEST_ID,
            )),
            ..
        ]
    );
}

#[fuchsia::test]
async fn test_tombstoned_attribute_does_not_exist() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();

        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![
                Item::new(
                    ObjectKey::object(10),
                    ObjectValue::Object {
                        kind: ObjectKind::Directory { sub_dirs: 0, dir_type: DirType::Normal },
                        attributes: ObjectAttributes { ..Default::default() },
                    },
                ),
                Item::new(
                    ObjectKey::graveyard_attribute_entry(
                        store.graveyard_directory_object_id(),
                        10,
                        AttributeId::TEST_ID,
                    ),
                    ObjectValue::Some,
                ),
            ],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [
            FsckIssue::Error(FsckError::TombstonedAttributeDoesNotExist(
                ..,
                10,
                AttributeId::TEST_ID
            )),
            ..
        ]
    );
}

#[fuchsia::test]
async fn test_invalid_object_in_store() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();

        install_items_in_store(
            &fs,
            store.as_ref(),
            vec![Item::new(ObjectKey::object(INVALID_OBJECT_ID), ObjectValue::Some)],
        )
        .await;
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Warning(FsckWarning::InvalidObjectIdInStore(..)), ..]
    );
}

#[fuchsia::test]
async fn test_invalid_child_in_store() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let store = fs.root_store();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        root_directory
            .insert_child(&mut transaction, "a", INVALID_OBJECT_ID, ObjectDescriptor::File)
            .await
            .expect("Insert child failed");
        transaction.commit().await.expect("commit transaction failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Warning(FsckWarning::InvalidObjectIdInStore(..)), ..]
    );
}

#[fuchsia::test]
async fn test_link_cycle() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let store = fs.root_store();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let parent = root_directory
            .create_child_dir(&mut transaction, "a")
            .await
            .expect("Create child failed");
        let child =
            parent.create_child_dir(&mut transaction, "b").await.expect("Create child failed");
        child
            .insert_child(&mut transaction, "c", parent.object_id(), ObjectDescriptor::Directory)
            .await
            .expect("Insert child failed");
        transaction.commit().await.expect("commit transaction failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [
            FsckIssue::Error(FsckError::MultipleLinksToDirectory(..)),
            FsckIssue::Error(FsckError::LinkCycle(..)),
            ..
        ]
    );
}

#[fuchsia::test]
async fn test_orphaned_link_cycle() {
    // This checks we catch a cycle where two directories refer to each other as children.
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let store = fs.root_store();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        let dir1 = Directory::create(&mut transaction, &store, None).await.expect("create failed");
        let dir2 = Directory::create(&mut transaction, &store, None).await.expect("create failed");

        dir1.insert_child(&mut transaction, "dir2", dir2.object_id(), ObjectDescriptor::Directory)
            .await
            .expect("insert_child failed");
        dir2.insert_child(&mut transaction, "dir1", dir1.object_id(), ObjectDescriptor::Directory)
            .await
            .expect("insert_child failed");

        transaction.commit().await.expect("commit transaction failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Error(FsckError::LinkCycle(..)), ..]);
}

#[fuchsia::test]
async fn test_incorrect_merkle_tree_size_empty_file() {
    let mut test = FsckTest::new().await;
    let proper_size;
    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let object = root_directory
            .create_child_file(&mut transaction, "verified file")
            .await
            .expect("Create child failed");
        transaction.commit_and_continue().await.expect("commit_and_continue transaction failed");

        object
            .enable_verity(fio::VerificationOptions {
                hash_algorithm: Some(fio::HashAlgorithm::Sha256),
                salt: Some(vec![]),
                ..Default::default()
            })
            .await
            .expect("set verified file metadata failed");
        proper_size = if let ObjectValue::Attribute { size, .. } = store
            .tree()
            .find(&ObjectKey::attribute(
                object.object_id(),
                AttributeId::FSVERITY_MERKLE,
                AttributeKey::Attribute,
            ))
            .await
            .expect("Finding verity attribute")
            .expect("Verity attribute doesn't exist")
            .value
        {
            size
        } else {
            panic!("Invalid type");
        };
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(
                    object.object_id(),
                    AttributeId::FSVERITY_MERKLE,
                    AttributeKey::Attribute,
                ),
                ObjectValue::attribute(0, false),
            ),
        );
        transaction.commit().await.expect("commit transaction failed");
        store.store_object_id()
    };
    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should have failed");
    assert_matches!(
        &test.errors()[..],
        [.., FsckIssue::Error(FsckError::IncorrectMerkleTreeSize(.., expected_size, actual_size))]
            if *actual_size == 0 && expected_size == &proper_size
    );
}

#[fuchsia::test]
async fn test_incorrect_merkle_tree_size_one_data_block() {
    let mut test = FsckTest::new().await;
    let proper_size;
    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let object = root_directory
            .create_child_file(&mut transaction, "verified file")
            .await
            .expect("Create child failed");
        transaction.commit_and_continue().await.expect("commit_and_continue transaction failed");

        let mut buf = object.allocate_buffer(fs.block_size().get() as usize).await;
        buf.fill(1);
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        object
            .enable_verity(fio::VerificationOptions {
                hash_algorithm: Some(fio::HashAlgorithm::Sha256),
                salt: Some(vec![]),
                ..Default::default()
            })
            .await
            .expect("set verified file metadata failed");
        proper_size = if let ObjectValue::Attribute { size, .. } = store
            .tree()
            .find(&ObjectKey::attribute(
                object.object_id(),
                AttributeId::FSVERITY_MERKLE,
                AttributeKey::Attribute,
            ))
            .await
            .expect("Finding verity attribute")
            .expect("Verity attribute doesn't exist")
            .value
        {
            size
        } else {
            panic!("Invalid type");
        };
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(
                    object.object_id(),
                    AttributeId::FSVERITY_MERKLE,
                    AttributeKey::Attribute,
                ),
                ObjectValue::attribute(2 * <Sha256 as Hasher>::Digest::DIGEST_LEN as u64, false),
            ),
        );
        transaction.commit().await.expect("commit transaction failed");
        store.store_object_id()
    };
    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should have failed");
    assert_matches!(
        &test.errors()[..],
        [FsckIssue::Error(FsckError::IncorrectMerkleTreeSize(.., expected_size, actual_size)), ..]
            if *actual_size == 2 * <Sha256 as Hasher>::Digest::DIGEST_LEN as u64 && expected_size == &proper_size
    );
}

#[fuchsia::test]
async fn test_incorrect_merkle_tree_size_data_unaligned() {
    let mut test = FsckTest::new().await;
    let block_size;
    let proper_size;
    let store_id = {
        let fs = test.filesystem();
        block_size = fs.block_size();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let object = root_directory
            .create_child_file(&mut transaction, "verified file")
            .await
            .expect("Create child failed");
        transaction.commit_and_continue().await.expect("commit_and_continue transaction failed");

        let mut buf = object.allocate_buffer((1 + 5 * fs.block_size()) as usize).await;
        buf.fill(1);
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        object
            .enable_verity(fio::VerificationOptions {
                hash_algorithm: Some(fio::HashAlgorithm::Sha256),
                salt: Some(vec![]),
                ..Default::default()
            })
            .await
            .expect("set verified file metadata failed");
        proper_size = if let ObjectValue::Attribute { size, .. } = store
            .tree()
            .find(&ObjectKey::attribute(
                object.object_id(),
                AttributeId::FSVERITY_MERKLE,
                AttributeKey::Attribute,
            ))
            .await
            .expect("Finding verity attribute")
            .expect("Verity attribute doesn't exist")
            .value
        {
            size
        } else {
            panic!("Invalid type");
        };
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(
                    object.object_id(),
                    AttributeId::FSVERITY_MERKLE,
                    AttributeKey::Attribute,
                ),
                ObjectValue::attribute(block_size + 1, false),
            ),
        );
        transaction.commit().await.expect("commit transaction failed");
        store.store_object_id()
    };
    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should have failed");
    assert_matches!(
        &test.errors()[..],
        [
            FsckIssue::Error(FsckError::IncorrectMerkleTreeSize(
                ..,
                expected_size,
                actual_size,
            )),
            ..
        ] if *actual_size == block_size + 1 && expected_size == &proper_size
    );
}

#[fuchsia::test]
async fn test_file_length_mismatch() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let device = fs.device();
        let store = fs.root_store();

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let handle =
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create object failed");
        transaction.commit().await.expect("commit transaction failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let buf = device.allocate_buffer(1).await;
        handle.txn_write(&mut transaction, 1_048_576, buf.as_ref()).await.expect("write failed");
        transaction.commit().await.expect("commit transaction failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(
                    handle.object_id(),
                    handle.attribute_id(),
                    AttributeKey::Attribute,
                ),
                ObjectValue::attribute(123, false),
            ),
        );
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::object(handle.object_id()),
                ObjectValue::Object {
                    kind: ObjectKind::File { refs: 1 },
                    attributes: ObjectAttributes {
                        creation_time: Timestamp::now(),
                        modification_time: Timestamp::now(),
                        allocated_size: 123,
                        ..Default::default()
                    },
                },
            ),
        );
        transaction.commit().await.expect("commit transaction failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [
            FsckIssue::Error(FsckError::ExtentExceedsLength(..)),
            FsckIssue::Error(FsckError::AllocatedSizeMismatch(..)),
            ..
        ]
    );
}

#[fuchsia::test]
async fn test_spurious_extents() {
    let mut test = FsckTest::new().await;
    const SPURIOUS_OFFSET: u64 = 100 << 20;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        transaction.add(
            store.store_object_id(),
            Mutation::insert_object(
                ObjectKey::extent(555, AttributeId::TEST_ID, 0..4096),
                ObjectValue::Extent(ExtentValue::new_raw(SPURIOUS_OFFSET, VOLUME_DATA_KEY_ID)),
            ),
        );
        transaction.add(
            store.store_object_id(),
            Mutation::insert_object(
                ObjectKey::extent(store.root_directory_object_id(), AttributeId::TEST_ID, 0..4096),
                ObjectValue::Extent(ExtentValue::new_raw(SPURIOUS_OFFSET, VOLUME_DATA_KEY_ID)),
            ),
        );
        transaction.commit().await.expect("commit failed");
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    let mut found = 0;
    for e in test.errors() {
        match e {
            FsckIssue::Warning(FsckWarning::ExtentForMissingAttribute(..)) => found |= 1,
            FsckIssue::Warning(FsckWarning::ExtentForNonexistentObject(..)) => found |= 2,
            _ => {}
        }
    }
    assert_eq!(found, 3, "Missing expected errors: {:?}", test.errors());
}

#[fuchsia::test]
async fn test_missing_encryption_key() {
    let mut test = FsckTest::new().await;

    let (store_id, object_id) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let handle;
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle = root_directory
            .create_child_file(&mut transaction, "child_file")
            .await
            .expect("create_child_file failed");
        let buf = handle.allocate_buffer(1).await;
        handle.txn_write(&mut transaction, 1_048_576, buf.as_ref()).await.expect("write failed");

        let txn_mutation = transaction
            .mutations()
            .iter()
            .find(|m| {
                matches!(
                    m.mutation,
                    Mutation::ObjectStore(ObjectStoreMutation {
                        item: Item {
                            key: ObjectKey {
                                data: ObjectKeyData::Attribute(_, AttributeKey::Extent(_)),
                                ..
                            },
                            ..
                        },
                        ..
                    })
                )
            })
            .expect("find failed");

        let mut mutation = txn_mutation.mutation.clone();

        let store_id = store.store_object_id();
        transaction.remove(store_id, mutation.clone());

        if let Mutation::ObjectStore(ObjectStoreMutation {
            item: Item { value: ObjectValue::Extent(ExtentValue::Some { key_id, .. }), .. },
            ..
        }) = &mut mutation
        {
            *key_id += 1;
        } else {
            unreachable!();
        }

        transaction.add(store_id, mutation);

        transaction.commit().await.expect("commit failed");

        (store_id, handle.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert_matches!(&test.errors()[..], [ FsckIssue::Error(FsckError::MissingKey(sid, oid, 1)) ] if *sid == store_id && *oid == object_id);
}

#[fuchsia::test]
async fn test_orphaned_keys() {
    let mut test = FsckTest::new().await;

    let store_id;
    {
        let fs = test.filesystem();
        let root_store = fs.root_store();
        store_id = root_store.store_object_id();

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![LockKey::object(store_id, 1000)], Options::default())
            .await
            .expect("new_transaction failed");
        transaction.add(
            store_id,
            Mutation::insert_object(
                ObjectKey::keys(1000),
                ObjectValue::Keys(EncryptionKeys::default()),
            ),
        );
        transaction.commit().await.expect("commit failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert_matches!(
        &test.errors()[..],
        [
            FsckIssue::Warning(FsckWarning::OrphanedKeys(sid, 1000)),
            FsckIssue::Error(FsckError::BadLastObjectId(_, _))
        ] if *sid == store_id
    );
}

#[fuchsia::test]
async fn test_missing_encryption_keys() {
    let mut test = FsckTest::new().await;

    let (store_id, object_id) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let handle;
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle = root_directory
            .create_child_file(&mut transaction, "child_file")
            .await
            .expect("create_child_file failed");

        let txn_mutation = transaction
            .mutations()
            .iter()
            .find(|m| {
                matches!(
                    m.mutation,
                    Mutation::ObjectStore(ObjectStoreMutation {
                        item: Item { key: ObjectKey { data: ObjectKeyData::Keys, .. }, .. },
                        ..
                    })
                )
            })
            .expect("find failed");

        let mutation = txn_mutation.mutation.clone();
        let store_id = store.store_object_id();
        transaction.remove(store_id, mutation);

        transaction.commit().await.expect("commit failed");

        (store_id, handle.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert_matches!(&test.errors()[..], [ FsckIssue::Error(FsckError::MissingEncryptionKeys(sid, oid)) ] if *sid == store_id && *oid == object_id);
}

#[fuchsia::test]
async fn test_encrypted_symlink_has_missing_keys() {
    let mut test = FsckTest::new().await;

    let (store_id, symlink_id) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let crypt = test.get_crypt();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let handle;
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle = root_directory
            .create_child_dir(&mut transaction, "dir")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");
        let transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        crypt.add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into()).expect("add_wrapping_key failed");
        handle
            .update_attributes(
                transaction,
                Some(&fio::MutableNodeAttributes {
                    wrapping_key_id: Some(WRAPPING_KEY_ID),
                    ..Default::default()
                }),
                0,
                None,
            )
            .await
            .expect("update attributes failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let symlink_id = handle
            .create_symlink(&mut transaction, b"target", "child_symlink")
            .await
            .expect("create_symlink failed");

        let txn_mutation = transaction
            .mutations()
            .iter()
            .find(|m| match m.mutation {
                Mutation::ObjectStore(ObjectStoreMutation {
                    item:
                        Item { key: ObjectKey { object_id, data: ObjectKeyData::Keys { .. } }, .. },
                    ..
                }) if object_id == symlink_id => true,
                _ => false,
            })
            .expect("find failed");

        let mutation = txn_mutation.mutation.clone();
        let store_id = store.store_object_id();
        transaction.remove(store_id, mutation.clone());
        transaction.commit().await.expect("commit failed");

        (store_id, symlink_id)
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert_matches!(&test.errors()[..], [ FsckIssue::Error(FsckError::MissingKey(sid, oid, 1)) ]
        if *sid == store_id && *oid == symlink_id);
}

#[fuchsia::test]
async fn test_encrypted_directory_has_unencrypted_child() {
    let mut test = FsckTest::new().await;

    let (store_id, parent_oid, child_oid) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let crypt = test.get_crypt();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let handle;
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle = root_directory
            .create_child_dir(&mut transaction, "dir")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");
        let transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        crypt.add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into()).expect("add_wrapping_key failed");
        handle
            .update_attributes(
                transaction,
                Some(&fio::MutableNodeAttributes {
                    wrapping_key_id: Some(WRAPPING_KEY_ID),
                    ..Default::default()
                }),
                0,
                None,
            )
            .await
            .expect("update attributes failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let subdir = handle
            .create_child_dir(&mut transaction, "subdir")
            .await
            .expect("create_child_file failed");

        let txn_mutation = transaction
            .mutations()
            .iter()
            .find(|m| {
                match m.mutation {
                    Mutation::ObjectStore(ObjectStoreMutation {
                        item: Item {
                            key: ObjectKey {
                                object_id,
                                data: ObjectKeyData::EncryptedChild { .. },
                            },
                            ..
                        },
                        ..
                    }) if object_id == handle.object_id() => true,
                    _ => false
                }
            })
            .expect("find failed");

        let mutation = txn_mutation.mutation.clone();
        let store_id = store.store_object_id();
        transaction.remove(store_id, mutation.clone());

        if let Mutation::ObjectStore(ObjectStoreMutation {
            item: Item { value, key: ObjectKey { object_id, .. }, .. },
            ..
        }) = &mutation
        {
            let mutation = Mutation::replace_or_insert_object(
                ObjectKey::child(*object_id, "subdir", DirType::Normal),
                value.clone(),
            );
            transaction.add(store_id, mutation);
            transaction.commit().await.expect("commit failed");
        } else {
            unreachable!();
        }

        (store_id, handle.object_id(), subdir.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    let expected = [FsckIssue::Error(FsckError::EncryptedDirectoryHasUnencryptedChild(
        store_id, parent_oid, child_oid,
    ))];
    assert_eq!(&test.errors()[..], &expected);
}

#[fuchsia::test]
async fn test_encrypted_directory_has_legacy_casefold_child() {
    let mut test = FsckTest::new().await;
    assert!(test.filesystem.is_some());

    let (store_id, parent_oid, child_oid) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let crypt = test.get_crypt();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        // Set up the wrapping key up front.
        crypt.add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into()).expect("add_wrapping_key failed");

        let handle;
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle = root_directory
            .create_child_dir(&mut transaction, "dir")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        // Make the directory encrypted.
        let transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle
            .update_attributes(
                transaction,
                Some(&fio::MutableNodeAttributes {
                    wrapping_key_id: Some(WRAPPING_KEY_ID),
                    ..Default::default()
                }),
                0,
                None,
            )
            .await
            .expect("update attributes failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let subdir = Directory::create_with_options(&mut transaction, &store, DirType::Normal)
            .await
            .expect("create_directory");

        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::child(handle.object_id(), "subdir", DirType::LegacyCasefold),
                ObjectValue::child(subdir.object_id(), ObjectDescriptor::Directory),
            ),
        );
        transaction.commit().await.expect("commit failed");

        (store.store_object_id(), handle.object_id(), subdir.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    // Check that the expected error is present. We use `any` to ignore other incidental
    // errors (like `SubDirCountMismatch`) triggered by manual corruption.
    // The `if` clause is a match guard. `sid`, `pid`, and `cid` are bound as references
    // because `e` is a reference, so we dereference them to compare with values.
    assert!(test.errors().iter().any(|e| matches!(
        e,
        FsckIssue::Error(FsckError::EncryptedDirectoryHasUnencryptedChild(
            sid, pid, cid
        )) if *sid == store_id && *pid == parent_oid && *cid == child_oid
    )));
}

#[fuchsia::test]
async fn test_unencrypted_directory_has_encrypted_child() {
    let mut test = FsckTest::new().await;

    let (store_id, parent_oid, child_oid) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let crypt = test.get_crypt();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let handle;
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle = root_directory
            .create_child_dir(&mut transaction, "dir")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        let subdir = handle
            .create_child_dir(&mut transaction, "subdir")
            .await
            .expect("create_child_file failed");

        let txn_mutation = transaction
            .mutations()
            .iter()
            .find(|m| match m.mutation {
                Mutation::ObjectStore(ObjectStoreMutation {
                    item:
                        Item {
                            key: ObjectKey { object_id, data: ObjectKeyData::Child { .. } },
                            value: ObjectValue::Child(..),
                            ..
                        },
                    ..
                }) if object_id == handle.object_id() => true,
                _ => false,
            })
            .expect("find failed");

        let mutation = txn_mutation.mutation.clone();
        let store_id = store.store_object_id();
        transaction.remove(store_id, mutation.clone());

        if let Mutation::ObjectStore(ObjectStoreMutation {
            item: Item { value, key: ObjectKey { object_id, .. }, .. },
            ..
        }) = &mutation
        {
            let mutation = Mutation::replace_or_insert_object(
                ObjectKey::encrypted_child(*object_id, [1, 2, 3].to_vec(), Some(0)),
                value.clone(),
            );
            transaction.add(store_id, mutation);
            transaction.commit().await.expect("commit failed");
        } else {
            unreachable!();
        }

        (store_id, handle.object_id(), subdir.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert_matches!(
        &test.errors()[..],
        [
            FsckIssue::Error(FsckError::UnencryptedDirectoryHasEncryptedChild(
                sid1,
                oid1,
                oid_child1
            )),
            ..
        ] if *sid1 == store_id && *oid1 == parent_oid && *oid_child1 == child_oid
    );
}

#[fuchsia::test]
async fn test_parent_and_child_encrypted_with_different_wrapping_keys() {
    const CHILD_WRAPPING_KEY_ID: WrappingKeyId = u128::to_le_bytes(3);
    let mut test = FsckTest::new().await;

    let (store_id, parent_oid, child_oid) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let crypt = test.get_crypt();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let handle;
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle = root_directory
            .create_child_dir(&mut transaction, "dir")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");
        let transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        crypt.add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into()).expect("add_wrapping_key failed");
        handle
            .update_attributes(
                transaction,
                Some(&fio::MutableNodeAttributes {
                    wrapping_key_id: Some(WRAPPING_KEY_ID),
                    ..Default::default()
                }),
                0,
                None,
            )
            .await
            .expect("update attributes failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        let subdir = handle
            .create_child_dir(&mut transaction, "subdir")
            .await
            .expect("create_child_file failed");

        let txn_mutation = transaction
            .mutations()
            .iter()
            .find(|m| match m.mutation {
                Mutation::ObjectStore(ObjectStoreMutation {
                    item:
                        Item {
                            key: ObjectKey { object_id, data: ObjectKeyData::Object },
                            value:
                                ObjectValue::Object {
                                    kind: ObjectKind::Directory { dir_type, .. }, ..
                                },
                            ..
                        },
                    ..
                }) if object_id == subdir.object_id() && dir_type.is_encrypted() => true,
                _ => false,
            })
            .expect("find failed");

        let mut mutation = txn_mutation.mutation.clone();
        let store_id = store.store_object_id();
        transaction.remove(store_id, mutation.clone());

        if let Mutation::ObjectStore(ObjectStoreMutation {
            item:
                Item {
                    value: ObjectValue::Object { kind: ObjectKind::Directory { dir_type, .. }, .. },
                    ..
                },
            ..
        }) = &mut mutation
        {
            *dir_type = DirType::Encrypted(CHILD_WRAPPING_KEY_ID);
        } else {
            unreachable!();
        }
        transaction.add(store_id, mutation);

        transaction.commit().await.expect("commit failed");

        (store_id, handle.object_id(), subdir.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert_matches!(
        &test.errors()[..],
        [FsckIssue::Error(FsckError::ChildEncryptedWithDifferentWrappingKeyThanParent(
            sid, oid, oid_child, parent_id, child_id
        ))]
        if *sid == store_id
            && *oid == parent_oid
            && *oid_child == child_oid
            && *parent_id == WRAPPING_KEY_ID
            && *child_id == CHILD_WRAPPING_KEY_ID
    );
}

#[fuchsia::test]
async fn test_encrypted_directory_no_wrapping_key() {
    let mut test = FsckTest::new().await;

    let (store_id, child_oid) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let crypt = test.get_crypt();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let handle;
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle = root_directory
            .create_child_dir(&mut transaction, "dir")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");
        let transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        crypt.add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into()).expect("add_wrapping_key failed");
        handle
            .update_attributes(
                transaction,
                Some(&fio::MutableNodeAttributes {
                    wrapping_key_id: Some(WRAPPING_KEY_ID),
                    ..Default::default()
                }),
                0,
                None,
            )
            .await
            .expect("update attributes failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let subdir = handle
            .create_child_dir(&mut transaction, "subdir")
            .await
            .expect("create_child_file failed");

        let txn_mutation = transaction
            .mutations()
            .iter()
            .find(|m| match m.mutation {
                Mutation::ObjectStore(ObjectStoreMutation {
                    item:
                        Item {
                            key: ObjectKey { object_id, data: ObjectKeyData::Object },
                            value:
                                ObjectValue::Object {
                                    kind: ObjectKind::Directory { dir_type, .. }, ..
                                },
                            ..
                        },
                    ..
                }) if object_id == subdir.object_id() && dir_type.is_encrypted() => true,
                _ => false,
            })
            .expect("find failed");

        let mut mutation = txn_mutation.mutation.clone();
        let store_id = store.store_object_id();
        transaction.remove(store_id, mutation.clone());

        if let Mutation::ObjectStore(ObjectStoreMutation {
            item:
                Item {
                    value: ObjectValue::Object { kind: ObjectKind::Directory { dir_type, .. }, .. },
                    ..
                },
            ..
        }) = &mut mutation
        {
            *dir_type = DirType::Normal;
        } else {
            unreachable!();
        }
        transaction.add(store_id, mutation);

        transaction.commit().await.expect("commit failed");

        (store_id, subdir.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert_matches!(&test.errors()[..], [FsckIssue::Error(FsckError::EncryptedChildDirectoryNoWrappingKey(sid, oid)) ]
    if *sid == store_id && *oid == child_oid);
}

#[fuchsia::test]
async fn test_directory_missing_encryption_key_for_large_extended_attribute() {
    let mut test = FsckTest::new().await;

    let (store_id, object_id) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let crypt = test.get_crypt();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let handle;
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle = root_directory
            .create_child_dir(&mut transaction, "dir")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(
                    handle.object_id(),
                    AttributeId::XATTR_RANGE_START,
                    AttributeKey::Attribute,
                ),
                ObjectValue::attribute(300, false),
            ),
        );
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::extended_attribute(handle.object_id(), b"foo".to_vec()),
                ObjectValue::extended_attribute(AttributeId::XATTR_RANGE_START),
            ),
        );
        transaction.commit().await.expect("commit failed");
        (store.store_object_id(), handle.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert_matches!(&test.errors()[..], [ FsckIssue::Error(FsckError::MissingKey(sid, oid, 0)) ] if *sid == store_id && *oid == object_id);
}

#[fuchsia::test]
async fn test_directory_missing_encryption_key_for_fscrypt() {
    let mut test = FsckTest::new().await;

    let (store_id, object_id) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let crypt = test.get_crypt();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let handle;
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle = root_directory
            .create_child_dir(&mut transaction, "dir")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        crypt.add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into()).expect("add_wrapping_key failed");
        handle
            .set_wrapping_key(&mut transaction, WRAPPING_KEY_ID)
            .await
            .expect("failed to set wrapping key");

        let txn_mutation = transaction
            .mutations()
            .iter()
            .find(|m| match &m.mutation {
                Mutation::ObjectStore(ObjectStoreMutation {
                    item:
                        Item {
                            key: ObjectKey { data: ObjectKeyData::Keys, .. },
                            value: ObjectValue::Keys(keys),
                            ..
                        },
                    ..
                }) => {
                    assert!(keys.get(1).is_some());
                    true
                }
                _ => false,
            })
            .expect("find failed");

        let mut mutation = txn_mutation.mutation.clone();
        let store_id = store.store_object_id();
        transaction.remove(store_id, mutation.clone());

        if let Mutation::ObjectStore(ObjectStoreMutation {
            item: Item { value: ObjectValue::Keys(keys), .. },
            ..
        }) = &mut mutation
        {
            // Move the key from index 1 (FSCRYPT_KEY_ID) to index 0 (VOLUME_DATA_KEY_ID).
            let key = keys.remove(1).unwrap();
            keys.insert(0, key);
        } else {
            unreachable!();
        }

        transaction.add(store_id, mutation);

        transaction.commit().await.expect("commit failed");

        (store_id, handle.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert_matches!(&test.errors()[..], [ FsckIssue::Error(FsckError::MissingKey(sid, oid, 1)) ] if *sid == store_id && *oid == object_id);
}

#[fuchsia::test]
async fn test_duplicate_key() {
    let mut test = FsckTest::new().await;

    let (store_id, object_id, key_id) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let handle;
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle = root_directory
            .create_child_file(&mut transaction, "child_file")
            .await
            .expect("create_child_file failed");

        let txn_mutation = transaction
            .mutations()
            .iter()
            .find(|m| {
                matches!(
                    m.mutation,
                    Mutation::ObjectStore(ObjectStoreMutation {
                        item: Item { key: ObjectKey { data: ObjectKeyData::Keys, .. }, .. },
                        ..
                    })
                )
            })
            .expect("find failed");

        let mut mutation = txn_mutation.mutation.clone();
        let store_id = store.store_object_id();
        transaction.remove(store_id, mutation.clone());

        let key_id;
        if let Mutation::ObjectStore(ObjectStoreMutation {
            item: Item { value: ObjectValue::Keys(keys), .. },
            ..
        }) = &mut mutation
        {
            let (id, key) = keys.first().unwrap().clone();
            key_id = id;
            keys.insert(id, key);
        } else {
            unreachable!();
        }

        transaction.add(store_id, mutation);

        transaction.commit().await.expect("commit failed");

        (store_id, handle.object_id(), key_id)
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert_matches!(&test.errors()[..], [ FsckIssue::Error(FsckError::DuplicateKey(sid, oid, kid)) ] if *sid == store_id && *oid == object_id && *kid == key_id);
}

#[fuchsia::test]
async fn test_project_accounting() {
    let mut test = FsckTest::new().await;

    let store_id;
    let orphaned_object_id;
    {
        let fs = test.filesystem();
        let store = fs.root_store();
        store_id = store.store_object_id();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        // Project 3 in use on file, no info on it.
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store_id, root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let object_id = root_directory
            .create_child_file(&mut transaction, "a")
            .await
            .expect("Create child failed")
            .object_id();
        let mut mutation = transaction
            .get_object_mutation(store_id, ObjectKey::object(object_id))
            .unwrap()
            .clone();
        if let ObjectValue::Object { attributes: ObjectAttributes { project_id, .. }, .. } =
            &mut mutation.item.value
        {
            *project_id = ProjectId::new(3);
        } else {
            panic!("Unexpected object type");
        }
        orphaned_object_id = object_id;
        transaction.add(store_id, Mutation::ObjectStore(mutation));
        transaction.commit().await.expect("commit failed");

        // Project 4 with mismatched actual and usage.
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(store_id, root_directory.object_id()),
                    LockKey::ProjectId {
                        store_object_id: store_id,
                        project_id: ProjectId::new(4).unwrap()
                    },
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        transaction.add(
            store_id,
            Mutation::merge_object(
                ObjectKey::project_usage(root_directory.object_id(), ProjectId::new(4).unwrap()),
                ObjectValue::BytesAndNodes { bytes: 0, nodes: 2 },
            ),
        );
        transaction.add(
            store_id,
            Mutation::insert_object(
                ObjectKey::project_limit(root_directory.object_id(), ProjectId::new(4).unwrap()),
                ObjectValue::BytesAndNodes { bytes: 0, nodes: 0 },
            ),
        );
        let object_id = root_directory
            .create_child_file(&mut transaction, "b")
            .await
            .expect("Create child failed")
            .object_id();
        let mut mutation = transaction
            .get_object_mutation(store_id, ObjectKey::object(object_id))
            .unwrap()
            .clone();
        if let ObjectValue::Object { attributes: ObjectAttributes { project_id, .. }, .. } =
            &mut mutation.item.value
        {
            *project_id = ProjectId::new(4);
        } else {
            panic!("Unexpected object type");
        }
        transaction.add(store_id, Mutation::ObjectStore(mutation));
        transaction.commit().await.expect("commit failed");

        // Project 5 just fine.
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(store_id, root_directory.object_id()),
                    LockKey::ProjectId {
                        store_object_id: store_id,
                        project_id: ProjectId::new(5).unwrap()
                    },
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        transaction.add(
            store_id,
            Mutation::merge_object(
                ObjectKey::project_usage(root_directory.object_id(), ProjectId::new(5).unwrap()),
                ObjectValue::BytesAndNodes { bytes: 0, nodes: 1 },
            ),
        );
        transaction.add(
            store_id,
            Mutation::insert_object(
                ObjectKey::project_limit(root_directory.object_id(), ProjectId::new(5).unwrap()),
                ObjectValue::BytesAndNodes { bytes: 0, nodes: 0 },
            ),
        );
        let object_id = root_directory
            .create_child_file(&mut transaction, "c")
            .await
            .expect("Create child failed")
            .object_id();
        let mut mutation = transaction
            .get_object_mutation(store_id, ObjectKey::object(object_id))
            .unwrap()
            .clone();
        if let ObjectValue::Object { attributes: ObjectAttributes { project_id, .. }, .. } =
            &mut mutation.item.value
        {
            *project_id = ProjectId::new(5);
        } else {
            panic!("Unexpected object type");
        }
        transaction.add(store_id, Mutation::ObjectStore(mutation));
        transaction.commit().await.expect("commit failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert!(test.errors().contains(&FsckIssue::Error(FsckError::ProjectUsedWithNoUsageTracking(
        store_id,
        ProjectId::new(3).unwrap(),
        orphaned_object_id
    ))));
    assert!(test.errors().contains(&FsckIssue::Warning(FsckWarning::ProjectUsageInconsistent(
        store_id,
        ProjectId::new(4).unwrap(),
        (0, 2),
        (0, 1)
    ))));
    assert_eq!(test.errors().len(), 2);
}

#[fuchsia::test]
async fn test_zombie_file() {
    let mut test = FsckTest::new().await;

    let (store_id, object_id, root_object_id) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let handle;
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle = root_directory
            .create_child_file(&mut transaction, "child_file")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        store.add_to_graveyard(&mut transaction, handle.object_id());
        transaction.commit().await.expect("commit failed");
        (store.store_object_id(), handle.object_id(), root_directory.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        &test.errors()[..],
        [
            FsckIssue::Error(FsckError::RefCountMismatch(object_id_1, 2, 1)),
            FsckIssue::Error(FsckError::ZombieFile(_, object_id_2, root_oids)),
        ] if object_id == *object_id_1 && object_id == *object_id_2 && root_oids == &[root_object_id]
    );
}

#[fuchsia::test]
async fn test_zombie_dir() {
    let mut test = FsckTest::new().await;

    let (store_id, object_id, root_object_id) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let handle;
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        handle = root_directory
            .create_child_dir(&mut transaction, "child_dir")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        store.add_to_graveyard(&mut transaction, handle.object_id());
        transaction.commit().await.expect("commit failed");
        (store.store_object_id(), handle.object_id(), root_directory.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [
            FsckIssue::Error(FsckError::ZombieDir(_, object_id_1, root_oid)),
        ] if object_id == object_id_1 && root_oid == root_object_id
    );
}

#[fuchsia::test]
async fn test_zombie_symlink() {
    let mut test = FsckTest::new().await;

    let (store_id, object_id, root_object_id) = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let symlink_object_id = root_directory
            .create_symlink(&mut transaction, b"target", "child_symlink")
            .await
            .expect("create_symlink failed");
        transaction.commit().await.expect("commit failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        store.add_to_graveyard(&mut transaction, symlink_object_id);
        transaction.commit().await.expect("commit failed");
        (store.store_object_id(), symlink_object_id, root_directory.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        &test.errors()[..],
        [
            FsckIssue::Error(FsckError::ZombieSymlink(_, object_id_1, root_oid)),
        ] if object_id == *object_id_1 && root_oid == &[root_object_id]
    );
}

#[fuchsia::test]
async fn test_empty_volume() {
    let mut test = FsckTest::new().await;
    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume.new_volume("vol", NewChildStoreOptions::default()).await.unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let object_id = {
            let file;
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        root_directory.object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            file = root_directory
                .create_child_file(&mut transaction, "child_file")
                .await
                .expect("create_child_file failed");
            let buffer = file.allocate_buffer(1).await;
            file.txn_write(&mut transaction, 0, buffer.as_ref()).await.expect("write failed");
            transaction.commit().await.expect("commit failed");
            file.object_id()
        };
        let mut transaction = root_directory
            .acquire_context_for_replace(None, "child_file", true)
            .await
            .expect("acquire_context_for_replace failed")
            .transaction;

        directory::replace_child(&mut transaction, None, (&root_directory, "child_file"))
            .await
            .expect("failed to unlink");
        transaction.commit().await.expect("commit failed");
        fs.graveyard().queue_tombstone_object(store.store_object_id(), object_id);
        // Make sure the graveyard processes the message so the bytes are deallocated.
        fs.graveyard().flush().await;
        fs.sync(SyncOptions { flush_device: true, ..Default::default() })
            .await
            .expect("sync failed");
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect("Fsck should succeed");
}

#[test_case(true; "read_only")]
#[test_case(false; "read_write")]
#[fuchsia::test]
async fn test_full_disk(read_only: bool) {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let root_store = fs.root_store();
        let device = fs.device();

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let layer_handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await
        .expect("create_object failed");
        transaction.commit().await.expect("commit failed");

        // Now write out our 'fill the disk' allocation.
        {
            let mut writer = PersistentLayerWriter::<_, AllocatorKey, AllocatorValue>::new(
                Writer::new(&layer_handle).await,
                1,
                fs.block_size(),
            )
            .await
            .expect("writer new");
            let end = fs.block_size().align_down(TEST_DEVICE_BLOCK_SIZE * TEST_DEVICE_BLOCK_COUNT);
            let item = Item::new(
                AllocatorKey { device_range: (0..end).into() },
                AllocatorValue::Abs { count: 2, owner_object_id: 9 },
            );
            writer.write(item.as_item_ref()).await.expect("write failed");
            writer.complete().await.expect("flush failed");
        }
        // Discard the mutable layer which contains mutations associated with the write itself.
        fs.allocator()
            .tree()
            .set_mutable_layer(crate::lsm_tree::skip_list_layer::SkipListLayer::new(1024));

        fs.sync(SyncOptions { flush_device: true, ..Default::default() }).await.expect("sync");

        let layer_handle_object_id = layer_handle.object_id();
        let allocator_info = fs.allocator().info();
        let mut allocator_info_vec = vec![];
        allocator_info.serialize_with_version(&mut allocator_info_vec).expect("serialize failed");
        allocator_info_vec.resize(4096, 0);
        let mut buf = device.allocate_buffer(allocator_info_vec.len()).await;
        buf.copy_from_slice(&allocator_info_vec[..]);

        let handle = ObjectStore::open_object(
            &root_store,
            fs.allocator().object_id(),
            HandleOptions::default(),
            None,
        )
        .await
        .expect("open allocator handle failed");
        handle
            .overwrite(
                0,
                buf.as_mut(),
                OverwriteOptions { allow_allocations: true, ..Default::default() },
            )
            .await
            .expect("overwrite failed");

        // Add "layer_handle" to the layer stack for the allocator but be careful not to
        // allocate anything in the process.
        let mut allocator_info = fs.allocator().info();
        allocator_info.layers.push(layer_handle_object_id);
        let mut allocator_info_vec = vec![];
        allocator_info.serialize_with_version(&mut allocator_info_vec).expect("serialize failed");
        allocator_info_vec.resize(4096 * 4, 0);
        let mut buf = device.allocate_buffer(allocator_info_vec.len()).await;
        buf.copy_from_slice(&allocator_info_vec[..]);

        let handle = ObjectStore::open_object(
            &root_store,
            fs.allocator().object_id(),
            HandleOptions::default(),
            None,
        )
        .await
        .expect("open allocator handle failed");
        handle
            .overwrite(
                0,
                buf.as_mut(),
                OverwriteOptions { allow_allocations: true, ..Default::default() },
            )
            .await
            .expect("overwrite failed");
    }
    if read_only {
        test.remount().await.expect("Remount failed");
    } else {
        test.remount_rw().await.expect_err("Remount succeeded");
    }
}

#[fuchsia::test]
async fn test_delete_volume() {
    let mut test = FsckTest::new().await;
    let store_id;
    {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume.new_volume("vol", NewChildStoreOptions::default()).await.unwrap();
        store_id = store.store_object_id();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store_id, root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        root_directory
            .create_child_file(&mut transaction, "child_file")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");
        let fs_clone = fs.clone();
        // Compact in one task while mutating the new store in another task.  This will ensure that
        // we write out a superblock which referencs the newly created volume in
        // journal_file_offsets.
        join!(
            async move {
                fs_clone.journal().force_compact().await.expect("compact failed");
            },
            async move {
                for i in 0..50 {
                    let mut transaction = fs
                        .root_store()
                        .new_transaction(
                            lock_keys![LockKey::object(store_id, root_directory.object_id())],
                            Options::default(),
                        )
                        .await
                        .expect("new_transaction failed");
                    root_directory
                        .create_child_file(&mut transaction, &format!("child_file_{i}"))
                        .await
                        .expect("create_child_file failed");
                    transaction.commit().await.expect("commit failed");
                }
            },
        );
    }
    {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(
                        root_volume.volume_directory().store().store_object_id(),
                        root_volume.volume_directory().object_id(),
                    ),
                    LockKey::flush(store_id)
                ],
                Options { borrow_metadata_space: true, ..Default::default() },
            )
            .await
            .expect("new_transaction failed");
        root_volume.delete_volume("vol", transaction, || {}).await.expect("delete_volume failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect("Fsck should succeed");
}

#[fuchsia::test]
async fn test_casefold() {
    let mut test = FsckTest::new().await;

    for dir_is_casefold in [false, true] {
        let fs = test.filesystem();
        let store = fs.root_store();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let dirname = if dir_is_casefold { "casefolded" } else { "regular" };

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let child_dir = root_directory
            .create_child_dir(&mut transaction, dirname)
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit transaction failed");

        child_dir.set_casefold(dir_is_casefold).await.expect("enable casefold");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), child_dir.object_id()),],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        // Manually add a child entry so we can add the wrong ObjectKeyData child type.
        let handle = Directory::create_with_options(&mut transaction, &store, DirType::Casefold)
            .await
            .expect("create_directory");
        transaction.add(
            child_dir.store().store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::child(
                    child_dir.object_id(),
                    "b",
                    if !dir_is_casefold { DirType::Casefold } else { DirType::Normal },
                ),
                ObjectValue::child(handle.object_id(), ObjectDescriptor::Directory),
            ),
        );
        let now = Timestamp::now();
        child_dir
            .update_dir_attributes_internal(
                &mut transaction,
                child_dir.object_id(),
                MutableAttributesInternal::new(1, Some(now), Some(now.as_nanos()), None),
            )
            .await
            .expect("update attributes");
        transaction.commit().await.expect("commit transaction failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Error(FsckError::CasefoldInconsistency(..)), ..]
    );
}

#[fuchsia::test]
async fn test_legacy_casefold_inconsistency() {
    let mut test = FsckTest::new().await;

    let (store_id, parent_oid, child_oid) = {
        let fs = test.filesystem();
        let store = fs.root_store();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let child_dir = root_directory
            .create_child_dir(&mut transaction, "dir")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit transaction failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), child_dir.object_id()),],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        // Manually add a child entry so we can add the wrong ObjectKeyData child type.
        let handle = Directory::create_with_options(&mut transaction, &store, DirType::Normal)
            .await
            .expect("create_directory");
        transaction.add(
            child_dir.store().store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::child(
                    child_dir.object_id(),
                    "b",
                    DirType::LegacyCasefold, // Wrong type!
                ),
                ObjectValue::child(handle.object_id(), ObjectDescriptor::Directory),
            ),
        );

        transaction.commit().await.expect("commit transaction failed");

        (store.store_object_id(), child_dir.object_id(), handle.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    // Verify that the first reported error is a CasefoldInconsistency for the expected IDs.
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Error(FsckError::CasefoldInconsistency(s, p, c)), ..]
        if s == store_id && p == parent_oid && c == child_oid
    );
}

#[fuchsia::test]
async fn test_missing_overwrite_extents() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let store = fs.root_store();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let file = root_directory
            .create_child_file(&mut transaction, "child_file")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), file.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(
                    file.object_id(),
                    AttributeId::TEST_ID,
                    AttributeKey::Attribute,
                ),
                ObjectValue::attribute(0, true),
            ),
        );
        transaction.commit().await.expect("commit failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Error(FsckError::MissingOverwriteExtents(..)), ..]
    );
}

#[fuchsia::test]
async fn test_overwrite_extent_flag_not_set() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let store = fs.root_store();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let file = root_directory
            .create_child_file(&mut transaction, "child_file")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        file.allocate(0..fs.block_size().get()).await.expect("allocate failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), file.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(file.object_id(), AttributeId::DATA, AttributeKey::Attribute),
                ObjectValue::attribute(fs.block_size().get(), false),
            ),
        );
        transaction.commit().await.expect("commit failed");
    }

    test.remount().await.expect("Remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [FsckIssue::Error(FsckError::OverwriteExtentFlagUnset(..)), ..]
    );
}

#[fuchsia::test]
async fn test_invalid_bloom_filter_for_allocator() {
    let mut test = FsckTest::new().await;

    {
        let fs = test.filesystem();
        let root_store = fs.root_store();
        let device = fs.device();

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let layer_handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await
        .expect("create_object failed");
        transaction.commit().await.expect("commit failed");

        {
            // Fill persistent layers with enough items to start using bloom filter. The following
            // value was chosen from trial and error. The minimum number of items to add is
            // complicated to calculate. It depends on the serialized size of the item,
            // `MINIMUM_DATA_BLOCKS_FOR_BLOOM_FILTER`, and `fs.block_size()`. See the
            // `PersistentLayerWriter`'s implementation of`LayerWriter::write(..)` for details of
            // of how the items are written to the persistent layer.
            let item_count = 1000;
            let mut items: Vec<Item<AllocatorKey, AllocatorValue>> = vec![];
            for i in 0..item_count as u64 {
                // The range per item needs to be disjoint to avoid them being merged.
                items.push(Item::new(
                    AllocatorKey {
                        device_range: ((2 * i * 100 * 512)..(2 * i + 1) * 100 * 512).into(),
                    },
                    AllocatorValue::Abs { count: 1, owner_object_id: 1 },
                ));
            }

            let mut writer = PersistentLayerWriter::<_, AllocatorKey, AllocatorValue>::new(
                Writer::new(&layer_handle).await,
                items.len(),
                fs.block_size(),
            )
            .await
            .expect("Writer::new failed");

            // Write the items into the layer - requires a minimum amount of items before the bloom
            // filter is used.
            for item in AsRef::<Vec<Item<AllocatorKey, AllocatorValue>>>::as_ref(&items) {
                writer.write(item.as_item_ref()).await.expect("write failed");
            }

            // Corrupt the bloom filter by clearing it. This will cause the filter to report that
            // no items exist.
            writer.bloom_filter().clear();

            writer.complete().await.expect("flush failed");
        }
        let mut allocator_info = fs.allocator().info();
        allocator_info.layers.push(layer_handle.object_id());
        let mut allocator_info_vec = vec![];
        allocator_info.serialize_with_version(&mut allocator_info_vec).expect("serialize failed");
        let mut buf = device.allocate_buffer(allocator_info_vec.len()).await;
        buf.copy_from_slice(&allocator_info_vec[..]);

        let handle = ObjectStore::open_object(
            &root_store,
            fs.allocator().object_id(),
            HandleOptions::default(),
            None,
        )
        .await
        .expect("open allocator handle failed");
        let mut transaction = handle.new_transaction().await.expect("new_transaction failed");
        handle.txn_write(&mut transaction, 0, buf.as_ref()).await.expect("txn_write failed");
        transaction.commit().await.expect("commit failed");
    }

    test.remount().await.expect("remount failed");
    test.run(TestOptions::default()).await.expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Fatal(FsckFatal::InvalidBloomFilter(..))]);
}

#[fuchsia::test]
async fn test_invalid_bloom_filter_for_volume() {
    let mut test = FsckTest::new().await;

    // The following is similar to `install_items_in_store` function above, but we need to interact
    // with the persistent layer directly to corrupt the bloom filter.
    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume.new_volume("vol", NewChildStoreOptions::default()).await.unwrap();
        let root_store = fs.root_store();
        let device = fs.device();

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let layer_handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await
        .expect("create_object failed");
        transaction.commit().await.expect("commit failed");

        {
            // Fill persistent layers with enough items to start using bloom filter. The following
            // value was chosen from trial and error. The minimum number of items to add is
            // complicated to calculate. It depends on the serialized size of the item,
            // `MINIMUM_DATA_BLOCKS_FOR_BLOOM_FILTER`, and `fs.block_size()`. See the
            // `PersistentLayerWriter`'s implementation of`LayerWriter::write(..)` for details of
            // of how the items are written to the persistent layer.
            let item_count = 1000;
            let mut items: Vec<Item<ObjectKey, ObjectValue>> = vec![];
            for i in 0..item_count as u64 {
                // The range per item needs to be disjoint to avoid them being merged.
                items.push(Item::new(
                    ObjectKey::extent(
                        100000,
                        AttributeId::TEST_ID,
                        (2 * i * 512)..(2 * i + 1) * 512,
                    ),
                    ObjectValue::deleted_extent(),
                ));
            }

            let mut writer = PersistentLayerWriter::<_, ObjectKey, ObjectValue>::new(
                Writer::new(&layer_handle).await,
                items.len(),
                fs.block_size(),
            )
            .await
            .expect("Writer::new failed");

            // Write the items into the layer - requires a minimum amount of items before the bloom
            // filter is used.
            for item in AsRef::<Vec<Item<ObjectKey, ObjectValue>>>::as_ref(&items) {
                writer.write(item.as_item_ref()).await.expect("write failed");
            }

            // Corrupt the bloom filter by clearing it. This will cause the filter to report that
            // no items exist.
            writer.bloom_filter().clear();

            writer.complete().await.expect("flush failed");
        }
        let store_info_handle = ObjectStore::open_object(
            &root_store,
            store.store_info_handle_object_id().unwrap(),
            HandleOptions::default(),
            None,
        )
        .await
        .expect("open store info handle failed");

        let mut store_info = if store_info_handle.get_size() == 0 {
            StoreInfo::default()
        } else {
            let mut cursor = std::io::Cursor::new(
                store_info_handle.contents(1000).await.expect("error reading content"),
            );
            StoreInfo::deserialize_with_version(&mut cursor).expect("deserialize_error").0
        };
        store_info.layers.push(layer_handle.object_id());
        let mut store_info_vec = vec![];
        store_info.serialize_with_version(&mut store_info_vec).expect("serialize failed");
        let mut buf = device.allocate_buffer(store_info_vec.len()).await;
        buf.copy_from_slice(&store_info_vec[..]);

        let mut transaction =
            store_info_handle.new_transaction().await.expect("new_transaction failed");
        store_info_handle
            .txn_write(&mut transaction, 0, buf.as_ref())
            .await
            .expect("txn_write failed");
        transaction.commit().await.expect("commit failed");
        store.store_object_id()
    };

    test.remount().await.expect("remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Fatal(FsckFatal::InvalidBloomFilter(..))]);
}

#[fuchsia::test]
async fn test_bad_casefold_hash() {
    let mut test = FsckTest::new().await;
    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        let dir = root_directory
            .create_child_dir(&mut transaction, "casefold_dir")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");

        // Verify object exists in tree
        assert!(
            store.tree().exists(&ObjectKey::object(dir.object_id())).await.expect("find failed")
        );
        let dir = Directory::open(&store, dir.object_id()).await.expect("open dir failed");

        dir.set_casefold(true).await.expect("set_casefold failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        test.get_crypt().add_wrapping_key(WRAPPING_KEY_ID, [0; 32]).unwrap();
        dir.set_wrapping_key(&mut transaction, WRAPPING_KEY_ID)
            .await
            .expect("set_wrapping_key failed");
        transaction.commit().await.expect("commit failed");

        // Create a child file "foo" so we have a valid child and key set up.
        // But we want to insert a BAD child "bar" with wrong hash.
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let _file =
            dir.create_child_file(&mut transaction, "foo").await.expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        // Create another file manually with wrong hash.
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let bad_file = ObjectStore::create_object(
            &store,
            &mut transaction,
            HandleOptions::default(),
            Some(WRAPPING_KEY_ID),
        )
        .await
        .expect("create_object failed");

        let cipher = dir.get_fscrypt_key().await.expect("get_key").into_cipher().expect("cipher");
        let name = "bar";
        let encrypted_name = encrypt_filename(cipher.as_ref(), dir.object_id(), name).unwrap();
        let correct_hash = cipher.hash_code_casefold(name);
        let bad_hash = correct_hash.wrapping_add(1);

        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::encrypted_child(dir.object_id(), encrypted_name, Some(bad_hash)),
                ObjectValue::child(bad_file.object_id(), ObjectDescriptor::File),
            ),
        );
        dir.update_dir_attributes_internal(
            &mut transaction,
            dir.object_id(),
            MutableAttributesInternal::new(
                0,
                Some(Timestamp::now()),
                Some(Timestamp::now().as_nanos()),
                None,
            ),
        )
        .await
        .expect("update attr failed");

        transaction.commit().await.expect("commit failed");
        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(test.errors()[..], [FsckIssue::Error(FsckError::BadCasefoldHash(..)), ..]);
}

#[fuchsia::test]
async fn test_bad_last_object_id() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        for i in 0..10 {
            root_directory
                .create_child_file(&mut transaction, &format!("child_file {i}"))
                .await
                .expect("create_child_file failed");
        }
        transaction.commit().await.expect("commit failed");

        store.test_set_last_object_id(1);

        // This ensures the last object ID gets written to StoreInfo.
        fs.journal().force_compact().await.unwrap();

        store.store_object_id()
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..TestOptions::default() })
        .await
        .expect_err("Fsck should fail");
    assert_matches!(
        test.errors()[..],
        [
            FsckIssue::Error(FsckError::NextObjectIdInUse(_, _)),
            FsckIssue::Error(FsckError::BadLastObjectId(_, _)),
        ]
    );
}

#[fuchsia::test]
async fn test_ino_lblk32_key_used_for_non_data_attribute() {
    let mut test = FsckTest::new().await;

    let (store_id, object_id) = {
        let fs = test.filesystem();
        let root_store = fs.root_store();
        let store_id = root_store.store_object_id();

        let handle;
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store_id, root_store.root_directory_object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let root_directory = Directory::open(&root_store, root_store.root_directory_object_id())
            .await
            .expect("open failed");
        handle = root_directory
            .create_child_file(&mut transaction, "foo")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        let mut keys = EncryptionKeys::default();
        keys.insert(2, EncryptionKey::FscryptInoLblk32File { key_identifier: [0u8; 16] });

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store_id, handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        transaction.add(
            store_id,
            Mutation::replace_or_insert_object(
                ObjectKey::keys(handle.object_id()),
                ObjectValue::Keys(keys),
            ),
        );
        transaction.add(
            store_id,
            Mutation::replace_or_insert_object(
                ObjectKey::extent(handle.object_id(), AttributeId::XATTR_RANGE_START, 0..4096),
                ObjectValue::Extent(ExtentValue::Some {
                    device_offset: 0,
                    mode: ExtentMode::Raw,
                    key_id: 2,
                }),
            ),
        );
        transaction.commit().await.expect("commit failed");

        (store_id, handle.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert!(
        test.errors().iter().any(|e| matches!(
            e,
            FsckIssue::Error(FsckError::InvalidInoLblk32KeyUsage(sid, oid))
                if *sid == store_id && *oid == object_id
        )),
        "errors: {:?}",
        test.errors()
    );
}

#[fuchsia::test]
async fn test_ino_lblk32_dir_key_used_for_file() {
    let mut test = FsckTest::new().await;

    let (store_id, object_id) = {
        let fs = test.filesystem();
        let root_store = fs.root_store();
        let store_id = root_store.store_object_id();

        let root_directory = Directory::open(&root_store, root_store.root_directory_object_id())
            .await
            .expect("open failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store_id, root_store.root_directory_object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let handle = root_directory
            .create_child_file(&mut transaction, "foo")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        let mut keys = EncryptionKeys::default();
        keys.insert(
            0,
            EncryptionKey::FscryptInoLblk32Dir { key_identifier: [0u8; 16], nonce: [0u8; 16] },
        );

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store_id, handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        transaction.add(
            store_id,
            Mutation::replace_or_insert_object(
                ObjectKey::keys(handle.object_id()),
                ObjectValue::Keys(keys),
            ),
        );
        transaction.commit().await.expect("commit failed");

        (store_id, handle.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert!(
        test.errors().iter().any(|e| matches!(
            e,
            FsckIssue::Error(FsckError::InvalidInoLblk32KeyUsage(sid, oid))
                if *sid == store_id && *oid == object_id
        )),
        "errors: {:?}",
        test.errors()
    );
}

#[fuchsia::test]
async fn test_ino_lblk32_file_key_used_for_directory() {
    let mut test = FsckTest::new().await;

    let (store_id, object_id) = {
        let fs = test.filesystem();
        let root_store = fs.root_store();
        let store_id = root_store.store_object_id();

        let root_directory = Directory::open(&root_store, root_store.root_directory_object_id())
            .await
            .expect("open failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store_id, root_store.root_directory_object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let handle = root_directory
            .create_child_dir(&mut transaction, "bardir")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");

        let mut keys = EncryptionKeys::default();
        keys.insert(0, EncryptionKey::FscryptInoLblk32File { key_identifier: [0u8; 16] });

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store_id, handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        transaction.add(
            store_id,
            Mutation::replace_or_insert_object(
                ObjectKey::keys(handle.object_id()),
                ObjectValue::Keys(keys),
            ),
        );
        transaction.commit().await.expect("commit failed");

        (store_id, handle.object_id())
    };

    test.remount().await.expect("Remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("Fsck should fail");

    assert!(
        test.errors().iter().any(|e| matches!(
            e,
            FsckIssue::Error(FsckError::InvalidInoLblk32KeyUsage(sid, oid))
                if *sid == store_id && *oid == object_id
        )),
        "errors: {:?}",
        test.errors()
    );
}

#[fuchsia::test]
async fn test_concurrent_fsck_volume_and_flush() {
    let mut test = FsckTest::new().await;

    let store_id = {
        let fs = test.filesystem();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(test.get_crypt()),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .unwrap();
        store.store_object_id()
    };

    test.remount_rw().await.expect("Remount failed");

    let fs = test.filesystem();
    let crypt = test.get_crypt();

    // Acquire a write lock on `LockKey::flush(store_id)` to simulate an ongoing flush / journal
    // compaction on the store.
    let flush_lock_keys = lock_keys![LockKey::flush(store_id)];
    let flush_guard = fs.lock_manager().write_lock(flush_lock_keys).await;

    // Spawn `fsck_volume_with_options` in a concurrent task.
    // `fsck_volume_with_options` will block waiting for `LockKey::flush(store_id)` BEFORE
    // acquiring the commit_mutex.
    let fs_clone = fs.clone();
    let crypt_clone = crypt.clone();
    let fsck_task = fasync::Task::spawn(async move {
        let options = FsckOptions::default();
        fsck_volume_with_options(
            fs_clone.as_ref(),
            &options,
            store_id,
            Some(crypt_clone as Arc<dyn Crypt>),
        )
        .await
    });

    // Give fsck_task a short interval to run until it blocks on LockKey::flush(store_id).
    fasync::Timer::new(std::time::Duration::from_millis(50)).await;

    // Now simulate the flush / journal compaction acquiring commit_mutex (`lock_commits()`).
    // Since `fsck_task` does not hold the commit_mutex whilst waiting for `LockKey::flush`, this
    // acquisition should succeed immediately.
    let commit_guard = fs.lock_commits().await;
    drop(commit_guard);
    drop(flush_guard);

    fsck_task.await.expect("fsck_volume_with_options failed");
}

#[fuchsia::test]
async fn test_illegal_key_in_root_store() {
    let mut test = FsckTest::new().await;

    let (store_id, object_id) = {
        let fs = test.filesystem();
        let root_store = fs.root_store();
        let store_id = root_store.store_object_id();

        let root_directory = Directory::open(&root_store, root_store.root_directory_object_id())
            .await
            .expect("open failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store_id, root_store.root_directory_object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let handle = root_directory
            .create_child_file(&mut transaction, "foo")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        let mut keys = EncryptionKeys::default();
        keys.insert(0, EncryptionKey::FscryptInoLblk32File { key_identifier: [0u8; 16] });

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store_id, handle.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        transaction.add(
            store_id,
            Mutation::replace_or_insert_object(
                ObjectKey::keys(handle.object_id()),
                ObjectValue::Keys(keys),
            ),
        );
        transaction.commit().await.expect("commit failed");

        (store_id, handle.object_id())
    };

    test.remount().await.expect("remount failed");
    test.run(TestOptions { volume_store_id: Some(store_id), ..Default::default() })
        .await
        .expect_err("fsck should fail");

    assert!(
        test.errors().iter().any(|e| matches!(
            e,
            FsckIssue::Error(FsckError::IllegalKeyInRootStore(err_store_id, err_object_id))
                if *err_store_id == store_id && *err_object_id == object_id
        )),
        "errors: {:?}",
        test.errors()
    );
}
