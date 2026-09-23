// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::FxfsError;
use crate::filesystem::{FlushReason, FxFilesystem};
use crate::object_store::directory::Directory;
use crate::object_store::transaction::{LockKeys, Mutation, Options, Transaction, lock_keys};
use crate::object_store::tree_cache::TreeCache;
use crate::object_store::{
    ChildValue, DirType, INVALID_OBJECT_ID, LockKey, NewChildStoreOptions, ObjectDescriptor,
    ObjectKey, ObjectStore, ObjectValue, StoreOptions, load_store_info,
};
use crate::serialized_types::LATEST_VERSION;
use anyhow::{Context, Error, anyhow, bail, ensure};
use log::error;
use std::sync::Arc;

// Volumes are a grouping of an object store and a root directory within this object store. They
// model a hierarchical tree of objects within a single store.
//
// Typically there will be one root volume which is referenced directly by the superblock. This root
// volume stores references to all other volumes on the system (as volumes/foo, volumes/bar, ...).
// For now, this hierarchy is only one deep.

pub const VOLUMES_DIRECTORY: &str = "volumes";

/// RootVolume is the top-level volume which stores references to all of the other Volumes.
pub struct RootVolume {
    _root_directory: Directory<ObjectStore>,
    filesystem: Arc<FxFilesystem>,
}

impl RootVolume {
    pub fn volume_directory(&self) -> &Directory<ObjectStore> {
        self.filesystem.object_manager().volume_directory()
    }

