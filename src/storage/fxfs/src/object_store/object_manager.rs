// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::FxfsError;
use crate::filesystem::{ApplyContext, ApplyMode, FlushReason, JournalingObject};
use crate::log::*;
use crate::metrics;
use crate::object_handle::INVALID_OBJECT_ID;
use crate::object_store::allocator::{Allocator, Reservation};
use crate::object_store::directory::Directory;
use crate::object_store::journal::{self, JournalCheckpoint};
use crate::object_store::transaction::{
    AssocObj, AssociatedObject, MetadataReservation, Mutation, ObjectMutationIterator, Transaction,
    TxnMutation,
};
use crate::object_store::tree_cache::TreeCache;
use crate::object_store::volume::{VOLUMES_DIRECTORY, list_volumes};
use crate::object_store::{ObjectDescriptor, ObjectStore};
use crate::round::round_div;
use crate::serialized_types::{LATEST_VERSION, Version};
use anyhow::{Context, Error, anyhow, bail, ensure};
use fuchsia_inspect::{Property as _, UintProperty};
use fuchsia_sync::RwLock;
use futures::FutureExt as _;
use rustc_hash::FxHashMap as HashMap;
use std::collections::hash_map::Entry;
use std::num::Saturating;
use std::sync::{Arc, OnceLock};

// Data written to the journal eventually needs to be flushed somewhere (typically into layer
// files).  Here we conservatively assume that could take up to four times as much space as it does
// in the journal.  In the layer file, it'll take up at least as much, but we must reserve the same
// again that so that there's enough space for compactions, and then we need some spare for
// overheads.
//
// TODO(https://fxbug.dev/42178158): We should come up with a better way of determining what the multiplier
// should be here.  2x was too low, as it didn't cover any space for metadata.  4x might be too
// much.
pub const fn reserved_space_from_journal_usage(journal_usage: u64) -> u64 {
    journal_usage * 4
}

/// ObjectManager is a global loading cache for object stores and other special objects.
pub struct ObjectManager {
    inner: RwLock<Inner>,
    metadata_reservation: OnceLock<Reservation>,
    volume_directory: OnceLock<Directory<ObjectStore>>,
    on_new_store: Option<Box<dyn Fn(&ObjectStore) + Send + Sync>>,
}

// Whilst we are flushing we need to keep track of the old checkpoint that we are hoping to flush,
// and a new one that should apply if we successfully finish the flush.
#[derive(Debug)]
enum Checkpoints {
    Current(JournalCheckpoint),
    Old(JournalCheckpoint),
    Both(/* old: */ JournalCheckpoint, /* current: */ JournalCheckpoint),
}

impl Checkpoints {
    // Returns the earliest checkpoint (which will always be the old one if present).
    fn earliest(&self) -> &JournalCheckpoint {
        match self {
            Checkpoints::Old(x) | Checkpoints::Both(x, _) | Checkpoints::Current(x) => x,
        }
    }
}

// We currently maintain strong references to all stores that have been opened, but there's no
// currently no mechanism for releasing stores that aren't being used.
struct Inner {
    stores: HashMap<u64, Arc<ObjectStore>>,
    root_parent_store_object_id: u64,
    root_store_object_id: u64,
    allocator_object_id: u64,
    allocator: Option<Arc<Allocator>>,

    // Records dependencies on the journal for objects i.e. an entry for object ID 1, would mean it
    // has a dependency on journal records from that offset.
    journal_checkpoints: HashMap<u64, Checkpoints>,

    // Mappings from object-id to a target reservation amount.  The object IDs here are from the
    // root store namespace, so it can be associated with any object in the root store.  A
    // reservation will be made to cover the *maximum* in this map, since it is assumed that any
    // requirement is only temporary, for the duration of a compaction, and that once compaction has
    // finished for a particular object, the space will be recovered.
    reservations: HashMap<u64, u64>,

    // The last journal end offset for a transaction that has been applied.  This is not necessarily
    // the same as the start offset for the next transaction because of padding.
    last_end_offset: u64,

    // A running counter that tracks metadata space that has been borrowed on the understanding that
    // eventually it will be recovered (potentially after a full compaction).
    borrowed_metadata_space: u64,

    // The maximum transaction size that has been encountered so far.
    max_transaction_size: (u64, UintProperty),

