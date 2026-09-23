// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::FxfsError;
use crate::filesystem::TruncateGuard;
use crate::log::*;
use crate::lsm_tree::Query;
use crate::lsm_tree::merge::{Merger, MergerIterator};
use crate::lsm_tree::types::{ItemRef, LayerIterator};
use crate::object_store::object_manager::ObjectManager;
use crate::object_store::object_record::{
    ObjectAttributes, ObjectKey, ObjectKeyData, ObjectKind, ObjectValue, Timestamp,
};
use crate::object_store::transaction::{Mutation, Options, Transaction};
use crate::object_store::{AttributeId, ObjectStore};
use anyhow::{Context, Error, anyhow, bail};
use fuchsia_async::{self as fasync};
use fuchsia_sync::Mutex;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;
use fxfs_trace::{TraceFutureExt, trace_future_args};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;

enum ReaperTask {
    None,
    Pending(UnboundedReceiver<Message>),
    Running(fasync::Task<()>),
}

/// A graveyard exists as a place to park objects that should be deleted when they are no longer in
/// use.  How objects enter and leave the graveyard is up to the caller to decide.  The intention is
/// that at mount time, any objects in the graveyard will get removed.  Each object store has a
/// directory like object that contains a list of the objects within that store that are part of the
/// graveyard.  A single instance of this Graveyard struct manages *all* stores.
pub struct Graveyard {
    object_manager: Arc<ObjectManager>,
    reaper_task: Mutex<ReaperTask>,
    channel: UnboundedSender<Message>,
}

enum Message {
    // Tombstone the object identified by <store-id>, <object-id>, Option<attribute-id>. If
    // <attribute-id> is Some, tombstone just the attribute instead of the entire object.
    Tombstone(u64, u64, Option<AttributeId>),

    // Trims the identified object.
    Trim(u64, u64),

    // When the flush message is processed, notifies sender.  This allows the receiver to know
    // that all preceding tombstone messages have been processed.
    Flush(oneshot::Sender<()>),
}

#[fxfs_trace::trace]
impl Graveyard {
    /// Creates a new instance of the graveyard manager.
    pub fn new(object_manager: Arc<ObjectManager>) -> Arc<Self> {
        let (sender, receiver) = unbounded();
        Arc::new(Graveyard {
            object_manager,
            reaper_task: Mutex::new(ReaperTask::Pending(receiver)),
            channel: sender,
        })
    }

    /// Creates a graveyard object in `store`.  Returns the object ID for the graveyard object.
    pub async fn create(
        transaction: &mut Transaction<'_>,
        store: &ObjectStore,
    ) -> Result<u64, Error> {
        let reserved_object_id = store.get_next_object_id().await?;
        let object_id = reserved_object_id.get();
        let now = Timestamp::now();
        transaction.add(
            store.store_object_id,
            Mutation::insert_object(
                ObjectKey::object(reserved_object_id.release().get()),
                ObjectValue::Object {
                    kind: ObjectKind::Graveyard,
                    attributes: ObjectAttributes {
                        creation_time: now.clone(),
                        modification_time: now,
                        ..Default::default()
                    },
                },
            ),
        );
        Ok(object_id)
    }

    /// Starts an asynchronous task to reap the graveyard for all entries older than
    /// |journal_offset| (exclusive).
    /// If a task is already started, this has no effect, even if that task was targeting an older
    /// |journal_offset|.
    pub fn reap_async(self: Arc<Self>) {
        let mut reaper_task = self.reaper_task.lock();
        if let ReaperTask::Pending(_) = &*reaper_task {
            if let ReaperTask::Pending(receiver) =
                std::mem::replace(&mut *reaper_task, ReaperTask::None)
            {
                *reaper_task = ReaperTask::Running(fasync::Task::spawn(
                    self.clone()
                        .reap_task(receiver)
                        .trace(trace_future_args!("Graveyard::reap_task")),
                ));
            } else {
                unreachable!();
            }
        }
    }