    /// Creates a new volume under a transaction lock.
    pub async fn new_volume(
        &self,
        volume_name: &str,
        options: NewChildStoreOptions,
    ) -> Result<Arc<ObjectStore>, Error> {
        let root_store = self.filesystem.root_store();
        let store;
        let mut transaction = root_store
            .new_transaction(
                lock_keys![LockKey::object(
                    root_store.store_object_id(),
                    self.volume_directory().object_id(),
                )],
                Options::default(),
            )
            .await?;

        ensure!(
            matches!(self.volume_directory().lookup(volume_name).await?, None),
            FxfsError::AlreadyExists
        );
        store = root_store
            .new_child_store(&mut transaction, options, Some(Box::new(TreeCache::new())))
            .await?;
        store.set_trace(self.filesystem.trace());

        // We must register the store here because create will add mutations for the store.
        self.filesystem.object_manager().add_store(store.clone());

        // If the transaction fails, we must unregister the store.
        struct CleanUp<'a>(&'a ObjectStore);
        impl Drop for CleanUp<'_> {
            fn drop(&mut self) {
                self.0.filesystem().object_manager().forget_store(self.0.store_object_id());
            }
        }
        let clean_up = CleanUp(&store);

        // Actually create the store in the transaction.
        store.create(&mut transaction).await?;

        self.volume_directory()
            .add_child_volume(&mut transaction, volume_name, store.store_object_id())
            .await?;
        transaction.commit().await?;

        std::mem::forget(clean_up);

        Ok(store)
    }

    /// Returns the volume with the given name.  This is not thread-safe.
    pub async fn volume(
        &self,
        volume_name: &str,
        options: StoreOptions,
    ) -> Result<Arc<ObjectStore>, Error> {
        // Lookup the volume object in the volume directory.
        let (store_object_id, descriptor, _) = self
            .volume_directory()
            .lookup(volume_name)
            .await
            .context("Volume lookup failed")?
            .ok_or(FxfsError::NotFound)
            .context("Volume missing in volume directory")?;
        match descriptor {
            ObjectDescriptor::Volume => (),
            _ => bail!(anyhow!(FxfsError::Inconsistent).context("Expected volume")),
        }
        // Lookup the object store corresponding to the volume.
        let store = self
            .filesystem
            .object_manager()
            .store(store_object_id)
            .ok_or(FxfsError::NotFound)
            .context("Missing volume store")?;
        store.set_trace(self.filesystem.trace());
        // Unlock the volume if required.
        if let Some(crypt) = options.crypt {
            let read_only = self.filesystem.options().read_only;
            store.unlock_inner(crypt, read_only).await.context("Failed to unlock volume")?;
        } else if store.is_locked() {
            bail!(FxfsError::AccessDenied);
        }
        // If the layer files are out of date, kick off a full compaction in the background.
        if let Some(scope_guard) = self.filesystem.scope().active_guard()
            && !self.filesystem.options().read_only
            && store.tree().get_earliest_version() != LATEST_VERSION
        {
            let keys = lock_keys![LockKey::flush(store_object_id)];
            let guard = self
                .filesystem
                .lock_manager()
                .write_lock(keys)
                .await
                .into_owned(self.filesystem.clone());
            let store = store.clone();
            self.filesystem.scope().spawn(async move {
                // This needs to be taken outside the task and passed in. The volume cannot be
                // re-locked until the guard is dropped so this prevents a race where the flush
                // starts after locking by taking it synchronously.
                let _guard = guard;
                // Takes a scope guard so that unencrypted stores will finish their flush before
                // allowing the filesystem to close as well. Journal flushes are normally run on
                // the filesystem's scope, and just like that case it is safe since encrypted
                // volumes can't be locked without taking the flush lock so it will be forced to
                // wait for this completion.
                let _scope_guard = scope_guard;
                if let Err(error) =
                    store.flush_guarded_with_reason(FlushReason::UpgradeVersion).await
                {
                    error!(error:?; "Failed background flush");
                }
            });
        }
        Ok(store)
    }

    /// Deletes the given volume.  Consumes `transaction` and runs `callback` during commit. The
    /// caller must have the correct locks for the volumes directory.
    pub async fn delete_volume(
        &self,
        volume_name: &str,
        mut transaction: Transaction<'_>,
        callback: impl FnOnce() + Send,
    ) -> Result<(), Error> {
        let objects_to_delete = self.delete_volume_impl(volume_name, &mut transaction).await?;
        transaction.commit_with_callback(|_| callback()).await.context("commit")?;
        // Tombstone the deleted objects.
        let root_store = self.filesystem.root_store();
        for object_id in &objects_to_delete {
            root_store.tombstone_object(*object_id, Options::default(), None).await?;
        }
        Ok(())
    }

    async fn delete_volume_impl(
        &self,
        volume_name: &str,
        transaction: &mut Transaction<'_>,
    ) -> Result<Vec<u64>, Error> {
        let object_id =
            match self.volume_directory().lookup(volume_name).await?.ok_or(FxfsError::NotFound)? {
                (object_id, ObjectDescriptor::Volume, _) => object_id,
                _ => bail!(anyhow!(FxfsError::Inconsistent).context("Expected volume")),
            };
        let root_store = self.filesystem.root_store();

        // Delete all the layers and encrypted mutations stored in root_store for this volume.
        // This includes the StoreInfo itself.
        let mut objects_to_delete = load_store_info(&root_store, object_id).await?.parent_objects();
        objects_to_delete.push(object_id);

        for object_id in &objects_to_delete {
            root_store.adjust_refs(transaction, *object_id, -1).await?;
        }
        // Mark all volume data as deleted.
        self.filesystem.allocator().mark_for_deletion(transaction, object_id);
        // Remove the volume entry from the VolumeDirectory.
        self.volume_directory().delete_child_volume(transaction, volume_name, object_id)?;
        Ok(objects_to_delete)
    }

    /// Adds the required mutations to atomically replace a volume, returning a list of object IDs
    /// of objects which can be deleted. If `dst` does not exist, this is equivalent to renaming the
    /// volume from `src` to `dst`. The caller must have the correct locks on the volumes directory.
    pub(crate) async fn replace_volume(
        &self,
        transaction: &mut Transaction<'_>,
        src: &str,
        dst: &str,
    ) -> Result<Option<Vec<u64>>, Error> {
        let src_object_id = match self.volume_directory().lookup(src).await? {
            Some((object_id, ObjectDescriptor::Volume, _)) => Ok(object_id),
            Some(_) => Err(FxfsError::Inconsistent),
            None => Err(FxfsError::NotFound),
        }?;

        let replaced_objects = if let Some((_, ObjectDescriptor::Volume, _)) =
            self.volume_directory().lookup(dst).await?
        {
            Some(self.delete_volume_impl(dst, transaction).await?)
        } else {
            None
        };

        transaction.add(
            self.volume_directory().store().store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::child(self.volume_directory().object_id(), src, DirType::Normal),
                ObjectValue::None,
            ),
        );

        transaction.add(
            self.volume_directory().store().store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::child(self.volume_directory().object_id(), dst, DirType::Normal),
                ObjectValue::Child(ChildValue {
                    object_id: src_object_id,
                    object_descriptor: ObjectDescriptor::Volume,
                }),
            ),
        );

        Ok(replaced_objects)
    }

    /// Attempts to install the image `image_file` in the volume `src` as the volume `dst`. The
    /// image file should be an fxfs partition image containing a volume matching the name `dst`.
    /// The contents of the `dst` volume in the image will be installed in-place into this
    /// filesystem, replacing an existing `dst` volume if one exists.
    ///
    /// There can be no other objects in `src` with extent records, and neither `src` nor `dst` can
    /// be encrypted.
    pub async fn install_volume(
        &self,
        src: &str,
        image_file: &str,
        dst: &str,
    ) -> Result<(), Error> {
        ObjectStore::install_volume(self, src, image_file, dst).await
    }

    /// Acquires a transaction with appropriate locks to remove volume |name|.
    /// Also returns the object ID of the store which will be deleted.
    pub async fn acquire_transaction_for_remove_volume(
        &self,
        name: &str,
        extra_keys: impl IntoIterator<Item = LockKey>,
        allow_not_found: bool,
    ) -> Result<(u64, Transaction<'_>), Error> {
        // Since we don't know the store object ID until we've looked it up in the volumes
        // directory, we need to loop until we have acquired a lock on a store whose ID is the same
        // as it was in the last iteration.
        let volume_dir = self.volume_directory();
        let store = volume_dir.store();
        let extra_keys = extra_keys.into_iter();
        let mut lock_keys = Vec::with_capacity(extra_keys.size_hint().1.unwrap_or(2) + 2);
        lock_keys.extend(extra_keys);
        lock_keys.push(LockKey::object(store.store_object_id(), volume_dir.object_id()));
        let orig_len = lock_keys.len();
        let mut transaction = None;
        loop {
            lock_keys.truncate(orig_len);
            let object_id = match volume_dir.lookup(name).await? {
                Some((object_id, ObjectDescriptor::Volume, _)) => {
                    // We have to ensure that the store isn't flushed while we delete it, because
                    // deleting the store will remove references to it from ObjectManager which are
                    // then updated by flushing.
                    lock_keys.push(LockKey::flush(object_id));
                    object_id
                }
                None => {
                    if allow_not_found {
                        INVALID_OBJECT_ID
                    } else {
                        bail!(FxfsError::NotFound);
                    }
                }
                _ => bail!(anyhow!(FxfsError::Inconsistent).context("Expected volume")),
            };

            // If the IDs match, return the transaction now.
            match transaction {
                Some(result @ (id, _)) if id == object_id => return Ok(result),
                _ => {}
            }

            transaction = Some((
                object_id,
                store
                    .new_transaction(
                        LockKeys::Vec(lock_keys.clone()),
                        Options { borrow_metadata_space: true, ..Default::default() },
                    )
                    .await?,
            ));
        }
    }
}