    // Extra temporary space that might be tied up in the journal that hasn't yet been deallocated.
    reserved_space: u64,
}

impl Inner {
    fn earliest_journal_offset(&self) -> Option<u64> {
        self.journal_checkpoints.values().map(|c| c.earliest().file_offset).min()
    }

    // Returns the required size of the metadata reservation assuming that no space has been
    // borrowed.  The invariant is: reservation-size + borrowed-space = required.
    fn required_reservation(&self) -> u64 {
        // Start with the maximum amount of temporary space we might need during compactions.
        self.reservations.values().max().unwrap_or(&0)

        // Account for data that has been written to the journal that will need to be written
        // to layer files when flushed.
            + self.earliest_journal_offset()
            .map(|min| reserved_space_from_journal_usage(self.last_end_offset - min))
            .unwrap_or(0)

        // Extra reserved space
            + self.reserved_space
    }

    fn journaling_object(&self, object_id: u64) -> Option<Arc<dyn JournalingObject>> {
        if object_id == self.allocator_object_id {
            Some(self.allocator.clone().unwrap() as Arc<dyn JournalingObject>)
        } else {
            self.stores.get(&object_id).map(|x| x.clone() as Arc<dyn JournalingObject>)
        }
    }
}

impl ObjectManager {
    pub fn new(on_new_store: Option<Box<dyn Fn(&ObjectStore) + Send + Sync>>) -> ObjectManager {
        ObjectManager {
            inner: RwLock::new(Inner {
                stores: HashMap::default(),
                root_parent_store_object_id: INVALID_OBJECT_ID,
                root_store_object_id: INVALID_OBJECT_ID,
                allocator_object_id: INVALID_OBJECT_ID,
                allocator: None,
                journal_checkpoints: HashMap::default(),
                reservations: HashMap::default(),
                last_end_offset: 0,
                borrowed_metadata_space: 0,
                max_transaction_size: (0, metrics::detail().create_uint("max_transaction_size", 0)),
                reserved_space: journal::RESERVED_SPACE,
            }),
            metadata_reservation: OnceLock::new(),
            volume_directory: OnceLock::new(),
            on_new_store,
        }
    }

    pub fn required_reservation(&self) -> u64 {
        self.inner.read().required_reservation()
    }

    pub fn root_parent_store_object_id(&self) -> u64 {
        self.inner.read().root_parent_store_object_id
    }

    pub fn root_parent_store(&self) -> Arc<ObjectStore> {
        let inner = self.inner.read();
        inner.stores.get(&inner.root_parent_store_object_id).unwrap().clone()
    }

    pub fn set_root_parent_store(&self, store: Arc<ObjectStore>) {
        if let Some(on_new_store) = &self.on_new_store {
            on_new_store(&store);
        }
        let mut inner = self.inner.write();
        let store_id = store.store_object_id();
        inner.stores.insert(store_id, store);
        inner.root_parent_store_object_id = store_id;
    }

    pub fn root_store_object_id(&self) -> u64 {
        self.inner.read().root_store_object_id
    }

    pub fn root_store(&self) -> Arc<ObjectStore> {
        let inner = self.inner.read();
        inner.stores.get(&inner.root_store_object_id).unwrap().clone()
    }

    pub fn set_root_store(&self, store: Arc<ObjectStore>) {
        if let Some(on_new_store) = &self.on_new_store {
            on_new_store(&store);
        }
        let mut inner = self.inner.write();
        let store_id = store.store_object_id();
        inner.stores.insert(store_id, store);
        inner.root_store_object_id = store_id;
    }

    pub fn is_system_store(&self, store_id: u64) -> bool {
        let inner = self.inner.read();
        store_id == inner.root_store_object_id || store_id == inner.root_parent_store_object_id
    }

    /// Returns the store which might or might not be locked.
    pub fn store(&self, store_object_id: u64) -> Option<Arc<ObjectStore>> {
        self.inner.read().stores.get(&store_object_id).cloned()
    }

    /// Returns the total bytes written to the LSM trees in the allocator and all object stores
    /// during compaction operations.
    pub fn compaction_bytes_written(&self) -> u64 {
        let inner = self.inner.read();
        let mut total = 0;
        if let Some(allocator) = &inner.allocator {
            total += allocator.tree().compaction_bytes_written();
        }
        for store in inner.stores.values() {
            total += store.tree().compaction_bytes_written();
        }
        total
    }