    /// Returns a future which completes when the ongoing reap task (if it exists) completes.
    pub async fn wait_for_reap(&self) {
        self.channel.close_channel();
        let task = std::mem::replace(&mut *self.reaper_task.lock(), ReaperTask::None);
        if let ReaperTask::Running(task) = task {
            task.await;
        }
    }

    async fn reap_task(self: Arc<Self>, mut receiver: UnboundedReceiver<Message>) {
        // Wait and process reap requests.
        while let Some(message) = receiver.next().await {
            match message {
                Message::Tombstone(store_id, object_id, attribute_id) => {
                    let res = if let Some(attribute_id) = attribute_id {
                        self.tombstone_attribute(store_id, object_id, attribute_id).await
                    } else {
                        self.tombstone_object(store_id, object_id, None).await
                    };
                    if let Err(e) = res {
                        error!(
                            error:? = e,
                            store_id,
                            oid = object_id,
                            attribute_id;
                            "Tombstone error"
                        );
                    }
                }
                Message::Trim(store_id, object_id) => {
                    if let Err(e) = self.trim(store_id, object_id).await {
                        error!(error:? = e, store_id, oid = object_id; "Tombstone error");
                    }
                }
                Message::Flush(sender) => {
                    let _ = sender.send(());
                }
            }
        }
    }

    /// Performs the initial mount-time reap for the given store.  This will queue all items in the
    /// graveyard.  Concurrently adding more entries to the graveyard will lead to undefined
    /// behaviour: the entries might or might not be immediately tombstoned, so callers should wait
    /// for this to return before changing to a state where more entries can be added.  Once this
    /// has returned, entries will be tombstoned in the background.
    #[trace]
    pub async fn initial_reap(self: &Arc<Self>, store: &ObjectStore) -> Result<usize, Error> {
        if store.filesystem().options().skip_initial_reap {
            return Ok(0);
        }
        let mut count = 0;
        let layer_set = store.tree().layer_set();
        let mut merger = layer_set.merger();
        let graveyard_object_id = store.graveyard_directory_object_id();
        let mut iter = Self::iter(graveyard_object_id, &mut merger).await?;
        let store_id = store.store_object_id();
        let mut queued_objects = BTreeSet::new();
        while let Some(GraveyardEntryInfo { object_id, attribute_id, value }) = iter.get() {
            store.graveyard_entries.fetch_add(1, Ordering::Relaxed);
            match value {
                ObjectValue::Some => {
                    if let Some(attribute_id) = attribute_id {
                        // If the object is already queued for tombstone, don't queue any attributes
                        // under it as well. The object tombstone will clean up any attributes as
                        // well as their graveyard entries.
                        if !queued_objects.contains(&(store_id, object_id)) {
                            self.queue_tombstone_attribute(store_id, object_id, attribute_id)
                        }
                    } else {
                        queued_objects.insert((store_id, object_id));
                        self.queue_tombstone_object(store_id, object_id)
                    }
                }
                ObjectValue::Trim => {
                    if attribute_id.is_some() {
                        return Err(anyhow!(
                            "Trim is not currently supported for a single attribute"
                        ));
                    }
                    self.queue_trim(store_id, object_id)
                }
                _ => bail!(anyhow!(FxfsError::Inconsistent).context("Bad graveyard value")),
            }
            count += 1;
            iter.advance().await?;
        }
        Ok(count)
    }
    /// Queues an object for tombstoning.
    pub fn queue_tombstone_object(&self, store_id: u64, object_id: u64) {
        let _ = self.channel.unbounded_send(Message::Tombstone(store_id, object_id, None));
    }

    /// Queues an object's attribute for tombstoning.
    pub fn queue_tombstone_attribute(
        &self,
        store_id: u64,
        object_id: u64,
        attribute_id: AttributeId,
    ) {
        let _ = self.channel.unbounded_send(Message::Tombstone(
            store_id,
            object_id,
            Some(attribute_id),
        ));
    }