/// Returns the root volume for the filesystem.
pub async fn root_volume(filesystem: Arc<FxFilesystem>) -> Result<RootVolume, Error> {
    let root_store = filesystem.root_store();
    let root_directory = Directory::open(&root_store, root_store.root_directory_object_id())
        .await
        .context("Unable to open root volume directory")?;
    Ok(RootVolume { _root_directory: root_directory, filesystem })
}

/// Returns the object IDs for all volumes.
pub async fn list_volumes(volume_directory: &Directory<ObjectStore>) -> Result<Vec<u64>, Error> {
    let layer_set = volume_directory.store().tree().layer_set();
    let mut merger = layer_set.merger();
    let mut iter = volume_directory.iter(&mut merger).await?;
    let mut object_ids = vec![];
    while let Some((_, id, _)) = iter.get() {
        object_ids.push(id);
        iter.advance().await?;
    }
    Ok(object_ids)
}

#[cfg(test)]
mod tests {
    use super::root_volume;
    use crate::filesystem::{FxFilesystem, SyncOptions};
    use crate::fsck::{FsckOptions, fsck_volume_with_options, fsck_with_options};
    use crate::lsm_tree::persistent_layer::PersistentLayerWriter;
    use crate::lsm_tree::types::LayerWriter as _;
    use crate::object_handle::{ObjectHandle, WriteObjectHandle};
    use crate::object_store::directory::Directory;
    use crate::object_store::transaction::{Options, lock_keys};
    use crate::object_store::{
        DirectWriter, HandleOptions, LockKey, NewChildStoreOptions, ObjectKey, ObjectStore,
        ObjectValue, StoreOptions,
    };
    use crate::serialized_types::{EARLIEST_SUPPORTED_VERSION, LATEST_VERSION};
    use fxfs_crypto::{Crypt, EncryptionKey, KeyPurpose};
    use fxfs_insecure_crypto::new_insecure_crypt;
    use std::sync::Arc;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;
    use test_case::test_case;