    /// This is not thread-safe: it assumes that a store won't be forgotten whilst the loop is
    /// running.  This is to be used after replaying the journal.
    pub async fn on_replay_complete(&self) -> Result<(), Error> {
        let root_store = self.root_store();

        let root_directory = Directory::open(&root_store, root_store.root_directory_object_id())
            .await
            .context("Unable to open root volume directory")?;

        match root_directory.lookup(VOLUMES_DIRECTORY).await? {
            None => bail!("Root directory not found"),
            Some((object_id, ObjectDescriptor::Directory, _)) => {
                let volume_directory = Directory::open(&root_store, object_id)
                    .await
                    .context("Unable to open volumes directory")?;
                self.volume_directory.set(volume_directory).unwrap();
            }
            _ => {
                bail!(
                    anyhow!(FxfsError::Inconsistent)
                        .context("Unexpected type for volumes directory")
                )
            }
        }

        let object_ids = list_volumes(self.volume_directory.get().unwrap())
            .await
            .context("Failed to list volumes")?;

        for store_id in object_ids {
            self.open_store(&root_store, store_id).await?;
        }

        // This can fail if a filesystem is created and truncated to a size
        // that doesn't leave enough free space for metadata reservations.
        self.init_metadata_reservation()
            .context("Insufficient free space for metadata reservation.")?;

        Ok(())
    }

    pub fn volume_directory(&self) -> &Directory<ObjectStore> {
        self.volume_directory.get().unwrap()
    }

    pub fn set_volume_directory(&self, volume_directory: Directory<ObjectStore>) {
        self.volume_directory.set(volume_directory).unwrap();
    }

    pub fn add_store(&self, store: Arc<ObjectStore>) {
        if let Some(on_new_store) = &self.on_new_store {
            on_new_store(&store);
        }
        let mut inner = self.inner.write();
        let store_object_id = store.store_object_id();
        assert_ne!(store_object_id, inner.root_parent_store_object_id);
        assert_ne!(store_object_id, inner.root_store_object_id);
        assert_ne!(store_object_id, inner.allocator_object_id);
        inner.stores.insert(store_object_id, store);
    }

    pub fn forget_store(&self, store_object_id: u64) {
        let mut inner = self.inner.write();
        assert_ne!(store_object_id, inner.allocator_object_id);
        inner.stores.remove(&store_object_id);
        inner.reservations.remove(&store_object_id);
    }

    pub fn set_allocator(&self, allocator: Arc<Allocator>) {
        let mut inner = self.inner.write();
        assert!(!inner.stores.contains_key(&allocator.object_id()));
        inner.allocator_object_id = allocator.object_id();
        inner.allocator = Some(allocator);
    }

    pub fn allocator(&self) -> Arc<Allocator> {
        self.inner.read().allocator.clone().unwrap()
    }

    /// Applies `mutation` to `object` with `context`.
    pub fn apply_mutation(
        &self,
        object_id: u64,
        mutation: Mutation,
        context: &ApplyContext<'_, '_>,
        associated_object: AssocObj<'_>,
    ) -> Result<(), Error> {
        debug!(oid = object_id, mutation:?; "applying mutation");
        let object = {
            let mut inner = self.inner.write();
            match mutation {
                Mutation::BeginFlush => {
                    if let Some(entry) = inner.journal_checkpoints.get_mut(&object_id) {
                        match entry {
                            Checkpoints::Current(x) | Checkpoints::Both(x, _) => {
                                *entry = Checkpoints::Old(x.clone());
                            }
                            _ => {}
                        }
                    }
                }
                Mutation::EndFlush => {
                    if let Entry::Occupied(mut o) = inner.journal_checkpoints.entry(object_id) {
                        let entry = o.get_mut();
                        match entry {
                            Checkpoints::Old(_) => {
                                o.remove();
                            }
                            Checkpoints::Both(_, x) => {
                                *entry = Checkpoints::Current(x.clone());
                            }
                            _ => {}
                        }
                    }
                }
                Mutation::DeleteVolume => {
                    if let Some(store) = inner.stores.remove(&object_id) {
                        store.mark_deleted();
                    }
                    inner.reservations.remove(&object_id);
                    inner.journal_checkpoints.remove(&object_id);
                    return Ok(());
                }
                _ => {
                    if object_id != inner.root_parent_store_object_id {
                        inner
                            .journal_checkpoints
                            .entry(object_id)
                            .and_modify(|entry| {
                                if let Checkpoints::Old(x) = entry {
                                    *entry =
                                        Checkpoints::Both(x.clone(), context.checkpoint.clone());
                                }
                            })
                            .or_insert_with(|| Checkpoints::Current(context.checkpoint.clone()));
                    }
                }
            }
            inner.journaling_object(object_id).unwrap()
        };
        associated_object.map(|o| o.will_apply_mutation(&mutation, object_id, self));
        object.apply_mutation(mutation, context, associated_object)
    }