    fn queue_trim(&self, store_id: u64, object_id: u64) {
        let _ = self.channel.unbounded_send(Message::Trim(store_id, object_id));
    }

    /// Waits for all preceding queued tombstones to finish.
    pub async fn flush(&self) {
        let (sender, receiver) = oneshot::channel::<()>();
        self.channel.unbounded_send(Message::Flush(sender)).unwrap();
        receiver.await.unwrap();
    }

    /// Immediately tombstones (discards) an object in the graveyard.
    /// NB: Code should generally use |queue_tombstone| instead.
    pub async fn tombstone_object(
        &self,
        store_id: u64,
        object_id: u64,
        truncate_guard: Option<&TruncateGuard<'_>>,
    ) -> Result<(), Error> {
        let store = self
            .object_manager
            .store(store_id)
            .with_context(|| format!("Failed to get store {}", store_id))?;
        // For now, it's safe to assume that all objects in the root parent and root store should
        // return space to the metadata reservation, but we might have to revisit that if we end up
        // with objects that are in other stores.
        let options = if store_id == self.object_manager.root_parent_store_object_id()
            || store_id == self.object_manager.root_store_object_id()
        {
            Options {
                borrow_metadata_space: true,
                allocator_reservation: Some(self.object_manager.metadata_reservation()),
                ..Default::default()
            }
        } else {
            Options { borrow_metadata_space: true, ..Default::default() }
        };
        store.tombstone_object(object_id, options, truncate_guard).await
    }

    /// Immediately tombstones (discards) and attribute in the graveyard.
    /// NB: Code should generally use |queue_tombstone| instead.
    pub async fn tombstone_attribute(
        &self,
        store_id: u64,
        object_id: u64,
        attribute_id: AttributeId,
    ) -> Result<(), Error> {
        let store = self
            .object_manager
            .store(store_id)
            .with_context(|| format!("Failed to get store {}", store_id))?;
        // For now, it's safe to assume that all objects in the root parent and root store should
        // return space to the metadata reservation, but we might have to revisit that if we end up
        // with objects that are in other stores.
        let options = if store_id == self.object_manager.root_parent_store_object_id()
            || store_id == self.object_manager.root_store_object_id()
        {
            Options {
                borrow_metadata_space: true,
                allocator_reservation: Some(self.object_manager.metadata_reservation()),
                ..Default::default()
            }
        } else {
            Options { borrow_metadata_space: true, ..Default::default() }
        };
        store.tombstone_attribute(object_id, attribute_id, options).await
    }

    async fn trim(&self, store_id: u64, object_id: u64) -> Result<(), Error> {
        let store = self
            .object_manager
            .store(store_id)
            .with_context(|| format!("Failed to get store {}", store_id))?;
        let fs = store.filesystem();
        let truncate_guard = fs.truncate_guard(store_id, object_id).await;
        store.trim(object_id, &truncate_guard).await.context("Failed to trim object")
    }

    /// Returns an iterator that will return graveyard entries skipping deleted ones.  Example
    /// usage:
    ///
    ///   let layer_set = graveyard.store().tree().layer_set();
    ///   let mut merger = layer_set.merger();
    ///   let mut iter = graveyard.iter(&mut merger).await?;
    ///
    pub async fn iter<'a, 'b>(
        graveyard_object_id: u64,
        merger: &'a mut Merger<'b, ObjectKey, ObjectValue>,
    ) -> Result<GraveyardIterator<'a, 'b>, Error> {
        Self::iter_from(merger, graveyard_object_id, 0).await
    }