    async fn do_fsck(
        fs: &Arc<FxFilesystem>,
        volume_name: Option<&str>,
        crypt: Option<Arc<dyn Crypt>>,
    ) {
        let fsck_options = FsckOptions {
            fail_on_warning: true,
            on_error: Box::new(|err| eprintln!("fsck error: {:?}", err)),
            ..Default::default()
        };
        fsck_with_options(fs.clone(), &fsck_options).await.expect("fsck filesystem");
        if let Some(volume_name) = volume_name {
            let root = root_volume(fs.clone()).await.unwrap();
            let vol = root
                .volume(
                    volume_name,
                    StoreOptions { crypt: crypt.clone(), ..StoreOptions::default() },
                )
                .await
                .expect("could not open volume");
            fsck_volume_with_options(&fs, &fsck_options, vol.store_object_id(), crypt)
                .await
                .expect("fsck volume");
        }
    }

    #[fuchsia::test]
    async fn test_lookup_nonexistent_volume() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
        root_volume
            .volume(
                "vol",
                StoreOptions {
                    crypt: Some(Arc::new(new_insecure_crypt())),
                    ..StoreOptions::default()
                },
            )
            .await
            .err()
            .expect("Volume shouldn't exist");
        filesystem.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_add_volume() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt = Arc::new(new_insecure_crypt());
        {
            let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume(
                    "vol",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone()),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");
            let mut transaction = filesystem
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new transaction failed");
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            root_directory
                .create_child_file(&mut transaction, "foo")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            filesystem.sync(SyncOptions::default()).await.expect("sync failed");
        };
        {
            filesystem.close().await.expect("Close failed");
            let device = filesystem.take_device().await;
            device.reopen(false);
            let filesystem = FxFilesystem::open(device).await.expect("open failed");
            do_fsck(&filesystem, Some("vol"), Some(crypt)).await;
            let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
            // NOTE: The volume should have been unlocked by `do_fsck` so we omit `crypt` here.
            let volume = root_volume
                .volume("vol", StoreOptions { crypt: None, ..StoreOptions::default() })
                .await
                .expect("volume failed");
            let root_directory = Directory::open(&volume, volume.root_directory_object_id())
                .await
                .expect("open failed");
            root_directory.lookup("foo").await.expect("lookup failed").expect("not found");
            filesystem.close().await.expect("Close failed");
        };
    }

    #[fuchsia::test]
    async fn test_delete_volume() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt = Arc::new(new_insecure_crypt());
        let store_object_id;
        let parent_objects;
        // Add volume and a file (some data).
        let store_id = {
            let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume(
                    "vol",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone()),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");
            store_object_id = store.store_object_id();
            let mut transaction = filesystem
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(store_object_id, store.root_directory_object_id())],
                    Options::default(),
                )
                .await
                .expect("new transaction failed");
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let handle = root_directory
                .create_child_file(&mut transaction, "foo")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");

            let mut buf = handle.allocate_buffer(8192).await;
            buf.fill(0xaa);
            handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
            store.flush().await.expect("flush failed");
            filesystem.sync(SyncOptions::default()).await.expect("sync failed");
            parent_objects = store.parent_objects();
            // Confirm parent objects exist.
            for object_id in &parent_objects {
                let _ = filesystem
                    .root_store()
                    .get_file_size(*object_id)
                    .await
                    .expect("Layer file missing? Bug in test.");
            }
            store.store_object_id()
        };
        filesystem.close().await.expect("Close failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.expect("open failed");
        do_fsck(&filesystem, Some("vol"), Some(crypt.clone())).await;
        {
            // Expect 8kiB accounted to the new volume.
            assert_eq!(
                filesystem.allocator().get_owner_allocated_bytes().get(&store_object_id),
                Some(&8192)
            );
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let transaction = filesystem
                .root_store()
                .new_transaction(
                    lock_keys![
                        LockKey::object(
                            root.volume_directory().store().store_object_id(),
                            root.volume_directory().object_id(),
                        ),
                        LockKey::flush(store_id)
                    ],
                    Options { borrow_metadata_space: true, ..Default::default() },
                )
                .await
                .expect("new_transaction failed");
            root.delete_volume("vol", transaction, || {}).await.expect("delete_volume");
            // Confirm data allocation is gone.
            assert_eq!(
                filesystem
                    .allocator()
                    .get_owner_allocated_bytes()
                    .get(&store_object_id)
                    .unwrap_or(&0),
                &0,
            );
            // Confirm volume entry is gone.
            root.volume(
                "vol",
                StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
            )
            .await
            .err()
            .expect("volume shouldn't exist anymore.");
        }
        filesystem.close().await.expect("Close failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        // All artifacts of the original volume should be gone.
        let filesystem = FxFilesystem::open(device).await.expect("open failed");
        do_fsck(&filesystem, None, None).await;
        for object_id in &parent_objects {
            let _ = filesystem
                .root_store()
                .get_file_size(*object_id)
                .await
                .err()
                .expect("File wasn't deleted.");
        }
        filesystem.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_replace_volume() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        // Add volume "vol" with a file "foo".
        {
            let root = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root.new_volume("vol", NewChildStoreOptions::default()).await.unwrap();
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
                .unwrap();
            let root_directory =
                Directory::open(&store, store.root_directory_object_id()).await.unwrap();
            let _ = root_directory.create_child_file(&mut transaction, "foo").await.unwrap();
            transaction.commit().await.expect("commit failed");
        }
        // Add a second volume "vol2" with a file "foo2".
        {
            let root = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root
                .new_volume("vol2", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");
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
                .expect("new transaction failed");
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let _ = root_directory
                .create_child_file(&mut transaction, "foo2")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
        }
        // Replace "vol" with "vol2", and ensure the filesystem and installed volume passes fsck.
        {
            let root = root_volume(fs.clone()).await.expect("root_volume failed");
            let mut transaction =
                root.acquire_transaction_for_remove_volume("vol", [], false).await.unwrap().1;
            root.replace_volume(&mut transaction, "vol2", "vol").await.unwrap();
            transaction.commit().await.unwrap();
            do_fsck(&fs, Some("vol"), None).await;
        }
        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.unwrap();
        do_fsck(&fs, Some("vol"), None).await;
        {
            let root = root_volume(fs.clone()).await.unwrap();
            // vol2 should now have replaced vol
            root.volume("vol2", StoreOptions::default())
                .await
                .err()
                .expect("vol2 shouldn't exist anymore.");
            let vol = root.volume("vol", StoreOptions::default()).await.unwrap();
            let dir = Directory::open(&vol, vol.root_directory_object_id()).await.unwrap();
            // The contents of "foo" should have been replaced entirely with those from "foo2".
            assert!(dir.lookup("foo").await.unwrap().is_none(), "foo should not be present");
            assert!(dir.lookup("foo2").await.unwrap().is_some(), "foo2 should be present");
        }
        fs.close().await.unwrap();
    }

    #[fuchsia::test]
    async fn test_create_volume_with_guid() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let guid = [1u8; 16];
        {
            let root = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root
                .new_volume("vol", NewChildStoreOptions { guid: Some(guid), ..Default::default() })
                .await
                .unwrap();
            assert_eq!(store.store_info().unwrap().guid, guid);
        }
        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.unwrap();
        {
            let root = root_volume(fs.clone()).await.unwrap();
            let vol = root.volume("vol", StoreOptions::default()).await.unwrap();
            assert_eq!(vol.guid(), guid);
        }
        fs.close().await.unwrap();
    }

    #[test_case(false; "unencrypted")]
    #[test_case(true; "encrypted")]
    #[fuchsia::test]
    async fn test_flush_on_open_to_update_version(encrypted: bool) {
        // Can't test it if we don't have any versions that we can migrate between.
        if LATEST_VERSION == EARLIEST_SUPPORTED_VERSION {
            return;
        }

        let device = DeviceHolder::new(FakeDevice::new(8192, 1024));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt: Option<Arc<dyn Crypt>> =
            if encrypted { Some(Arc::new(new_insecure_crypt())) } else { None };
        {
            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_vol
                .new_volume(
                    "test",
                    NewChildStoreOptions {
                        options: StoreOptions { crypt: crypt.clone(), ..StoreOptions::default() },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");

            let parent_store = store.parent_store().unwrap();
            let txn_options = Options {
                skip_journal_checks: true,
                borrow_metadata_space: true,
                allocator_reservation: Some(fs.object_manager().metadata_reservation()),
                ..Default::default()
            };
            let mut transaction = parent_store
                .new_transaction(lock_keys![], txn_options)
                .await
                .expect("new_transaction failed");
            let new_layer_object = if let Some(crypt) = &crypt {
                let raw_id = parent_store.get_next_object_id().await.unwrap();
                let (fxfs_key, unwrapped_key) =
                    crypt.create_key(raw_id.get(), KeyPurpose::Data).await.unwrap();
                ObjectStore::create_object_with_key(
                    &parent_store,
                    &mut transaction,
                    raw_id,
                    HandleOptions { skip_journal_checks: true, ..Default::default() },
                    EncryptionKey::Fxfs(fxfs_key),
                    unwrapped_key,
                )
                .await
                .expect("create_object_with_key failed")
            } else {
                ObjectStore::create_object(
                    &parent_store,
                    &mut transaction,
                    HandleOptions { skip_journal_checks: true, ..Default::default() },
                    None,
                )
                .await
                .expect("create_object failed")
            };
            transaction.commit().await.expect("commit failed");

            // Add an empty layer to the set with the old version.
            {
                let layer_writer = DirectWriter::new(&new_layer_object, txn_options).await;
                let writer = PersistentLayerWriter::<_, ObjectKey, ObjectValue>::new_with_version(
                    layer_writer,
                    0,
                    fs.block_size(),
                    EARLIEST_SUPPORTED_VERSION,
                )
                .await
                .expect("writer failed");
                writer.complete().await.expect("writer complete failed");
            }

            let mut store_info = store.load_store_info().await.unwrap();
            store_info.layers = vec![new_layer_object.object_id()];
            let mut end_transaction = parent_store
                .new_transaction(
                    lock_keys![LockKey::object(
                        parent_store.store_object_id(),
                        store.store_info_handle_object_id().unwrap()
                    )],
                    txn_options,
                )
                .await
                .expect("new_transaction failed");
            store
                .write_store_info(&mut end_transaction, &store_info)
                .await
                .expect("write_store_info failed");
            end_transaction.commit().await.expect("commit failed");
        }
        fs.close().await.expect("close failed");

        // Re-open filesystem.
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        {
            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
            let _store = root_vol
                .volume("test", StoreOptions { crypt: crypt.clone(), ..StoreOptions::default() })
                .await
                .expect("volume failed");
        }

        // Closing the filesystem should force the background flush to complete.
        fs.close().await.expect("close failed");

        // Re-open filesystem again to ensure the layer file version has been updated.
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        {
            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_vol
                .volume("test", StoreOptions { crypt: crypt.clone(), ..StoreOptions::default() })
                .await
                .expect("volume failed");

            assert_eq!(store.tree.get_earliest_version(), LATEST_VERSION);
        }

        fs.close().await.expect("close failed");
    }
}