    /// Replays `mutations` for a single transaction.  `journal_offsets` contains the per-object
    /// starting offsets; if the current transaction offset precedes an offset, the mutations for
    /// that object are ignored.  `context` contains the location in the journal file for this
    /// transaction and `end_offset` is the ending journal offset for this transaction.
    pub async fn replay_mutations(
        &self,
        mutations: Vec<(u64, Mutation)>,
        journal_offsets: &HashMap<u64, u64>,
        context: &ApplyContext<'_, '_>,
        end_offset: u64,
    ) -> Result<(), Error> {
        debug!(checkpoint = context.checkpoint.file_offset; "REPLAY");
        let txn_size = {
            let mut inner = self.inner.write();
            if end_offset > inner.last_end_offset {
                Some(end_offset - std::mem::replace(&mut inner.last_end_offset, end_offset))
            } else {
                None
            }
        };

        let allocator_object_id = self.inner.read().allocator_object_id;

        for (object_id, mutation) in mutations {
            if let Mutation::UpdateBorrowed(borrowed) = mutation {
                if let Some(txn_size) = txn_size {
                    self.inner.write().borrowed_metadata_space = borrowed
                        .checked_add(reserved_space_from_journal_usage(txn_size))
                        .ok_or(FxfsError::Inconsistent)?;
                }
                continue;
            }

            // Don't replay mutations if the object doesn't want it.
            if let Some(&offset) = journal_offsets.get(&object_id) {
                if context.checkpoint.file_offset < offset {
                    continue;
                }
            }

            // If this is the first time we've encountered this store, we'll need to open it.
            if object_id != allocator_object_id {
                self.open_store(&self.root_store(), object_id).await?;
            }

            self.apply_mutation(object_id, mutation, context, AssocObj::None)?;
        }
        Ok(())
    }

    /// Opens the specified store if it isn't already.  This is *not* thread-safe.
    async fn open_store(&self, parent: &Arc<ObjectStore>, object_id: u64) -> Result<(), Error> {
        if self.inner.read().stores.contains_key(&object_id) {
            return Ok(());
        }
        let store = ObjectStore::open(parent, object_id, Some(Box::new(TreeCache::new())))
            .await
            .with_context(|| format!("Failed to open store {object_id}"))?;
        if let Some(on_new_store) = &self.on_new_store {
            on_new_store(&store);
        }
        assert!(self.inner.write().stores.insert(object_id, store).is_none());
        Ok(())
    }

    /// Called by the journaling system to apply a transaction.  `checkpoint` indicates the location
    /// in the journal file for this transaction.  Returns an optional mutation to be written to be
    /// included with the transaction.
    pub fn apply_transaction(
        &self,
        transaction: &mut Transaction<'_>,
        checkpoint: &JournalCheckpoint,
    ) -> Result<Option<Mutation>, Error> {
        // Record old values so we can see what changes as a result of this transaction.
        let old_amount = self.metadata_reservation().amount();
        let old_required = self.inner.read().required_reservation();

        debug!(checkpoint = checkpoint.file_offset; "BEGIN TXN");
        let mutations = transaction.take_mutations();
        let context =
            ApplyContext { mode: ApplyMode::Live(transaction), checkpoint: checkpoint.clone() };
        for TxnMutation { object_id, mutation, associated_object, .. } in mutations {
            self.apply_mutation(object_id, mutation, &context, associated_object)?;
        }
        debug!("END TXN");

        Ok(if let MetadataReservation::Borrowed = transaction.metadata_reservation {
            // If this transaction is borrowing metadata, figure out what has changed and return a
            // mutation with the updated value for borrowed.  The transaction might have allocated
            // or deallocated some data from the metadata reservation, or it might have made a
            // change that means we need to reserve more or less space (e.g. we compacted).
            let new_amount = self.metadata_reservation().amount();
            let mut inner = self.inner.write();
            let new_required = inner.required_reservation();
            let add = old_amount + new_required;
            let sub = new_amount + old_required;
            if add >= sub {
                inner.borrowed_metadata_space += add - sub;
            } else {
                inner.borrowed_metadata_space =
                    inner.borrowed_metadata_space.saturating_sub(sub - add);
            }
            Some(Mutation::UpdateBorrowed(inner.borrowed_metadata_space))
        } else {
            // This transaction should have had no impact on the metadata reservation or the amount
            // we need to reserve.
            debug_assert_eq!(self.metadata_reservation().amount(), old_amount);
            debug_assert_eq!(self.inner.read().required_reservation(), old_required);
            None
        })
    }