    /// Like "iter", but seeks from a specific (store-id, object-id) tuple.  Example usage:
    ///
    ///   let layer_set = graveyard.store().tree().layer_set();
    ///   let mut merger = layer_set.merger();
    ///   let mut iter = graveyard.iter_from(&mut merger, (2, 3)).await?;
    ///
    async fn iter_from<'a, 'b>(
        merger: &'a mut Merger<'b, ObjectKey, ObjectValue>,
        graveyard_object_id: u64,
        from: u64,
    ) -> Result<GraveyardIterator<'a, 'b>, Error> {
        GraveyardIterator::new(
            graveyard_object_id,
            merger
                .query(Query::FullRange(&ObjectKey::graveyard_entry(graveyard_object_id, from)))
                .await?,
        )
        .await
    }
}

pub struct GraveyardIterator<'a, 'b> {
    object_id: u64,
    iter: MergerIterator<'a, 'b, ObjectKey, ObjectValue>,
}

/// Contains information about a graveyard entry associated with a particular object or
/// attribute.
#[derive(Debug, PartialEq)]
pub struct GraveyardEntryInfo {
    object_id: u64,
    attribute_id: Option<AttributeId>,
    value: ObjectValue,
}

impl GraveyardEntryInfo {
    pub fn object_id(&self) -> u64 {
        self.object_id
    }

    pub fn attribute_id(&self) -> Option<AttributeId> {
        self.attribute_id
    }

    pub fn value(&self) -> &ObjectValue {
        &self.value
    }
}

impl<'a, 'b> GraveyardIterator<'a, 'b> {
    async fn new(
        object_id: u64,
        iter: MergerIterator<'a, 'b, ObjectKey, ObjectValue>,
    ) -> Result<GraveyardIterator<'a, 'b>, Error> {
        let mut iter = GraveyardIterator { object_id, iter };
        iter.skip_deleted_entries().await?;
        Ok(iter)
    }

    async fn skip_deleted_entries(&mut self) -> Result<(), Error> {
        loop {
            match self.iter.get() {
                Some(ItemRef {
                    key: ObjectKey { object_id, .. },
                    value: ObjectValue::None,
                    ..
                }) if *object_id == self.object_id => {}
                _ => return Ok(()),
            }
            self.iter.advance().await?;
        }
    }

    pub fn get(&self) -> Option<GraveyardEntryInfo> {
        match self.iter.get() {
            Some(ItemRef {
                key: ObjectKey { object_id: oid, data: ObjectKeyData::GraveyardEntry { object_id } },
                value,
                ..
            }) if *oid == self.object_id => Some(GraveyardEntryInfo {
                object_id: *object_id,
                attribute_id: None,
                value: value.clone(),
            }),
            Some(ItemRef {
                key:
                    ObjectKey {
                        object_id: oid,
                        data: ObjectKeyData::GraveyardAttributeEntry { object_id, attribute_id },
                    },
                value,
                ..
            }) if *oid == self.object_id => Some(GraveyardEntryInfo {
                object_id: *object_id,
                attribute_id: Some(*attribute_id),
                value: value.clone(),
            }),
            _ => None,
        }
    }

    pub async fn advance(&mut self) -> Result<(), Error> {
        self.iter.advance().await?;
        self.skip_deleted_entries().await
    }
}

#[cfg(test)]
mod tests {
    use super::{Graveyard, GraveyardEntryInfo, ObjectStore};
    use crate::errors::FxfsError;
    use crate::filesystem::{FxFilesystem, FxFilesystemBuilder};
    use crate::fsck::fsck;
    use crate::hooks::Hooks;
    use crate::object_handle::ObjectHandle;
    use crate::object_store::data_object_handle::WRITE_ATTR_BATCH_SIZE;
    use crate::object_store::directory::Directory;
    use crate::object_store::journal::JournalOptions;
    use crate::object_store::object_record::{AttributeKey, ObjectValue};
    use crate::object_store::transaction::{Options, lock_keys};
    use crate::object_store::volume::root_volume;
    use crate::object_store::{
        AttributeId, HandleOptions, LockKey, Mutation, NewChildStoreOptions, ObjectKey, Timestamp,
    };
    use assert_matches::assert_matches;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;

