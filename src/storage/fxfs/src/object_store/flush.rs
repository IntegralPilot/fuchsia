// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This module is responsible for flushing (a.k.a. compacting) the object store trees.

use crate::errors::FxfsError;
use crate::filesystem::{FlushReason, ForceMajor};
use crate::log::*;
use crate::lsm_tree::types::{ItemRef, LayerIterator};
use crate::lsm_tree::{LSMTree, layers_from_handles};
use crate::object_handle::{INVALID_OBJECT_ID, ObjectHandle, ReadObjectHandle};
use crate::object_store::extent_record::ExtentValue;
use crate::object_store::object_manager::{ObjectManager, ReservationUpdate};
use crate::object_store::object_record::{ObjectKey, ObjectValue};
use crate::object_store::transaction::{AssociatedObject, LockKey, Mutation, lock_keys};
use crate::object_store::{
    AssocObj, DirectWriter, EncryptedMutations, HandleOptions, LastObjectId, LastObjectIdInfo,
    LockState, MAX_ENCRYPTED_MUTATIONS_SIZE, ObjectStore, Options, ReservedId, StoreInfo,
    layer_size_from_encrypted_mutations_size, tree,
};
use crate::serialized_types::{LATEST_VERSION, Version, VersionedLatest};
use anyhow::{Context, Error, anyhow};
use std::sync::OnceLock;
use std::sync::atomic::Ordering;

#[fxfs_trace::trace]
impl ObjectStore {
    /// Takes a flush lock on self and performs the flush with a default
    /// FlushReason::Journal(ForceMajor::False).
    pub async fn flush(&self) -> Result<Version, Error> {
        self.flush_with_reason(FlushReason::Journal(ForceMajor::False)).await
    }

    /// Takes a flush lock on self and performs the flush.
    pub async fn flush_with_reason(&self, reason: FlushReason) -> Result<Version, Error> {
        let filesystem = self.filesystem();

        let keys = lock_keys![LockKey::flush(self.store_object_id())];
        let _guard = filesystem.lock_manager().write_lock(keys).await;

        self.flush_guarded_with_reason(reason).await
    }

    /// Performs a flush while already holding a flush guard on self.
    #[trace("store_object_id" => self.store_object_id)]
    pub async fn flush_guarded_with_reason(&self, reason: FlushReason) -> Result<Version, Error> {
        if self.parent_store.is_none() {
            // Early exit, but still return the earliest version used by a struct in the tree
            return Ok(self.tree.get_earliest_version());
        }

        // After taking the lock, check to see if the store has been deleted.
        if matches!(*self.lock_state.lock(), LockState::Deleted) {
            // When we compact, it's possible that the store has been deleted since we gathered the
            // list of stores that need compacting.  This is benign.
            return Ok(LATEST_VERSION);
        }

        let filesystem = self.filesystem();
        let object_manager = filesystem.object_manager();
        let earliest_version = self.tree.get_earliest_version();
        let needs_flush = object_manager.needs_flush(self.store_object_id);
        // If we don't need to do anything for the stated flush purpose, do nothing.
        if (reason == FlushReason::Journal(ForceMajor::False) && !needs_flush)
            || (reason == FlushReason::UpgradeVersion && earliest_version == LATEST_VERSION)
            || (reason == FlushReason::EncryptedMutations
                && self.store_info().unwrap().encrypted_mutations_object_id == INVALID_OBJECT_ID)
        {
            // Early exit, but still return the earliest version used by a struct in the
            // tree.
            return Ok(earliest_version);
        }

        let trace = self.trace.load(Ordering::Relaxed);
        if trace {
            info!(store_id = self.store_object_id(); "OS: begin flush");
        }

        if matches!(&*self.lock_state.lock(), LockState::Locked) {
            self.flush_locked().await.with_context(|| {
                format!("Failed to flush object store {}", self.store_object_id)
            })?;
        } else {
            self.flush_unlocked(reason).await.with_context(|| {
                format!("Failed to flush object store {}", self.store_object_id)
            })?;
        }

        if trace {
            info!(store_id = self.store_object_id(); "OS: end flush");
        }
        if let Some(callback) = &*self.flush_callback.lock() {
            callback(self);
        }

        // NOTE: `num_flushes` must only be incremented while the flush lock is held, as
        // `ObjectStore::unlock` relies on this to detect concurrent flushes.
        let mut counters = self.counters.lock();
        counters.num_flushes += 1;
        counters.last_flush_time = Some(std::time::SystemTime::now());
        // Return the earliest version used by a struct in the tree
        Ok(self.tree.get_earliest_version())
    }