    /// Called by the journaling system after a transaction has been written providing the end
    /// offset for the transaction so that we can adjust borrowed metadata space accordingly.
    pub fn did_commit_transaction(
        &self,
        transaction: &mut Transaction<'_>,
        _checkpoint: &JournalCheckpoint,
        end_offset: u64,
    ) {
        let reservation = self.metadata_reservation();
        let mut inner = self.inner.write();
        let journal_usage = end_offset - std::mem::replace(&mut inner.last_end_offset, end_offset);

        if journal_usage > inner.max_transaction_size.0 {
            inner.max_transaction_size.0 = journal_usage;
            inner.max_transaction_size.1.set(journal_usage);
        }

        let txn_space = reserved_space_from_journal_usage(journal_usage);
        match &mut transaction.metadata_reservation {
            MetadataReservation::None => unreachable!(),
            MetadataReservation::Borrowed => {
                // Account for the amount we need to borrow for the transaction itself now that we
                // know the transaction size.
                inner.borrowed_metadata_space += txn_space;

                // This transaction borrowed metadata space, but it might have returned space to the
                // transaction that we can now give back to the allocator.
                let to_give_back = (reservation.amount() + inner.borrowed_metadata_space)
                    .saturating_sub(inner.required_reservation());
                if to_give_back > 0 {
                    reservation.give_back(to_give_back);
                }
            }
            MetadataReservation::Hold(hold_amount) => {
                // Transfer reserved space into the metadata reservation.
                let txn_reservation = transaction.allocator_reservation.unwrap();
                assert_ne!(
                    txn_reservation as *const _, reservation as *const _,
                    "MetadataReservation::Borrowed should be used."
                );
                txn_reservation.commit(txn_space);
                if txn_reservation.owner_object_id() != reservation.owner_object_id() {
                    assert_eq!(
                        reservation.owner_object_id(),
                        None,
                        "Should not be mixing attributed owners."
                    );
                    inner
                        .allocator
                        .as_ref()
                        .unwrap()
                        .disown_reservation(txn_reservation.owner_object_id(), txn_space);
                }
                if let Some(amount) = hold_amount.checked_sub(txn_space) {
                    *hold_amount = amount;
                } else {
                    panic!("Transaction was larger than metadata reservation");
                }
                reservation.add(txn_space);
            }
            MetadataReservation::Reservation(txn_reservation) => {
                // Transfer reserved space into the metadata reservation.
                txn_reservation.move_to(reservation, txn_space);
            }
        }
        // Check that our invariant holds true.
        debug_assert_eq!(
            reservation.amount() + inner.borrowed_metadata_space,
            inner.required_reservation(),
            "txn_space: {}, reservation_amount: {}, borrowed: {}, required: {}",
            txn_space,
            reservation.amount(),
            inner.borrowed_metadata_space,
            inner.required_reservation(),
        );
    }

    /// Drops a transaction.  This is called automatically when a transaction is dropped.  If the
    /// transaction has been committed, it should contain no mutations and so nothing will get rolled
    /// back.  For each mutation, drop_mutation is called to allow for roll back (e.g. the allocator
    /// will unreserve allocations).
    pub fn drop_transaction(&self, transaction: &mut Transaction<'_>) {
        for TxnMutation { object_id, mutation, .. } in transaction.take_mutations() {
            self.journaling_object(object_id).map(|o| o.drop_mutation(mutation, transaction));
        }
    }