    const TEST_DEVICE_BLOCK_SIZE: u32 = 512;

    #[fuchsia::test]
    async fn test_graveyard() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_store = fs.root_store();

        assert_eq!(root_store.graveyard_count(), 0);

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let handle1 = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await
        .expect("create_object failed");
        let handle2 = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await
        .expect("create_object failed");
        transaction.commit().await.expect("commit failed");
        let id1 = handle1.object_id();
        let id2 = handle2.object_id();

        // Create and add two objects to the graveyard.
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");

        root_store.add_to_graveyard(&mut transaction, id1);
        root_store.add_to_graveyard(&mut transaction, id2);
        transaction.commit().await.expect("commit failed");

        assert_eq!(root_store.graveyard_count(), 2);

        // Check that we see the objects we added.
        {
            let layer_set = root_store.tree().layer_set();
            let mut merger = layer_set.merger();
            let mut iter = Graveyard::iter(root_store.graveyard_directory_object_id(), &mut merger)
                .await
                .expect("iter failed");
            assert_matches!(
                iter.get().expect("missing entry"),
                GraveyardEntryInfo { object_id, attribute_id: None, value: ObjectValue::Some }
                if object_id == id1
            );
            iter.advance().await.expect("advance failed");
            assert_matches!(
                iter.get().expect("missing entry"),
                GraveyardEntryInfo { object_id, attribute_id: None, value: ObjectValue::Some }
                if object_id == id2
            );
            iter.advance().await.expect("advance failed");
            assert_eq!(iter.get(), None);
        }

        // Remove one of the objects.
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        root_store.remove_from_graveyard(&mut transaction, id2);
        transaction.commit().await.expect("commit failed");

        assert_eq!(root_store.graveyard_count(), 1);