    // Flushes an unlocked store. Returns the layer file sizes.
    async fn flush_unlocked(&self, reason: FlushReason) -> Result<Vec<u64>, Error> {
        struct StoreInfoSnapshot<'a> {
            store: &'a ObjectStore,
            store_info: OnceLock<StoreInfo>,
        }
        impl AssociatedObject for StoreInfoSnapshot<'_> {
            fn will_apply_mutation(
                &self,
                _mutation: &Mutation,
                _object_id: u64,
                _manager: &ObjectManager,
            ) {
                let mut store_info = self.store.store_info().unwrap();

                let lock_state = self.store.lock_state.lock();
                if let LockState::Unlocked { mutations_cipher, .. } = &*lock_state {
                    store_info.mutations_cipher_offset = mutations_cipher.sequence_number();
                }

                self.store_info.set(store_info).unwrap();
            }
        }

        let store_info_snapshot = StoreInfoSnapshot { store: self, store_info: OnceLock::new() };

        let filesystem = self.filesystem();
        let object_manager = filesystem.object_manager();
        let reservation = object_manager.metadata_reservation();
        let txn_options = Options {
            skip_journal_checks: true,
            borrow_metadata_space: true,
            allocator_reservation: Some(reservation),
            ..Default::default()
        };

        // The BeginFlush mutation must be within a transaction that has no impact on StoreInfo
        // since we want to get an accurate snapshot of StoreInfo.
        let mut transaction = self.new_transaction(lock_keys![], txn_options).await?;
        transaction.add_with_object(
            self.store_object_id(),
            Mutation::BeginFlush,
            AssocObj::Borrowed(&store_info_snapshot),
        );
        transaction.commit().await.context("Failed to commit BeginFlush transaction")?;

        let mut new_store_info = store_info_snapshot.store_info.into_inner().unwrap();

        // There is a transaction to create objects at the start and then another transaction at the
        // end. Between those two transactions, there are transactions that write to the files.  In
        // the first transaction, objects are created in the graveyard. Upon success, the objects
        // are removed from the graveyard.
        let mut transaction = self.new_transaction(lock_keys![], txn_options).await?;

        // Create and write a new layer, compacting existing layers.
        let parent_store = self.parent_store.as_ref().unwrap();
        let handle_options = HandleOptions { skip_journal_checks: true, ..Default::default() };
        let id_and_key = {
            let mut lock_state = self.lock_state.lock();
            match &mut *lock_state {
                LockState::Unlocked { cached_keys, .. } => {
                    if let Some(item) = cached_keys.pop() {
                        Ok(Some(item))
                    } else {
                        log::warn!("No cached keys for flush for store {}", self.store_object_id());
                        Err(anyhow!(FxfsError::Internal).context("No cached keys for flush"))
                    }
                }
                LockState::UnlockedReadOnly(..) => {
                    Err(anyhow!(FxfsError::Internal).context("Flush on read-only store"))
                }
                LockState::Unencrypted => Ok(None),
                _ => Err(anyhow!(FxfsError::Internal))
                    .with_context(|| format!("Invalid lock state ({:?}) for flush", *lock_state)),
            }
        }?;

        let new_object_tree_layer = if let Some((raw_id, key, unwrapped_key)) = id_and_key {
            let object_id = ReservedId::new(parent_store, raw_id);
            ObjectStore::create_object_with_key(
                parent_store,
                &mut transaction,
                object_id,
                handle_options,
                key,
                unwrapped_key,
            )
            .await?
        } else {
            ObjectStore::create_object(parent_store, &mut transaction, handle_options, None).await?
        };
        let writer = DirectWriter::new(&new_object_tree_layer, txn_options).await;
        let new_object_tree_layer_object_id = new_object_tree_layer.object_id();
        parent_store.add_to_graveyard(&mut transaction, new_object_tree_layer_object_id);

        transaction.commit().await.context("Failed to commit create layer transaction")?;

        // *Do* the actual compaction.
        let full_compaction = match reason {
            FlushReason::UpgradeVersion | FlushReason::Journal(ForceMajor::True) => true,
            _ => false,
        };
        let (layers_to_keep, old_layers) = tree::flush(
            &self.tree,
            writer,
            matches!(reason, FlushReason::Journal(_))
                .then(|| filesystem.journal().get_compaction_yielder()),
            full_compaction,
        )
        .await
        .context("Failed to flush tree")?;

        // Finalise the compaction.
        let mut new_layers = layers_from_handles([new_object_tree_layer]).await?;
        new_layers.extend(layers_to_keep.iter().map(|l| (*l).clone()));

        new_store_info.layers = Vec::new();
        for layer in &new_layers {
            if let Some(handle) = layer.handle() {
                new_store_info.layers.push(handle.object_id());
            }
        }

        let reservation_update: ReservationUpdate; // Must live longer than end_transaction.
        let mut end_transaction = parent_store
            .new_transaction(
                lock_keys![LockKey::object(
                    self.parent_store.as_ref().unwrap().store_object_id(),
                    self.store_info_handle_object_id().unwrap(),
                )],
                txn_options,
            )
            .await?;

        parent_store.remove_from_graveyard(&mut end_transaction, new_object_tree_layer_object_id);

        // Move the existing layers we're compacting to the graveyard at the end.
        for layer in &old_layers {
            if let Some(handle) = layer.handle() {
                parent_store.add_to_graveyard(&mut end_transaction, handle.object_id());
            }
        }

        let old_encrypted_mutations_object_id =
            std::mem::replace(&mut new_store_info.encrypted_mutations_object_id, INVALID_OBJECT_ID);
        if old_encrypted_mutations_object_id != INVALID_OBJECT_ID {
            parent_store.add_to_graveyard(&mut end_transaction, old_encrypted_mutations_object_id);
        }

        // `last_object_id` is updated differently to other members of `StoreInfo`.  We must ensure
        // that those fields match the current in-memory values.  See the lengthy comment in
        // `get_next_object_id` for more information.  `end_transaction` has a lock on the same lock
        // that `get_next_object_id` uses, so there's no danger of the key changing now.
        //
        // This might capture object IDs that might be in transactions not yet committed.  In
        // theory, we could do better than this but it's not worth the effort.
        match &mut new_store_info.last_object_id {
            LastObjectIdInfo::Unencrypted { id } => {
                let LastObjectId::Unencrypted { id: in_memory_value } =
                    &*self.last_object_id.lock()
                else {
                    unreachable!()
                };
                *id = *in_memory_value;
            }
            LastObjectIdInfo::Encrypted { id, key } => {
                let LastObjectId::Encrypted { id: in_memory_value, .. } =
                    &*self.last_object_id.lock()
                else {
                    unreachable!()
                };
                *id = *in_memory_value;
                let guard = self.store_info.lock();
                let current_store_info = guard.as_ref().unwrap();
                let LastObjectIdInfo::Encrypted { key: in_memory_value, .. } =
                    &current_store_info.last_object_id
                else {
                    unreachable!()
                };
                *key = in_memory_value.clone();
            }
            LastObjectIdInfo::Low32Bit => {}
        }

        self.write_store_info(&mut end_transaction, &new_store_info).await?;

        let layer_file_sizes = new_layers
            .iter()
            .map(|l| l.handle().map(ReadObjectHandle::get_size).unwrap_or(0))
            .collect::<Vec<u64>>();

        let total_layer_size = layer_file_sizes.iter().sum();
        reservation_update =
            ReservationUpdate::new(tree::reservation_amount_from_layer_size(total_layer_size));

        end_transaction.add_with_object(
            self.store_object_id(),
            Mutation::EndFlush,
            AssocObj::Borrowed(&reservation_update),
        );

        if self.trace.load(Ordering::Relaxed) {
            info!(
                store_id = self.store_object_id(),
                old_layer_count = old_layers.len(),
                new_layer_count = new_layers.len(),
                total_layer_size,
                new_store_info:?;
                "OS: compacting"
            );
        }

        end_transaction
            .commit_with_callback(|_| {
                let mut store_info = self.store_info.lock();
                let info = store_info.as_mut().unwrap();
                info.layers = new_store_info.layers;
                info.encrypted_mutations_object_id = new_store_info.encrypted_mutations_object_id;
                info.mutations_cipher_offset = new_store_info.mutations_cipher_offset;
                self.tree.set_layers(new_layers);
            })
            .await
            .context("Failed to commit EndFlush transaction")?;

        // Now close the layers and purge them.
        for layer in old_layers {
            let object_id = layer.handle().map(|h| h.object_id());
            layer.close_layer().await;
            if let Some(object_id) = object_id {
                parent_store
                    .tombstone_object(object_id, txn_options, None)
                    .await
                    .context("Failed to tombstone old layer")?;
            }
        }

        if old_encrypted_mutations_object_id != INVALID_OBJECT_ID {
            parent_store
                .tombstone_object(old_encrypted_mutations_object_id, txn_options, None)
                .await
                .context("Failed to tombstone old encrypted mutations")?;
        }

        Ok(layer_file_sizes)
    }

    // Flushes a locked store.
    async fn flush_locked(&self) -> Result<(), Error> {
        let filesystem = self.filesystem();
        let object_manager = filesystem.object_manager();
        let reservation = object_manager.metadata_reservation();
        let txn_options = Options {
            skip_journal_checks: true,
            borrow_metadata_space: true,
            allocator_reservation: Some(reservation),
            ..Default::default()
        };

        let mut transaction = self.new_transaction(lock_keys![], txn_options).await?;
        transaction.add(self.store_object_id(), Mutation::BeginFlush);
        transaction.commit().await.context("Failed to commit BeginFlush transaction")?;

        let mut new_store_info = self.load_store_info().await?;

        // There is a transaction to create objects at the start and then another transaction at the
        // end. Between those two transactions, there are transactions that write to the files.  In
        // the first transaction, objects are created in the graveyard. Upon success, the objects
        // are removed from the graveyard.
        let mut transaction = self.new_transaction(lock_keys![], txn_options).await?;

        let reservation_update: ReservationUpdate; // Must live longer than end_transaction.
        let handle; // Must live longer than end_transaction.
        let mut end_transaction;

        // We need to either write our encrypted mutations to a new file, or append them to an
        // existing one.
        let parent_store = self.parent_store.as_ref().unwrap();
        handle = if new_store_info.encrypted_mutations_object_id == INVALID_OBJECT_ID {
            let handle = ObjectStore::create_object(
                parent_store,
                &mut transaction,
                HandleOptions { skip_journal_checks: true, ..Default::default() },
                None,
            )
            .await?;
            let oid = handle.object_id();
            end_transaction = parent_store
                .new_transaction(
                    lock_keys![
                        LockKey::object(parent_store.store_object_id(), oid),
                        LockKey::object(
                            parent_store.store_object_id(),
                            self.store_info_handle_object_id().unwrap(),
                        ),
                    ],
                    txn_options,
                )
                .await?;
            new_store_info.encrypted_mutations_object_id = oid;
            parent_store.add_to_graveyard(&mut transaction, oid);
            parent_store.remove_from_graveyard(&mut end_transaction, oid);
            handle
        } else {
            end_transaction = parent_store
                .new_transaction(
                    lock_keys![
                        LockKey::object(
                            parent_store.store_object_id(),
                            new_store_info.encrypted_mutations_object_id,
                        ),
                        LockKey::object(
                            parent_store.store_object_id(),
                            self.store_info_handle_object_id().unwrap(),
                        ),
                    ],
                    txn_options,
                )
                .await?;
            ObjectStore::open_object(
                parent_store,
                new_store_info.encrypted_mutations_object_id,
                HandleOptions { skip_journal_checks: true, ..Default::default() },
                None,
            )
            .await?
        };
        transaction
            .commit()
            .await
            .context("Failed to commit create encrypted mutations transaction")?;

        // Append the encrypted mutations, which need to be read from the journal.
        // This assumes that the journal has no buffered mutations for this store (see Self::lock).
        let journaled = filesystem
            .journal()
            .read_transactions_for_object(self.store_object_id)
            .await
            .context("Failed to read encrypted mutations from journal")?;
        let mut buffer = handle.allocate_buffer(MAX_ENCRYPTED_MUTATIONS_SIZE).await;
        let mut writer = buffer.writer();
        EncryptedMutations::from_replayed_mutations(self.store_object_id, journaled)
            .serialize_with_version(&mut writer)?;
        let len = writer.position();
        handle
            .txn_write(&mut end_transaction, handle.get_size(), buffer.subslice(..len))
            .await
            .context("Failed to write encrypted mutations")?;

        self.write_store_info(&mut end_transaction, &new_store_info)
            .await
            .context("Failed to write store info")?;

        let mut total_layer_size = 0;
        for &oid in &new_store_info.layers {
            total_layer_size += parent_store.get_file_size(oid).await?;
        }
        total_layer_size +=
            layer_size_from_encrypted_mutations_size(handle.get_size() + len as u64);

        reservation_update =
            ReservationUpdate::new(tree::reservation_amount_from_layer_size(total_layer_size));

        end_transaction.add_with_object(
            self.store_object_id(),
            Mutation::EndFlush,
            AssocObj::Borrowed(&reservation_update),
        );

        end_transaction.commit().await.context("Failed to commit EndFlush transaction")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{FlushReason, ForceMajor};
    use crate::filesystem::{FxFilesystem, FxFilesystemBuilder};
    use crate::object_handle::{
        INVALID_OBJECT_ID, ObjectHandle, ReadObjectHandle, WriteObjectHandle,
    };
    use crate::object_store::directory::Directory;
    use crate::object_store::journal::JournalOptions;
    use crate::object_store::transaction::{Options, lock_keys};
    use crate::object_store::volume::root_volume;
    use crate::object_store::{
        HandleOptions, LockKey, NewChildStoreOptions, ObjectStore, StoreOptions,
        layer_size_from_encrypted_mutations_size, tree,
    };
    use fxfs_insecure_crypto::new_insecure_crypt;
    use std::sync::Arc;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;

    #[fuchsia::test]
    async fn test_flush_when_locked() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 1024));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let crypt = Arc::new(new_insecure_crypt());
        let store = root_volume
            .new_volume(
                "test",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .expect("new_volume failed");
        let root_dir =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let foo = root_dir
            .create_child_file(&mut transaction, "foo")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        // When the volume is first created it will include a new mutations key but we want to test
        // what happens when the encrypted mutations file doesn't contain a new mutations key, so we
        // flush here.
        store.flush().await.expect("flush failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let bar = root_dir
            .create_child_file(&mut transaction, "bar")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        store.lock().await.expect("lock failed");

        // Flushing the store whilst locked should create an encrypted mutations file.
        store.flush().await.expect("flush failed");

        // Check the reservation.
        let info = store.load_store_info().await.unwrap();
        let parent_store = store.parent_store().unwrap();
        let mut total_layer_size = 0;
        for &oid in &info.layers {
            total_layer_size +=
                parent_store.get_file_size(oid).await.expect("get_file_size failed");
        }
        assert_ne!(info.encrypted_mutations_object_id, INVALID_OBJECT_ID);
        total_layer_size += layer_size_from_encrypted_mutations_size(
            parent_store
                .get_file_size(info.encrypted_mutations_object_id)
                .await
                .expect("get_file_size failed"),
        );
        assert_eq!(
            fs.object_manager().reservation(store.store_object_id()),
            Some(tree::reservation_amount_from_layer_size(total_layer_size))
        );

        // Unlocking the store should replay that encrypted mutations file.
        store.unlock(crypt).await.expect("unlock failed");

        ObjectStore::open_object(&store, foo.object_id(), HandleOptions::default(), None)
            .await
            .expect("open_object failed");

        ObjectStore::open_object(&store, bar.object_id(), HandleOptions::default(), None)
            .await
            .expect("open_object failed");

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn test_major_compaction_frees_reservation_and_merges_layers() {
        let device = DeviceHolder::new(FakeDevice::new(32768, 512));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_volume
            .new_volume("test", NewChildStoreOptions::default())
            .await
            .expect("new_volume failed");
        let root_dir =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        // 1. Populate the store with files to create an initial layer > 512 KiB
        // (DEFAULT_RECLAIM_SIZE).
        let num_files = 2000;
        let mut file_ids = Vec::with_capacity(num_files);
        for i in 0..num_files {
            let filename = format!("file_{:04}_{:<200}", i, i);
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), root_dir.object_id())],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let file = root_dir
                .create_child_file(&mut transaction, &filename)
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            file_ids.push((file.object_id(), filename));
        }

        // Flush (minor) to create Layer 0.
        store.flush().await.expect("flush failed");

        let info_initial = store.load_store_info().await.expect("load_store_info failed");
        assert_eq!(info_initial.layers.len(), 1);
        let initial_reservation = fs
            .object_manager()
            .reservation(store.store_object_id())
            .expect("reservation not found");

        // 2. Delete half the files (1000 files).
        for (oid, filename) in &file_ids[0..1000] {
            let mut transaction = store
                .new_transaction(
                    lock_keys![
                        LockKey::object(store.store_object_id(), root_dir.object_id()),
                        LockKey::object(store.store_object_id(), *oid),
                    ],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let replaced = crate::object_store::directory::replace_child(
                &mut transaction,
                None,
                (&root_dir, filename.as_str()),
            )
            .await
            .expect("replace_child failed");
            assert_matches::assert_matches!(
                replaced,
                crate::object_store::directory::ReplacedChild::Object(id) if id == *oid
            );
            transaction.commit().await.expect("commit failed");
        }

        // 3. Minor flush after deleting 1000 files (FlushReason::Journal(ForceMajor::False)).
        // Minor compaction will NOT merge with the base layer because base layer > 512 KiB.
        store
            .flush_with_reason(FlushReason::Journal(ForceMajor::False))
            .await
            .expect("minor flush failed");

        let info_after_minor = store.load_store_info().await.expect("load_store_info failed");
        assert_eq!(
            info_after_minor.layers.len(),
            2,
            "Minor compaction should keep the base layer, creating 2 layers"
        );
        let res_after_minor = fs
            .object_manager()
            .reservation(store.store_object_id())
            .expect("reservation not found");
        assert!(
            res_after_minor >= initial_reservation,
            "Minor compaction should not decrease reservation because base layer is kept"
        );

        // 4. Major flush (FlushReason::Journal(ForceMajor::True)).
        // Major compaction must merge ALL layers, purge tombstones, and reduce reservation.
        store
            .flush_with_reason(FlushReason::Journal(ForceMajor::True))
            .await
            .expect("major flush failed");

        let info_after_major = store.load_store_info().await.expect("load_store_info failed");
        assert_eq!(
            info_after_major.layers.len(),
            1,
            "Major compaction should merge all layers into 1"
        );
        let res_after_major = fs
            .object_manager()
            .reservation(store.store_object_id())
            .expect("reservation not found");
        assert!(
            res_after_major < initial_reservation,
            "Major compaction should decrease reservation (was {}, initial was {})",
            res_after_major,
            initial_reservation
        );

        // 5. Verify data integrity.
        // Deleted files must be gone.
        for (oid, filename) in &file_ids[0..1000] {
            assert!(
                root_dir.lookup(filename).await.expect("lookup failed").is_none(),
                "Deleted file {} should not exist",
                oid
            );
        }
        for (oid, filename) in &file_ids[1000..num_files] {
            let res = root_dir.lookup(filename).await.expect("lookup failed");
            assert!(res.is_some(), "Surviving file {} should exist", oid);
            assert_eq!(res.unwrap().0, *oid);
        }

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn test_major_compaction_without_journal_mutations() {
        let device = DeviceHolder::new(FakeDevice::new(32768, 512));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_volume
            .new_volume("test", NewChildStoreOptions::default())
            .await
            .expect("new_volume failed");
        let root_dir =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        // Create initial base layer > 512 KiB.
        for i in 0..2000 {
            let filename = format!("file_{:04}_{:<200}", i, i);
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), root_dir.object_id())],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            root_dir.create_child_file(&mut transaction, &filename).await.expect("create failed");
            transaction.commit().await.expect("commit failed");
        }
        store.flush().await.expect("flush failed");

        // Write a small file and do a minor flush to create a second layer.
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        root_dir.create_child_file(&mut transaction, "extra_file").await.expect("create failed");
        transaction.commit().await.expect("commit failed");
        store
            .flush_with_reason(FlushReason::Journal(ForceMajor::False))
            .await
            .expect("flush failed");

        let info = store.load_store_info().await.expect("load_store_info failed");
        assert_eq!(info.layers.len(), 2, "Should have 2 layers before major flush");

        // Now, do a major compaction when there are NO journal mutations in ObjectManager.
        assert!(!fs.object_manager().needs_flush(store.store_object_id()));
        store
            .flush_with_reason(FlushReason::Journal(ForceMajor::True))
            .await
            .expect("major flush failed");

        let info_after_major = store.load_store_info().await.expect("load_store_info failed");
        assert_eq!(
            info_after_major.layers.len(),
            1,
            "Major flush should merge 2 layers into 1 even without journal mutations"
        );

        // Doing another major flush when already at 1 layer and no mutations should succeed
        // and leave 1 layer.
        assert!(!fs.object_manager().needs_flush(store.store_object_id()));
        store
            .flush_with_reason(FlushReason::Journal(ForceMajor::True))
            .await
            .expect("second major flush failed");
        let info_after_second = store.load_store_info().await.expect("load_store_info failed");
        assert_eq!(
            info_after_second.layers.len(),
            1,
            "Major flush when at 1 layer should succeed and leave 1 layer"
        );

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn test_major_compaction_purges_deleted_extents() {
        let device = DeviceHolder::new(FakeDevice::new(32768, 512));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_volume
            .new_volume("test", NewChildStoreOptions::default())
            .await
            .expect("new_volume failed");
        let root_dir =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        // 1. Create a base layer > 512 KiB so minor compaction won't merge with it.
        for i in 0..2000 {
            let filename = format!("file_{:04}_{:<200}", i, i);
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), root_dir.object_id())],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            root_dir.create_child_file(&mut transaction, &filename).await.expect("create failed");
            transaction.commit().await.expect("commit failed");
        }

        // Create a data file with extents.
        let data_oid = {
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), root_dir.object_id())],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let file = root_dir
                .create_child_file(&mut transaction, "data_file")
                .await
                .expect("create_child_file failed");
            let oid = file.object_id();
            transaction.commit().await.expect("commit failed");
            oid
        };

        let data_file = ObjectStore::open_object(&store, data_oid, HandleOptions::default(), None)
            .await
            .expect("open_object failed");
        {
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), data_oid)],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let mut buffer = data_file.allocate_buffer(131072).await;
            buffer.fill(0xAB);
            data_file
                .txn_write(&mut transaction, 0, buffer.as_ref())
                .await
                .expect("txn_write failed");
            transaction.commit().await.expect("commit failed");
        }

        // Flush (minor) to commit extents to Layer 0.
        store.flush().await.expect("flush failed");

        let info0 = store.load_store_info().await.expect("load_store_info failed");
        assert_eq!(info0.layers.len(), 1);

        // Truncate data_file to 0 to write extent tombstones (ExtentValue::None).
        data_file.truncate(0).await.expect("truncate failed");

        // Minor flush: writes Layer 1 with extent tombstones on top of Layer 0.
        store
            .flush_with_reason(FlushReason::Journal(ForceMajor::False))
            .await
            .expect("minor flush failed");
        let info1 = store.load_store_info().await.expect("load_store_info failed");
        assert_eq!(info1.layers.len(), 2, "Minor flush should keep Layer 0 and create Layer 1");

        // Major flush: purges ExtentValue::None tombstones via major_iter and merges into 1 layer.
        store
            .flush_with_reason(FlushReason::Journal(ForceMajor::True))
            .await
            .expect("major flush failed");
        let info2 = store.load_store_info().await.expect("load_store_info failed");
        assert_eq!(info2.layers.len(), 1, "Major flush should merge into 1 layer");

        // Verify data_file size is 0.
        assert_eq!(data_file.get_size(), 0);

        fs.close().await.expect("close failed");
    }

    // This test case is a regression test for b/548631578.  It verifies that we will force major
    // compactions when borrowed_metadata_space gets too large relative to metadata_reservation.
    // Minor compactions are not sufficient in all cases to return borrowed metadata space, which
    // can eventually exhuast the metadata reservation and prevent all operations (including
    // compaction itself).
    #[fuchsia::test]
    async fn test_borrow_metadata_space_fails_without_major_compaction() {
        let reclaim_size = 65536;
        // 102,400 blocks of 512 bytes = 50 MiB device.
        let device = DeviceHolder::new(FakeDevice::new(102400, 512));
        let fs = FxFilesystemBuilder::new()
            .journal_options(JournalOptions { reclaim_size, ..Default::default() })
            .format(true)
            .open(device)
            .await
            .expect("open failed");

        // The test creates many files in several stores, and then deletes them.  The deletions
        // create additional layer files, which should be collapsed into the base layers (and
        // cancel out the file creations) following a major compaction, which should happen
        // automatically due to the large amount of borrowed space.
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let num_stores = 3;
        let files_per_store = 3500;
        let mut stores_and_files = Vec::new();

        for s in 0..num_stores {
            let store = root_volume
                .new_volume(&format!("test_{s}"), NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");
            let root_dir = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let mut file_ids = Vec::with_capacity(files_per_store);
            for i in 0..files_per_store {
                let filename = format!("file_{:04}_{:<200}", i, i);
                let mut transaction = store
                    .new_transaction(
                        lock_keys![LockKey::object(store.store_object_id(), root_dir.object_id())],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                let file = root_dir
                    .create_child_file(&mut transaction, &filename)
                    .await
                    .expect("create_child_file failed");
                transaction.commit().await.expect("commit failed");
                file_ids.push((file.object_id(), filename));
            }
            store
                .flush_with_reason(FlushReason::Journal(ForceMajor::True))
                .await
                .expect("flush failed");
            stores_and_files.push((store, root_dir, file_ids));
        }

        for (_, root_dir, file_ids) in stores_and_files {
            for (oid, filename) in &file_ids[0..2500] {
                let mut context = root_dir
                    .acquire_context_for_replace(None, filename.as_str(), true)
                    .await
                    .expect("acquire_context failed");
                let replaced = crate::object_store::directory::replace_child(
                    &mut context.transaction,
                    None,
                    (&root_dir, filename.as_str()),
                )
                .await
                .expect("replace_child failed");
                assert_matches::assert_matches!(
                    replaced,
                    crate::object_store::directory::ReplacedChild::Object(id) if id == *oid
                );
                context.transaction.commit().await.expect("commit failed");
            }
        }

        fs.close().await.expect("close failed");
    }
}

impl tree::MajorCompactable<ObjectKey, ObjectValue> for LSMTree<ObjectKey, ObjectValue> {
    async fn major_iter(
        iter: impl LayerIterator<ObjectKey, ObjectValue>,
    ) -> Result<impl LayerIterator<ObjectKey, ObjectValue>, Error> {
        iter.filter(|item: ItemRef<'_, _, _>| match item {
            // Object Tombstone.
            ItemRef { value: ObjectValue::None, .. } => false,
            // Deleted extent.
            ItemRef { value: ObjectValue::Extent(ExtentValue::None), .. } => false,
            _ => true,
        })
        .await
    }
}