    /// Returns the journal file offsets that each object depends on and the checkpoint for the
    /// minimum offset.
    pub fn journal_file_offsets(&self) -> (HashMap<u64, u64>, Option<JournalCheckpoint>) {
        let inner = self.inner.read();
        let mut min_checkpoint = None;
        let mut offsets = HashMap::default();
        for (&object_id, checkpoint) in &inner.journal_checkpoints {
            let checkpoint = checkpoint.earliest();
            match &mut min_checkpoint {
                None => min_checkpoint = Some(checkpoint),
                Some(min_checkpoint) => {
                    if checkpoint.file_offset < min_checkpoint.file_offset {
                        *min_checkpoint = checkpoint;
                    }
                }
            }
            offsets.insert(object_id, checkpoint.file_offset);
        }
        (offsets, min_checkpoint.cloned())
    }

    /// Returns the checkpoint into the journal that the object depends on, or None if the object
    /// has no journaled updates.
    pub fn journal_checkpoint(&self, object_id: u64) -> Option<JournalCheckpoint> {
        self.inner
            .read()
            .journal_checkpoints
            .get(&object_id)
            .map(|checkpoints| checkpoints.earliest().clone())
    }

    /// Returns true if the object identified by `object_id` is known to have updates recorded in
    /// the journal that the object depends upon.
    pub fn needs_flush(&self, object_id: u64) -> bool {
        self.inner.read().journal_checkpoints.contains_key(&object_id)
    }

    /// Flushes changes to device with the given reason.
    /// Also returns the earliest known version of a struct on the filesystem.
    pub async fn flush(&self, reason: FlushReason) -> Result<Version, Error> {
        let objects = {
            let inner = self.inner.read();
            let mut object_ids = inner.journal_checkpoints.keys().cloned().collect::<Vec<_>>();
            // Process objects in reverse sorted order because that will mean we compact the root
            // object store last which will ensure we include the metadata from the compactions of
            // other objects.
            object_ids.sort_unstable();
            object_ids
                .iter()
                .rev()
                .filter_map(|oid| inner.journaling_object(*oid).map(|obj| (*oid, obj)))
                .collect::<Vec<_>>()
        };

        // As we iterate, keep track of the earliest version used by structs in these objects
        let mut earliest_version: Version = LATEST_VERSION;
        for (object_id, object) in objects {
            let object_earliest_version = object
                .flush(reason)
                .await
                .with_context(|| format!("Failed to flush oid {object_id}"))?;
            if object_earliest_version < earliest_version {
                earliest_version = object_earliest_version;
            }
        }

        Ok(earliest_version)
    }

    pub fn journaling_object(&self, object_id: u64) -> Option<Arc<dyn JournalingObject>> {
        self.inner.read().journaling_object(object_id)
    }

    pub fn init_metadata_reservation(&self) -> Result<(), Error> {
        if self.root_parent_store().filesystem().options().read_only {
            // We don't need metadata reservations in read-only mode.
            return Ok(());
        }
        let inner = self.inner.read();
        let required = inner.required_reservation();
        ensure!(required >= inner.borrowed_metadata_space, FxfsError::Inconsistent);
        let allocator = inner.allocator.as_ref().cloned().unwrap();
        self.metadata_reservation
            .set(
                allocator
                    .clone()
                    .reserve(None, inner.required_reservation() - inner.borrowed_metadata_space)
                    .with_context(|| {
                        format!(
                            "Failed to reserve {} - {} = {} bytes, free={}, \
                             owner_bytes={}",
                            inner.required_reservation(),
                            inner.borrowed_metadata_space,
                            inner.required_reservation() - inner.borrowed_metadata_space,
                            Saturating(allocator.get_disk_bytes()) - allocator.get_used_bytes(),
                            allocator.owner_bytes_debug(),
                        )
                    })?,
            )
            .unwrap();
        Ok(())
    }

    pub fn metadata_reservation(&self) -> &Reservation {
        self.metadata_reservation.get().unwrap()
    }

    pub fn is_metadata_reservation(&self, reservation: &Reservation) -> bool {
        self.metadata_reservation.get().is_some_and(|r| std::ptr::eq(r, reservation))
    }