        // Check that the graveyard has been updated as expected.
        let layer_set = root_store.tree().layer_set();
        let mut merger = layer_set.merger();
        let mut iter = Graveyard::iter(root_store.graveyard_directory_object_id(), &mut merger)
            .await
            .expect("iter failed");
        assert_matches!(
            iter.get().expect("missing entry"),
            GraveyardEntryInfo { object_id, attribute_id: None, value: ObjectValue::Some }
            if object_id == id1
        );
        iter.advance().await.expect("advance failed");
        assert_eq!(iter.get(), None);
    }

    #[fuchsia::test]
    async fn test_graveyard_count_replay() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let (device, _object_ids) = {
            let fs = FxFilesystemBuilder::new()
                .skip_initial_reap(true)
                .format(true)
                .open(device)
                .await
                .expect("open failed");
            let root_store = fs.root_store();

            let mut object_ids = Vec::new();
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let handle1 = ObjectStore::create_object(
                &root_store,
                &mut transaction,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("create_object failed");
            let handle2 = ObjectStore::create_object(
                &root_store,
                &mut transaction,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("create_object failed");
            transaction.commit().await.expect("commit failed");
            object_ids.push(handle1.object_id());
            object_ids.push(handle2.object_id());

            // Create and add two objects to the graveyard.
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");

            root_store.add_to_graveyard(&mut transaction, object_ids[0]);
            root_store.add_to_graveyard(&mut transaction, object_ids[1]);
            transaction.commit().await.expect("commit failed");

            assert_eq!(root_store.graveyard_count(), 2);
            fs.close().await.expect("close failed");
            (fs.take_device().await, object_ids)
        };
        device.reopen(false);
        let device = {
            let fs =
                FxFilesystemBuilder::new().read_only(true).open(device).await.expect("open failed");
            let root_store = fs.root_store();
            // Counter is 0 because initial_reap is not called for read-only mounts.
            assert_eq!(root_store.graveyard_count(), 0);

            // Now manually run it. This will count and queue (but the reaper isn't running).
            let count =
                fs.graveyard().initial_reap(&root_store).await.expect("initial_reap failed");
            let actual_count = root_store.graveyard_count();
            assert_eq!(count, 2, "initial_reap found wrong number of items (count={})", count);
            assert_eq!(
                actual_count, 2,
                "graveyard_count returned {} but initial_reap found {}",
                actual_count, count
            );

            fs.close().await.expect("close failed");
            fs.take_device().await
        };
        device.reopen(false);
        {
            // Now test the full flow where they are automatically reaped.
            let fs = FxFilesystem::open(device).await.expect("open failed");
            let root_store = fs.root_store();

            // They might or might not have been reaped yet.
            // Wait for the reaper to finish.
            fs.graveyard().wait_for_reap().await;

            // Now the count MUST be 0.
            assert_eq!(root_store.graveyard_count(), 0);
            fs.close().await.expect("close failed");
        }
    }

    #[fuchsia::test]
    async fn test_tombstone_attribute() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_store = fs.root_store();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");

        let handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await
        .expect("failed to create object");
        transaction.commit().await.expect("commit failed");

        handle
            .write_attr(AttributeId::TEST_ID, &[0; 8192])
            .await
            .expect("failed to write attribute");
        let object_id = handle.object_id();
        let mut transaction = handle.new_transaction().await.expect("new_transaction failed");
        transaction.add(
            root_store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::graveyard_attribute_entry(
                    root_store.graveyard_directory_object_id(),
                    object_id,
                    AttributeId::TEST_ID,
                ),
                ObjectValue::Some,
            ),
        );

        transaction.commit().await.expect("commit failed");

        fs.close().await.expect("failed to close filesystem");
        let device = fs.take_device().await;
        device.reopen(false);

        let fs =
            FxFilesystemBuilder::new().read_only(true).open(device).await.expect("open failed");
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("failed to close filesystem");
        let device = fs.take_device().await;
        device.reopen(false);

        // On open, the filesystem will call initial_reap which will call queue_tombstone().
        let fs = FxFilesystem::open(device).await.expect("open failed");
        // `wait_for_reap` ensures that the Message::Tombstone is actually processed.
        fs.graveyard().wait_for_reap().await;
        let root_store = fs.root_store();

        let handle =
            ObjectStore::open_object(&root_store, object_id, HandleOptions::default(), None)
                .await
                .expect("failed to open object");

        assert_eq!(handle.read_attr(AttributeId::TEST_ID).await.expect("read_attr failed"), None);
        fsck(fs.clone()).await.expect("fsck failed");
    }

    #[fuchsia::test]
    async fn test_tombstone_attribute_and_object() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_store = fs.root_store();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");

        let handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await
        .expect("failed to create object");
        transaction.commit().await.expect("commit failed");

        const ATTR_1: AttributeId = AttributeId::TEST_ID;
        const ATTR_2: AttributeId = AttributeId::TEST_ID.next();
        // With both of these it will test that both their graveyard entries got cleaned up in
        // trim_or_tombstone() via two different paths.
        handle.write_attr(ATTR_1, &[0; 8192]).await.expect("failed to write attribute");
        handle.write_attr(ATTR_2, &[0; 8192]).await.expect("failed to write attribute");
        let object_id = handle.object_id();
        let mut transaction = handle.new_transaction().await.expect("new_transaction failed");
        transaction.add(
            root_store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::graveyard_attribute_entry(
                    root_store.graveyard_directory_object_id(),
                    object_id,
                    ATTR_1,
                ),
                ObjectValue::Some,
            ),
        );
        transaction.add(
            root_store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::graveyard_attribute_entry(
                    root_store.graveyard_directory_object_id(),
                    object_id,
                    ATTR_2,
                ),
                ObjectValue::Some,
            ),
        );
        transaction.commit().await.expect("commit failed");
        let mut transaction = handle.new_transaction().await.expect("new_transaction failed");
        transaction.add(
            root_store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::graveyard_entry(root_store.graveyard_directory_object_id(), object_id),
                ObjectValue::Some,
            ),
        );
        transaction.commit().await.expect("commit failed");

        fs.close().await.expect("failed to close filesystem");
        let device = fs.take_device().await;
        device.reopen(false);

        let fs =
            FxFilesystemBuilder::new().read_only(true).open(device).await.expect("open failed");
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("failed to close filesystem");
        let device = fs.take_device().await;
        device.reopen(false);

        // On open, the filesystem will call initial_reap which will call queue_tombstone().
        let fs = FxFilesystem::open(device).await.expect("open failed");
        // `wait_for_reap` ensures that the two tombstone messages are processed.
        fs.graveyard().wait_for_reap().await;

        let root_store = fs.root_store();
        if let Err(e) =
            ObjectStore::open_object(&root_store, object_id, HandleOptions::default(), None).await
        {
            assert!(FxfsError::NotFound.matches(&e));
        } else {
            panic!("open_object succeeded");
        };
        fsck(fs.clone()).await.expect("fsck failed");
    }

    #[fuchsia::test]
    async fn test_tombstone_large_attribute() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_store = fs.root_store();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");

        let handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await
        .expect("failed to create object");
        transaction.commit().await.expect("commit failed");

        let object_id = {
            let mut transaction = handle.new_transaction().await.expect("new_transaction failed");
            transaction.add(
                root_store.store_object_id(),
                Mutation::replace_or_insert_object(
                    ObjectKey::graveyard_attribute_entry(
                        root_store.graveyard_directory_object_id(),
                        handle.object_id(),
                        AttributeId::TEST_ID,
                    ),
                    ObjectValue::Some,
                ),
            );

            // This write should span three transactions. This test mimics the behavior when the
            // last transaction gets interrupted by a filesystem.close().
            handle
                .write_new_attr_in_batches(
                    &mut transaction,
                    AttributeId::TEST_ID,
                    &vec![0; 3 * WRITE_ATTR_BATCH_SIZE],
                    WRITE_ATTR_BATCH_SIZE,
                )
                .await
                .expect("failed to write attribute");

            handle.object_id()
            // Drop the transaction to simulate interrupting the attribute creation as well as to
            // release the transaction locks.
        };

        fs.close().await.expect("failed to close filesystem");
        let device = fs.take_device().await;
        device.reopen(false);

        let fs =
            FxFilesystemBuilder::new().read_only(true).open(device).await.expect("open failed");
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("failed to close filesystem");
        let device = fs.take_device().await;
        device.reopen(false);

        // On open, the filesystem will call initial_reap which will call queue_tombstone().
        let fs = FxFilesystem::open(device).await.expect("open failed");
        // `wait_for_reap` ensures that the two tombstone messages are processed.
        fs.graveyard().wait_for_reap().await;

        let root_store = fs.root_store();

        let handle =
            ObjectStore::open_object(&root_store, object_id, HandleOptions::default(), None)
                .await
                .expect("failed to open object");

        assert_eq!(handle.read_attr(AttributeId::TEST_ID).await.expect("read_attr failed"), None);
        fsck(fs.clone()).await.expect("fsck failed");
    }

    async fn populate_store(store: &ObjectStore, graveyard: bool, object_count: usize) {
        let now = Timestamp::now();
        let graveyard_id = store.graveyard_directory_object_id();
        let batch_size = 100;
        for batch_start in (0..object_count).step_by(batch_size) {
            let mut transaction = store
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let count = std::cmp::min(batch_size, object_count - batch_start);
            for i in 0..count {
                let oid = 1000 + (batch_start + i) as u64;
                if graveyard {
                    transaction.add(
                        store.store_object_id(),
                        Mutation::insert_object(
                            ObjectKey::graveyard_entry(graveyard_id, oid),
                            ObjectValue::Some,
                        ),
                    );
                }
                transaction.add(
                    store.store_object_id(),
                    Mutation::insert_object(
                        ObjectKey::object(oid),
                        ObjectValue::file(1, 0, now, now, now, now, None, None),
                    ),
                );
                transaction.add(
                    store.store_object_id(),
                    Mutation::insert_object(
                        ObjectKey::attribute(oid, AttributeId::DATA, AttributeKey::Attribute),
                        ObjectValue::attribute(0, false),
                    ),
                );
            }
            transaction.commit().await.expect("commit failed");
        }
        store.flush().await.expect("flush failed");
    }

    // This test verifies that graveyard operations check for journal space before committing
    // transactions (skip_journal_checks: false). If compactions are paused, graveyard operations
    // will throttle when journal space runs low and wait for compactions to free space (invoking
    // the check space callback), rather than borrowing unbounded metadata space.
    #[fuchsia::test]
    async fn test_graveyard_borrowed_metadata_space_fails_compaction() {
        let reclaim_size = 65536;
        // 200,000 blocks of 512 bytes = 100 MiB device.
        let device = DeviceHolder::new(FakeDevice::new(200000, TEST_DEVICE_BLOCK_SIZE));
        let (mut hooks, fs_hooks) = Hooks::new();
        let fs = FxFilesystemBuilder::new()
            .hooks(fs_hooks)
            .journal_options(JournalOptions { reclaim_size, ..Default::default() })
            .format(true)
            .open(device)
            .await
            .expect("open failed");

        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let num_stores = 9;
        let objects_per_store = 2000;
        let mut stores = Vec::with_capacity(num_stores);

        for s in 0..num_stores {
            let store = root_volume
                .new_volume(&format!("test_{s}"), NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");
            stores.push(store);
        }

        // Fill stores 0..n-1 with objects and graveyard markers for them.
        for store in &stores[0..num_stores - 1] {
            populate_store(store, true, objects_per_store).await;
        }

        // Last store is populated with surviving objects.
        let store_last = &stores[num_stores - 1];
        populate_store(store_last, false, objects_per_store).await;

        // Force a compaction so the journal is empty before compactions are paused.
        fs.journal().force_compact().await.expect("force_compact failed");

        // Pause compactions so the graveyard would accumulate journal usage past the reclaim limit
        // if compactions remain paused.
        fs.journal().pause_compactions().await;

        // Set a callback which will automatically unblock the journal and resume compactions when
        // waiting for journal space. The idea is that we don't want background compactions to run
        // while we are flushing the graveyard, but if the graveyard would block on the journal, we
        // want to unblock it.
        let journal_clone = fs.journal().clone();
        hooks.set_waiting_for_journal_space(move || {
            journal_clone.resume_compactions();
        });

        for store in &stores[0..num_stores - 1] {
            let store_id = store.store_object_id();
            for oid in 1000..1000 + objects_per_store as u64 {
                fs.graveyard().queue_tombstone_object(store_id, oid);
            }
        }
        fs.graveyard().flush().await;

        // Perform a transaction to store_last to trigger compaction.  If the graveyard properly
        // blocked on journal space, that should have triggered a blocking compaction and there
        // should be enough space.  If the graveyard did not ever block on journal space, this will
        // fail because there will be too much borrowed metadata space.
        let root_dir_last = Directory::open(store_last, store_last.root_directory_object_id())
            .await
            .expect("open root_dir_last failed");
        let mut transaction = store_last
            .new_transaction(
                lock_keys![LockKey::object(
                    store_last.store_object_id(),
                    root_dir_last.object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        root_dir_last.create_child_file(&mut transaction, "trigger").await.expect("create failed");
        transaction.commit().await.expect("commit failed");

        fs.close().await.expect("close failed");
    }
}