    pub fn update_reservation(&self, object_id: u64, amount: u64) {
        self.inner.write().reservations.insert(object_id, amount);
    }

    pub fn reservation(&self, object_id: u64) -> Option<u64> {
        self.inner.read().reservations.get(&object_id).cloned()
    }

    /// Returns the maximum reservation held by any object (the amount budgeted for compaction).
    pub fn max_store_reservation(&self) -> u64 {
        self.inner.read().reservations.values().max().cloned().unwrap_or(0)
    }

    pub fn set_reserved_space(&self, amount: u64) {
        self.inner.write().reserved_space = amount;
    }

    pub fn last_end_offset(&self) -> u64 {
        self.inner.read().last_end_offset
    }

    pub fn set_last_end_offset(&self, v: u64) {
        self.inner.write().last_end_offset = v;
    }

    pub fn borrowed_metadata_space(&self) -> u64 {
        self.inner.read().borrowed_metadata_space
    }

    pub fn set_borrowed_metadata_space(&self, v: u64) {
        self.inner.write().borrowed_metadata_space = v;
    }

    pub fn write_mutations(
        &self,
        object_id: u64,
        mutations: ObjectMutationIterator<'_, '_>,
        writer: journal::Writer<'_>,
    ) {
        self.journaling_object(object_id).unwrap().write_mutations(mutations, writer);
    }

    pub fn unlocked_stores(&self) -> Vec<Arc<ObjectStore>> {
        let inner = self.inner.read();
        let mut stores = Vec::new();
        for store in inner.stores.values() {
            if !store.is_locked() {
                stores.push(store.clone());
            }
        }
        stores
    }

    /// Creates a lazy inspect node named `str` under `parent` which will yield statistics for the
    /// object manager when queried.
    pub fn track_statistics(self: &Arc<Self>, parent: &fuchsia_inspect::Node, name: &str) {
        let this = Arc::downgrade(self);
        parent.record_lazy_child(name, move || {
            let this_clone = this.clone();
            async move {
                let inspector = fuchsia_inspect::Inspector::default();
                if let Some(this) = this_clone.upgrade() {
                    let (required, borrowed, earliest_checkpoint) = {
                        // TODO(https://fxbug.dev/42069513): Push-back or rate-limit to prevent DoS.
                        let inner = this.inner.read();
                        (
                            inner.required_reservation(),
                            inner.borrowed_metadata_space,
                            inner.earliest_journal_offset(),
                        )
                    };
                    let root = inspector.root();
                    if let Some(reservation) = this.metadata_reservation.get() {
                        root.record_uint("metadata_reservation", reservation.amount());
                    }
                    root.record_uint("required_reservation", required);
                    root.record_uint("borrowed_reservation", borrowed);
                    if let Some(earliest_checkpoint) = earliest_checkpoint {
                        root.record_uint("earliest_checkpoint", earliest_checkpoint);
                    }

                    // TODO(https://fxbug.dev/42068224): Post-compute rather than manually computing metrics.
                    if let Some(x) = round_div(100 * borrowed, required) {
                        root.record_uint("borrowed_to_required_reservation_percent", x);
                    }
                }
                Ok(inspector)
            }
            .boxed()
        });
    }

    /// Normally, we make new transactions pay for overheads incurred by the journal, such as
    /// checksums and padding, but if the journal has discarded a significant amount after a replay,
    /// we run the risk of there not being enough reserved.  To handle this, if the amount is
    /// significant, we force the journal to borrow the space (using a journal created transaction).
    pub fn needs_borrow_for_journal(&self, checkpoint: u64) -> bool {
        checkpoint.checked_sub(self.inner.read().last_end_offset).unwrap() > 256
    }
}

/// ReservationUpdate is an associated object that sets the amount reserved for an object
/// (overwriting any previous amount). Updates must be applied as part of a transaction before
/// did_commit_transaction runs because it will reconcile the accounting for reserved metadata
/// space.
pub struct ReservationUpdate(u64);

impl ReservationUpdate {
    pub fn new(amount: u64) -> Self {
        Self(amount)
    }
}

impl AssociatedObject for ReservationUpdate {
    fn will_apply_mutation(&self, _mutation: &Mutation, object_id: u64, manager: &ObjectManager) {
        manager.update_reservation(object_id, self.0);
    }
}
