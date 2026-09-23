// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::checksum::Checksum;
use crate::filesystem::FxFilesystem;
use crate::log::*;
use crate::lsm_tree::types::Item;
use crate::object_handle::INVALID_OBJECT_ID;
use crate::object_store::allocator::{AllocatorItem, Reservation};
use crate::object_store::object_manager::{ObjectManager, reserved_space_from_journal_usage};
use crate::object_store::object_record::{
    FxfsKey, FxfsKeyV49, ObjectItem, ObjectItemV56, ObjectKey, ObjectKeyData, ObjectValue,
    ProjectProperty,
};
use crate::object_store::{AttributeId, AttributeKey, ProjectId};
use crate::serialized_types::{Migrate, Versioned, migrate_to_version};
use anyhow::Error;
use either::{Either, Left, Right};
use fprint::TypeFingerprint;
use fuchsia_sync::Mutex;
use futures::future::poll_fn;
use futures::pin_mut;
use rustc_hash::FxHashMap as HashMap;
use scopeguard::ScopeGuard;
use serde::{Deserialize, Serialize};
use std::cell::UnsafeCell;
use std::cmp::Ordering;
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, btree_set};
use std::iter::Peekable;
use std::marker::PhantomPinned;
use std::ops::{Deref, DerefMut, Range};
use std::sync::Arc;
use std::task::{Poll, Waker};
use std::{fmt, mem};

/// This allows for special handling of certain transactions such as deletes and the
/// extension of Journal extents. For most other use cases it is appropriate to use
/// `default()` here.
#[derive(Clone, Copy, Default)]
pub struct Options<'a> {
    /// If true, don't check for low journal space.  This should be true for any transactions that
    /// might alleviate journal space (i.e. compaction).
    pub skip_journal_checks: bool,

    /// If true, borrow metadata space from the metadata reservation.  This setting should be set to
    /// true for any transaction that will either not affect space usage after compaction
    /// (e.g. setting attributes), or reduce space usage (e.g. unlinking).  Otherwise, a transaction
    /// might fail with an out-of-space error.
    pub borrow_metadata_space: bool,

    /// If specified, a reservation to be used with the transaction.  If not set, any allocations
    /// that are part of this transaction will have to take their chances, and will fail if there is
    /// no free space.  The intention is that this should be used for things like the journal which
    /// require guaranteed space.
    pub allocator_reservation: Option<&'a Reservation>,
}

// This is the amount of space that we reserve for metadata when we are creating a new transaction.
// A transaction should not take more than this.  This is expressed in terms of space occupied in
// the journal; transactions must not take up more space in the journal than the number below.  The
// amount chosen here must be large enough for the maximum possible transaction that can be created,
// so transactions always need to be bounded which might involve splitting an operation up into
// smaller transactions.
pub const TRANSACTION_MAX_JOURNAL_USAGE: u64 = 24_576;
pub const TRANSACTION_METADATA_MAX_AMOUNT: u64 =
    reserved_space_from_journal_usage(TRANSACTION_MAX_JOURNAL_USAGE);

#[must_use]
pub struct TransactionLocks<'a>(pub WriteGuard<'a>);

/// The journal consists of these records which will be replayed at mount time.  Within a
/// transaction, these are stored as a set which allows some mutations to be deduplicated and found
/// (and we require custom comparison functions below).  For example, we need to be able to find
/// object size changes.
pub type Mutation = MutationV57;

#[derive(
    Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, TypeFingerprint, Versioned,
)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub enum MutationV57 {
    ObjectStore(ObjectStoreMutationV56),
    EncryptedObjectStore(#[serde(with = "crate::zerocopy_serialization")] Box<[u8]>),
    Allocator(AllocatorMutationV32),
    /// Indicates the beginning of a flush. This would typically involve sealing a tree.
    BeginFlush,
    /// Indicates the end of a flush. This would typically involve replacing the immutable layers
    /// with compacted ones.
    EndFlush,
    /// Volume has been deleted. Requires we remove it from the set of managed ObjectStore.
    DeleteVolume,
    UpdateBorrowed(u64),
    UpdateMutationsKey(UpdateMutationsKey),
    CreateInternalDir(u64),
}

#[derive(Migrate, Clone, Debug, PartialEq, Serialize, Deserialize, TypeFingerprint, Versioned)]
#[migrate_to_version(MutationV57)]
pub enum MutationV56 {
    ObjectStore(ObjectStoreMutationV56),
    EncryptedObjectStore(#[serde(with = "crate::zerocopy_serialization")] Box<[u8]>),
    Allocator(AllocatorMutationV32),
    BeginFlush,
    EndFlush,
    DeleteVolume,
    UpdateBorrowed(u64),
    UpdateMutationsKey(UpdateMutationsKey),
    CreateInternalDir(u64),
}

impl Mutation {
    pub fn insert_object(key: ObjectKey, value: ObjectValue) -> Self {
        Mutation::ObjectStore(ObjectStoreMutation {
            item: Item::new(key, value),
            op: Operation::Insert,
        })
    }

    pub fn replace_or_insert_object(key: ObjectKey, value: ObjectValue) -> Self {
        Mutation::ObjectStore(ObjectStoreMutation {
            item: Item::new(key, value),
            op: Operation::ReplaceOrInsert,
        })
    }

    pub fn merge_object(key: ObjectKey, value: ObjectValue) -> Self {
        Mutation::ObjectStore(ObjectStoreMutation {
            item: Item::new(key, value),
            op: Operation::Merge,
        })
    }

    pub fn update_mutations_key(key: FxfsKey) -> Self {
        Mutation::UpdateMutationsKey(key.into())
    }
}

// We have custom comparison functions for mutations that just use the key, rather than the key and
// value that would be used by default so that we can deduplicate and find mutations (see
// get_object_mutation below).
pub type ObjectStoreMutation = ObjectStoreMutationV56;

#[derive(Clone, Debug, Serialize, Deserialize, TypeFingerprint)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub struct ObjectStoreMutationV56 {
    pub item: ObjectItemV56,
    pub op: Operation,
}

/// The different LSM tree operations that can be performed as part of a mutation.
pub type Operation = OperationV32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypeFingerprint)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub enum OperationV32 {
    Insert,
    ReplaceOrInsert,
    Merge,
}

impl Ord for ObjectStoreMutation {
    fn cmp(&self, other: &Self) -> Ordering {
        self.item.key.cmp(&other.item.key)
    }
}

impl PartialOrd for ObjectStoreMutation {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ObjectStoreMutation {
    fn eq(&self, other: &Self) -> bool {
        self.item.key.eq(&other.item.key)
    }
}

impl Eq for ObjectStoreMutation {}

impl Ord for AllocatorItem {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

impl PartialOrd for AllocatorItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Same as std::ops::Range but with Ord and PartialOrd support, sorted first by start of the range,
/// then by the end.
pub type DeviceRange = DeviceRangeV32;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, TypeFingerprint)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub struct DeviceRangeV32(pub Range<u64>);

impl Deref for DeviceRange {
    type Target = Range<u64>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for DeviceRange {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<Range<u64>> for DeviceRange {
    fn from(range: Range<u64>) -> Self {
        Self(range)
    }
}

impl Into<Range<u64>> for DeviceRange {
    fn into(self) -> Range<u64> {
        self.0
    }
}

impl Ord for DeviceRange {
    fn cmp(&self, other: &Self) -> Ordering {
        self.start.cmp(&other.start).then(self.end.cmp(&other.end))
    }
}

impl PartialOrd for DeviceRange {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub type AllocatorMutation = AllocatorMutationV32;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, TypeFingerprint)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub enum AllocatorMutationV32 {
    Allocate {
        device_range: DeviceRangeV32,
        owner_object_id: u64,
    },
    Deallocate {
        device_range: DeviceRangeV32,
        owner_object_id: u64,
    },
    SetLimit {
        owner_object_id: u64,
        bytes: u64,
    },
    /// Marks all extents with a given owner_object_id for deletion.
    /// Used to free space allocated to encrypted ObjectStore where we may not have the key.
    /// Note that the actual deletion time is undefined so this should never be used where an
    /// ObjectStore is still in use due to a high risk of corruption. Similarly, owner_object_id
    /// should never be reused for the same reasons.
    MarkForDeletion(u64),
}

pub type UpdateMutationsKey = UpdateMutationsKeyV49;

#[derive(Clone, Debug, Serialize, Deserialize, TypeFingerprint)]
pub struct UpdateMutationsKeyV49(pub FxfsKeyV49);

impl From<UpdateMutationsKey> for FxfsKey {
    fn from(outer: UpdateMutationsKey) -> Self {
        outer.0
    }
}

impl From<FxfsKey> for UpdateMutationsKey {
    fn from(inner: FxfsKey) -> Self {
        Self(inner)
    }
}

#[cfg(fuzz)]
impl<'a> arbitrary::Arbitrary<'a> for UpdateMutationsKey {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(UpdateMutationsKey::from(FxfsKey::arbitrary(u).unwrap()))
    }
}

impl Ord for UpdateMutationsKey {
    fn cmp(&self, other: &Self) -> Ordering {
        (self as *const UpdateMutationsKey).cmp(&(other as *const _))
    }
}

impl PartialOrd for UpdateMutationsKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for UpdateMutationsKey {}

impl PartialEq for UpdateMutationsKey {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// When creating a transaction, locks typically need to be held to prevent two or more writers
/// trying to make conflicting mutations at the same time.  LockKeys are used for this.
/// NOTE: Ordering is important here!  The lock manager sorts the list of locks in a transaction
/// to acquire them in a consistent order, but there is a special case for the Flush lock.
/// The Flush lock is taken when we flush an LSM tree (e.g. an object store), and is held for
/// several transactions.  As such, it must come first in the lock acquisition ordering, so that
/// other transactions using the Flush lock have the same ordering as in flushing.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Copy)]
pub enum LockKey {
    /// Used to lock flushing an object.
    Flush {
        object_id: u64,
    },

    /// Used to lock changes to a particular object attribute (e.g. writes).
    ObjectAttribute {
        store_object_id: u64,
        object_id: u64,
        attribute_id: AttributeId,
    },

    /// Used to lock changes to a particular object (e.g. adding a child to a directory).
    Object {
        store_object_id: u64,
        object_id: u64,
    },

    ProjectId {
        store_object_id: u64,
        project_id: ProjectId,
    },

    /// Used to lock any truncate operations for a file.
    Truncate {
        store_object_id: u64,
        object_id: u64,
    },

    /// A lock used when getting or creating the internal directory.
    InternalDirectory {
        store_object_id: u64,
    },

    /// Used to serialize pre caching of keys.  The lock ordering is different for this: it is
    /// acquired in `Filesystem::commit_transaction` and happens *after* other keys have been
    /// promoted to write locks, but before the commit lock.
    PreCacheKeys {
        store_object_id: u64,
    },
}

impl LockKey {
    pub const fn object_attribute(
        store_object_id: u64,
        object_id: u64,
        attribute_id: AttributeId,
    ) -> Self {
        LockKey::ObjectAttribute { store_object_id, object_id, attribute_id }
    }

    pub const fn object(store_object_id: u64, object_id: u64) -> Self {
        LockKey::Object { store_object_id, object_id }
    }

    pub const fn flush(object_id: u64) -> Self {
        LockKey::Flush { object_id }
    }

    pub const fn truncate(store_object_id: u64, object_id: u64) -> Self {
        LockKey::Truncate { store_object_id, object_id }
    }

    pub const fn pre_cache_keys(store_object_id: u64) -> Self {
        LockKey::PreCacheKeys { store_object_id }
    }
}

/// A container for holding `LockKey` objects. Can store a single `LockKey` inline.
#[derive(Clone, Debug)]
pub enum LockKeys {
    None,
    Inline(LockKey),
    Vec(Vec<LockKey>),
}

impl LockKeys {
    pub fn with_capacity(capacity: usize) -> Self {
        if capacity > 1 { LockKeys::Vec(Vec::with_capacity(capacity)) } else { LockKeys::None }
    }

    pub fn push(&mut self, key: LockKey) {
        match self {
            Self::None => *self = LockKeys::Inline(key),
            Self::Inline(inline) => {
                *self = LockKeys::Vec(vec![*inline, key]);
            }
            Self::Vec(vec) => vec.push(key),
        }
    }

    pub fn truncate(&mut self, len: usize) {
        match self {
            Self::None => {}
            Self::Inline(_) => {
                if len == 0 {
                    *self = Self::None;
                }
            }
            Self::Vec(vec) => vec.truncate(len),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Inline(_) => 1,
            Self::Vec(vec) => vec.len(),
        }
    }

    fn contains(&self, key: &LockKey) -> bool {
        match self {
            Self::None => false,
            Self::Inline(single) => single == key,
            Self::Vec(vec) => vec.contains(key),
        }
    }

    fn sort_unstable(&mut self) {
        match self {
            Self::Vec(vec) => vec.sort_unstable(),
            _ => {}
        }
    }

    fn dedup(&mut self) {
        match self {
            Self::Vec(vec) => vec.dedup(),
            _ => {}
        }
    }

    fn iter(&self) -> LockKeysIter<'_> {
        match self {
            LockKeys::None => LockKeysIter::None,
            LockKeys::Inline(key) => LockKeysIter::Inline(key),
            LockKeys::Vec(keys) => LockKeysIter::Vec(keys.iter()),
        }
    }
}

enum LockKeysIter<'a> {
    None,
    Inline(&'a LockKey),
    Vec(std::slice::Iter<'a, LockKey>),
}

impl<'a> Iterator for LockKeysIter<'a> {
    type Item = &'a LockKey;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::None => None,
            Self::Inline(inline) => {
                let next = *inline;
                *self = Self::None;
                Some(next)
            }
            Self::Vec(vec) => vec.next(),
        }
    }
}

impl Default for LockKeys {
    fn default() -> Self {
        LockKeys::None
    }
}

#[macro_export]
macro_rules! lock_keys {
    () => {
        $crate::object_store::transaction::LockKeys::None
    };
    ($lock_key:expr $(,)?) => {
        $crate::object_store::transaction::LockKeys::Inline($lock_key)
    };
    ($($lock_keys:expr),+ $(,)?) => {
        $crate::object_store::transaction::LockKeys::Vec(vec![$($lock_keys),+])
    };
}
pub use lock_keys;

/// Mutations in a transaction can be associated with an object so that when mutations are applied,
/// updates can be applied to in-memory structures. For example, we cache object sizes, so when a
/// size change is applied, we can update the cached object size.
pub trait AssociatedObject: Send + Sync {
    fn will_apply_mutation(&self, _mutation: &Mutation, _object_id: u64, _manager: &ObjectManager) {
    }
}

pub enum AssocObj<'a> {
    None,
    Borrowed(&'a dyn AssociatedObject),
    Owned(Box<dyn AssociatedObject>),
}

impl AssocObj<'_> {
    pub fn map<R, F: FnOnce(&dyn AssociatedObject) -> R>(&self, f: F) -> Option<R> {
        match self {
            AssocObj::None => None,
            AssocObj::Borrowed(b) => Some(f(*b)),
            AssocObj::Owned(o) => Some(f(o.as_ref())),
        }
    }
}

pub struct TxnMutation<'a> {
    // This, at time of writing, is either the object ID of an object store, or the object ID of the
    // allocator.  In the case of an object mutation, there's another object ID in the mutation
    // record that would be for the object actually being changed.
    pub object_id: u64,

    // The actual mutation.  This gets serialized to the journal.
    pub mutation: Mutation,

    // An optional associated object for the mutation.  During replay, there will always be no
    // associated object.
    pub associated_object: AssocObj<'a>,
}

// We store TxnMutation in a set, and for that, we only use object_id and mutation and not the
// associated object or checksum.
//
// WARNING: It is critical that `object_id` (which corresponds to the store ID for store mutations)
// remains the primary key for sorting. The commit pipeline (`commit_transaction`) relies on
// the mutations being sorted by `object_id` to acquire store locks in a consistent order,
// preventing deadlocks.
impl Ord for TxnMutation<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.object_id.cmp(&other.object_id).then_with(|| self.mutation.cmp(&other.mutation))
    }
}

impl PartialOrd for TxnMutation<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for TxnMutation<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.object_id.eq(&other.object_id) && self.mutation.eq(&other.mutation)
    }
}

impl Eq for TxnMutation<'_> {}

impl std::fmt::Debug for TxnMutation<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TxnMutation")
            .field("object_id", &self.object_id)
            .field("mutation", &self.mutation)
            .finish()
    }
}

/// An iterator over mutations belonging to a single object store within a transaction.
/// It wraps a mutable reference to a peekable iterator over transaction mutations and yields
/// mutations until the object ID changes from the object ID peeked at creation time.
pub struct ObjectMutationIterator<'a, 'b> {
    iter: &'a mut Peekable<btree_set::Iter<'b, TxnMutation<'b>>>,
    object_id: u64,
}

impl<'a, 'b> ObjectMutationIterator<'a, 'b> {
    pub fn new(iter: &'a mut Peekable<btree_set::Iter<'b, TxnMutation<'b>>>) -> Option<Self> {
        let object_id = iter.peek()?.object_id;
        Some(Self { iter, object_id })
    }

    pub fn object_id(&self) -> u64 {
        self.object_id
    }
}

impl<'b> Iterator for ObjectMutationIterator<'_, 'b> {
    type Item = &'b Mutation;

    fn next(&mut self) -> Option<Self::Item> {
        if self.iter.peek().is_some_and(|m| m.object_id == self.object_id) {
            Some(&self.iter.next().unwrap().mutation)
        } else {
            None
        }
    }
}

impl Drop for ObjectMutationIterator<'_, '_> {
    // Need to have a drop to iterate to the end so that we always finish the current object before
    // releasing the borrow.
    fn drop(&mut self) {
        for _ in self.by_ref() {}
    }
}

pub enum MetadataReservation {
    // The state after a transaction has been dropped.
    None,

    // Metadata space for this transaction is being borrowed from ObjectManager's metadata
    // reservation.
    Borrowed,

    // A metadata reservation was made when the transaction was created.
    Reservation(Reservation),

    // The metadata space is being _held_ within `allocator_reservation`.
    Hold(u64),
}

/// A transaction groups mutation records to be committed as a group.
pub struct Transaction<'a> {
    fs: Arc<FxFilesystem>,

    // The mutations that make up this transaction.
    mutations: BTreeSet<TxnMutation<'a>>,

    // The locks that this transaction currently holds.
    txn_locks: LockKeys,

    /// If set, an allocator reservation that should be used for allocations.
    pub allocator_reservation: Option<&'a Reservation>,

    /// The reservation for the metadata for this transaction.
    pub metadata_reservation: MetadataReservation,

    // Keep track of objects explicitly created by this transaction. No locks are required for them.
    // Addressed by (owner_object_id, object_id).
    new_objects: BTreeSet<(u64, u64)>,

    /// Any data checksums which should be evaluated when replaying this transaction.
    checksums: Vec<(Range<u64>, Vec<Checksum>, bool)>,

    /// Set if this transaction contains data (i.e. includes any extent mutations).
    includes_write: bool,
}

impl<'a> Transaction<'a> {
    /// Creates a new transaction.  `txn_locks` are read locks that can be upgraded to write locks
    /// at commit time.
    pub async fn new(
        fs: Arc<FxFilesystem>,
        options: Options<'a>,
        txn_locks: LockKeys,
    ) -> Result<Transaction<'a>, Error> {
        fs.add_transaction(options.skip_journal_checks).await;
        let fs_clone = fs.clone();
        let guard = scopeguard::guard((), |_| fs_clone.sub_transaction());
        let (metadata_reservation, allocator_reservation, hold) =
            fs.reservation_for_transaction(options).await?;

        let txn_locks = {
            let lock_manager = fs.lock_manager();
            let mut write_guard = lock_manager.txn_lock(txn_locks).await;
            std::mem::take(&mut write_guard.0.lock_keys)
        };
        let mut transaction = Transaction {
            fs,
            mutations: BTreeSet::new(),
            txn_locks,
            allocator_reservation: None,
            metadata_reservation,
            new_objects: BTreeSet::new(),
            checksums: Vec::new(),
            includes_write: false,
        };

        ScopeGuard::into_inner(guard);
        hold.map(|h| h.forget()); // Transaction takes ownership from here on.
        transaction.allocator_reservation = allocator_reservation;
        Ok(transaction)
    }

    pub fn mutations(&self) -> &BTreeSet<TxnMutation<'a>> {
        &self.mutations
    }

    pub fn take_mutations(&mut self) -> BTreeSet<TxnMutation<'a>> {
        self.new_objects.clear();
        mem::take(&mut self.mutations)
    }

    /// Adds a mutation to this transaction.  If the mutation already exists, it is replaced and the
    /// old mutation is returned.
    pub fn add(&mut self, object_id: u64, mutation: Mutation) -> Option<Mutation> {
        self.add_with_object(object_id, mutation, AssocObj::None)
    }

    /// Removes a mutation that matches `mutation`.
    pub fn remove(&mut self, object_id: u64, mutation: Mutation) {
        let txn_mutation = TxnMutation { object_id, mutation, associated_object: AssocObj::None };
        if self.mutations.remove(&txn_mutation) {
            if let Mutation::ObjectStore(ObjectStoreMutation {
                item:
                    ObjectItem {
                        key: ObjectKey { object_id: new_object_id, data: ObjectKeyData::Object },
                        ..
                    },
                op: Operation::Insert,
            }) = txn_mutation.mutation
            {
                self.new_objects.remove(&(object_id, new_object_id));
            }
        }
    }

    /// Adds a mutation with an associated object. If the mutation already exists, it is replaced
    /// and the old mutation is returned.
    pub fn add_with_object(
        &mut self,
        object_id: u64,
        mutation: Mutation,
        associated_object: AssocObj<'a>,
    ) -> Option<Mutation> {
        assert!(object_id != INVALID_OBJECT_ID);
        if let Mutation::ObjectStore(ObjectStoreMutation {
            item:
                Item {
                    key:
                        ObjectKey { data: ObjectKeyData::Attribute(_, AttributeKey::Extent(_)), .. },
                    ..
                },
            ..
        }) = &mutation
        {
            self.includes_write = true;
        }
        let txn_mutation = TxnMutation { object_id, mutation, associated_object };
        self.verify_locks(&txn_mutation);
        self.mutations.replace(txn_mutation).map(|m| m.mutation)
    }

    pub fn add_checksum(&mut self, range: Range<u64>, checksums: Vec<Checksum>, first_write: bool) {
        self.checksums.push((range, checksums, first_write));
    }

    pub fn includes_write(&self) -> bool {
        self.includes_write
    }

    pub fn checksums(&self) -> &[(Range<u64>, Vec<Checksum>, bool)] {
        &self.checksums
    }

    pub fn take_checksums(&mut self) -> Vec<(Range<u64>, Vec<Checksum>, bool)> {
        std::mem::replace(&mut self.checksums, Vec::new())
    }

    fn verify_locks(&mut self, mutation: &TxnMutation<'_>) {
        // It was considered to change the locks from Vec to BTreeSet since we'll now be searching
        // through it, but given the small set that these locks usually comprise, it probably isn't
        // worth it.
        match mutation {
            TxnMutation {
                mutation:
                    Mutation::ObjectStore {
                        0: ObjectStoreMutation { item: ObjectItem { key, .. }, op },
                    },
                object_id: store_object_id,
                ..
            } => {
                match &key.data {
                    ObjectKeyData::Attribute(..) => {
                        // TODO(https://fxbug.dev/42073914): Check lock requirements.
                    }
                    ObjectKeyData::Child { .. }
                    | ObjectKeyData::EncryptedChild(_)
                    | ObjectKeyData::EncryptedCasefoldChild(_)
                    | ObjectKeyData::CasefoldChild { .. }
                    | ObjectKeyData::LegacyCasefoldChild(_) => {
                        let id = key.object_id;
                        if !self.txn_locks.contains(&LockKey::object(*store_object_id, id))
                            && !self.new_objects.contains(&(*store_object_id, id))
                        {
                            debug_assert!(
                                false,
                                "Not holding required lock for object {id} \
                                in store {store_object_id}"
                            );
                            error!(
                                "Not holding required lock for object {id} in store \
                                {store_object_id}"
                            )
                        }
                    }
                    ObjectKeyData::GraveyardEntry { .. } => {
                        // TODO(https://fxbug.dev/42073911): Check lock requirements.
                    }
                    ObjectKeyData::GraveyardAttributeEntry { .. } => {
                        // TODO(https://fxbug.dev/122974): Check lock requirements.
                    }
                    ObjectKeyData::Keys => {
                        let id = key.object_id;
                        if !self.txn_locks.contains(&LockKey::object(*store_object_id, id))
                            && !self.new_objects.contains(&(*store_object_id, id))
                        {
                            debug_assert!(
                                false,
                                "Not holding required lock for object {id} \
                                in store {store_object_id}"
                            );
                            error!(
                                "Not holding required lock for object {id} in store \
                                {store_object_id}"
                            )
                        }
                    }
                    ObjectKeyData::Object => match op {
                        // Insert implies the caller expects no object with which to race
                        Operation::Insert => {
                            self.new_objects.insert((*store_object_id, key.object_id));
                        }
                        Operation::Merge | Operation::ReplaceOrInsert => {
                            let id = key.object_id;
                            if !self.txn_locks.contains(&LockKey::object(*store_object_id, id))
                                && !self.new_objects.contains(&(*store_object_id, id))
                            {
                                debug_assert!(
                                    false,
                                    "Not holding required lock for object {id} \
                                    in store {store_object_id}"
                                );
                                error!(
                                    "Not holding required lock for object {id} in store \
                                    {store_object_id}"
                                )
                            }
                        }
                    },
                    ObjectKeyData::Project { project_id, property: ProjectProperty::Limit } => {
                        if !self.txn_locks.contains(&LockKey::ProjectId {
                            store_object_id: *store_object_id,
                            project_id: *project_id,
                        }) {
                            debug_assert!(
                                false,
                                "Not holding required lock for project limit id {project_id} \
                                in store {store_object_id}"
                            );
                            error!(
                                "Not holding required lock for project limit id {project_id} in \
                                store {store_object_id}"
                            )
                        }
                    }
                    ObjectKeyData::Project { property: ProjectProperty::Usage, .. } => match op {
                        Operation::Insert | Operation::ReplaceOrInsert => {
                            panic!(
                                "Project usage is all handled by merging deltas, no inserts or \
                                replacements should be used"
                            );
                        }
                        // Merges are all handled like atomic +/- and serialized by the tree locks.
                        Operation::Merge => {}
                    },
                    ObjectKeyData::ExtendedAttribute { .. } => {
                        let id = key.object_id;
                        if !self.txn_locks.contains(&LockKey::object(*store_object_id, id))
                            && !self.new_objects.contains(&(*store_object_id, id))
                        {
                            debug_assert!(
                                false,
                                "Not holding required lock for object {id} \
                                in store {store_object_id} while mutating extended attribute"
                            );
                            error!(
                                "Not holding required lock for object {id} in store \
                                {store_object_id} while mutating extended attribute"
                            )
                        }
                    }
                }
            }
            TxnMutation { mutation: Mutation::DeleteVolume, object_id, .. } => {
                if !self.txn_locks.contains(&LockKey::flush(*object_id)) {
                    debug_assert!(false, "Not holding required lock for DeleteVolume");
                    error!("Not holding required lock for DeleteVolume");
                }
            }
            _ => {}
        }
    }

    /// Returns true if this transaction has no mutations.
    pub fn is_empty(&self) -> bool {
        self.mutations.is_empty()
    }

    /// Searches for an existing object mutation within the transaction that has the given key and
    /// returns it if found.
    pub fn get_object_mutation(
        &self,
        store_object_id: u64,
        key: ObjectKey,
    ) -> Option<&ObjectStoreMutation> {
        if let Some(TxnMutation { mutation: Mutation::ObjectStore(mutation), .. }) =
            self.mutations.get(&TxnMutation {
                object_id: store_object_id,
                mutation: Mutation::insert_object(key, ObjectValue::None),
                associated_object: AssocObj::None,
            })
        {
            Some(mutation)
        } else {
            None
        }
    }

    /// Commits a transaction.  If successful, returns the journal offset of the transaction.
    pub async fn commit(mut self) -> Result<u64, Error> {
        debug!(txn:? = &self; "Commit");
        self.fs.clone().commit_transaction(&mut self, |x| x).await
    }

    /// Commits and then runs the callback whilst locks are held.  The callback accepts a single
    /// parameter which is the journal offset of the transaction.
    pub async fn commit_with_callback<R: Send>(
        mut self,
        f: impl FnOnce(u64) -> R + Send,
    ) -> Result<R, Error> {
        debug!(txn:? = &self; "Commit");
        self.fs.clone().commit_transaction(&mut self, f).await
    }

    /// Commits the transaction, but allows the transaction to be used again.  The locks are not
    /// dropped (but transaction locks will get downgraded to read locks).
    pub async fn commit_and_continue(&mut self) -> Result<(), Error> {
        debug!(txn:? = self; "Commit");
        self.fs.clone().commit_transaction(self, |_| {}).await?;
        assert!(self.mutations.is_empty());
        self.fs.lock_manager().downgrade_locks(&self.txn_locks);
        Ok(())
    }

    /// Prepares to commit by upgrading transaction locks to write locks and waiting for active
    /// readers to finish.
    pub async fn commit_prepare(&self) {
        self.fs.lock_manager().commit_prepare(self).await;
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        // Call the filesystem implementation of drop_transaction which should, as a minimum, call
        // LockManager's drop_transaction to ensure the locks are released.
        debug!(txn:? = &self; "Drop");
        self.fs.clone().drop_transaction(self);
    }
}

impl std::fmt::Debug for Transaction<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transaction")
            .field("mutations", &self.mutations)
            .field("txn_locks", &self.txn_locks)
            .field("reservation", &self.allocator_reservation)
            .finish()
    }
}

pub enum BorrowedOrOwned<'a, T> {
    Borrowed(&'a T),
    Owned(T),
}

impl<T> Deref for BorrowedOrOwned<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            BorrowedOrOwned::Borrowed(b) => b,
            BorrowedOrOwned::Owned(o) => &o,
        }
    }
}

impl<'a, T> From<&'a T> for BorrowedOrOwned<'a, T> {
    fn from(value: &'a T) -> Self {
        BorrowedOrOwned::Borrowed(value)
    }
}

impl<T> From<T> for BorrowedOrOwned<'_, T> {
    fn from(value: T) -> Self {
        BorrowedOrOwned::Owned(value)
    }
}

/// LockManager holds the locks that transactions might have taken.  A TransactionManager
/// implementation would typically have one of these.
///
/// Three different kinds of locks are supported.  There are read locks and write locks, which are
/// as one would expect.  The third kind of lock is a _transaction_ lock (which is also known as an
/// upgradeable read lock).  When first acquired, these block other writes (including other
/// transaction locks) but do not block reads.  When it is time to commit a transaction, these locks
/// are upgraded to full write locks (without ever dropping the lock) and then dropped after
/// committing (unless commit_and_continue is used).  This way, reads are only blocked for the
/// shortest possible time.  It follows that write locks should be used sparingly.  Locks are
/// granted in order with one exception: when a lock is in the initial _transaction_ lock state
/// (LockState::Locked), all read locks are allowed even if there are other tasks waiting for the
/// lock.  The reason for this is because we allow read locks to be taken by tasks that have taken a
/// _transaction_ lock (i.e. recursion is allowed).  In other cases, such as when a writer is
/// waiting and there are only readers, readers will queue up behind the writer.
///
/// To summarize:
///
/// +-------------------------+-----------------+----------------+------------------+
/// |                         | While read_lock | While txn_lock | While write_lock |
/// |                         | is held         | is held        | is held          |
/// +-------------------------+-----------------+----------------+------------------+
/// | Can acquire read_lock?  | true            | true           | false            |
/// +-------------------------+-----------------+----------------+------------------+
/// | Can acquire txn_lock?   | true            | false          | false            |
/// +-------------------------+-----------------+----------------+------------------+
/// | Can acquire write_lock? | false           | false          | false            |
/// +-------------------------+-----------------+----------------+------------------+
pub struct LockManager {
    locks: Mutex<Locks>,
}

struct Locks {
    keys: HashMap<LockKey, LockEntry>,
}

impl Locks {
    fn drop_lock(&mut self, key: LockKey, state: LockState) {
        if let Entry::Occupied(mut occupied) = self.keys.entry(key) {
            let entry = occupied.get_mut();
            let wake = match state {
                LockState::ReadLock => {
                    entry.read_count -= 1;
                    entry.read_count == 0
                }
                // drop_write_locks currently depends on us treating Locked and WriteLock the same.
                LockState::Locked | LockState::WriteLock => {
                    entry.state = LockState::ReadLock;
                    true
                }
            };
            if wake {
                // SAFETY: The lock in `LockManager::locks` is held.
                unsafe {
                    entry.wake();
                }
                if entry.can_remove() {
                    occupied.remove_entry();
                }
            }
        } else {
            unreachable!();
        }
    }

    fn drop_read_locks(&mut self, lock_keys: LockKeys) {
        for lock in lock_keys.iter() {
            self.drop_lock(*lock, LockState::ReadLock);
        }
    }

    fn drop_write_locks(&mut self, lock_keys: LockKeys) {
        for lock in lock_keys.iter() {
            // This is a bit hacky, but this works for locks in either the Locked or WriteLock
            // states.
            self.drop_lock(*lock, LockState::WriteLock);
        }
    }

    // Downgrades locks from WriteLock to Locked.
    fn downgrade_locks(&mut self, lock_keys: &LockKeys) {
        for lock in lock_keys.iter() {
            // SAFETY: The lock in `LockManager::locks` is held.
            unsafe {
                self.keys.get_mut(lock).unwrap().downgrade_lock();
            }
        }
    }
}

#[derive(Debug)]
struct LockEntry {
    // In the states that allow readers (ReadLock, Locked), this count can be non-zero
    // to indicate the number of active readers.
    read_count: u64,

    // The state of the lock (see below).
    state: LockState,

    // A doubly-linked list of wakers that should be woken when they have been granted the lock.
    // New wakers are usually chained on to tail, with the exception being the case where a lock in
    // state Locked is to be upgraded to WriteLock, but can't because there are readers.  It might
    // be possible to use intrusive-collections in the future.
    head: *const LockWaker,
    tail: *const LockWaker,
}

unsafe impl Send for LockEntry {}

// Represents a node in the waker list.  It is only safe to access the members wrapped by UnsafeCell
// when LockManager's `locks` member is locked.
struct LockWaker {
    // The next and previous pointers in the doubly-linked list.
    next: UnsafeCell<*const LockWaker>,
    prev: UnsafeCell<*const LockWaker>,

    // Holds the lock key for this waker.  This is required so that we can find the associated
    // `LockEntry`.
    key: LockKey,

    // The underlying waker that should be used to wake the task.
    waker: UnsafeCell<WakerState>,

    // The target state for this waker.
    target_state: LockState,

    // True if this is an upgrade.
    is_upgrade: bool,

    // We need to be pinned because these form part of the linked list.
    _pin: PhantomPinned,
}

enum WakerState {
    // This is the initial state before the waker has been first polled.
    Pending,

    // Once polled, this contains the actual waker.
    Registered(Waker),

    // The waker has been woken and has been granted the lock.
    Woken,
}

impl WakerState {
    fn is_woken(&self) -> bool {
        matches!(self, WakerState::Woken)
    }
}

unsafe impl Send for LockWaker {}
unsafe impl Sync for LockWaker {}

impl LockWaker {
    // Waits for the waker to be woken.
    async fn wait(&self, manager: &LockManager) {
        // We must guard against the future being dropped.
        let waker_guard = scopeguard::guard((), |_| {
            let mut locks = manager.locks.lock();
            // SAFETY: We've acquired the lock.
            unsafe {
                if (*self.waker.get()).is_woken() {
                    // We were woken, but didn't actually run, so we must drop the lock.
                    if self.is_upgrade {
                        locks.keys.get_mut(&self.key).unwrap().downgrade_lock();
                    } else {
                        locks.drop_lock(self.key, self.target_state);
                    }
                } else {
                    // We haven't been woken but we've been dropped so we must remove ourself from
                    // the waker list.
                    locks.keys.get_mut(&self.key).unwrap().remove_waker(self);
                }
            }
        });

        poll_fn(|cx| {
            let _locks = manager.locks.lock();
            // SAFETY: We've acquired the lock.
            unsafe {
                if (*self.waker.get()).is_woken() {
                    Poll::Ready(())
                } else {
                    *self.waker.get() = WakerState::Registered(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await;

        ScopeGuard::into_inner(waker_guard);
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum LockState {
    // In this state, there are only readers.
    ReadLock,

    // This state is used for transactions to lock other writers (including other transactions), but
    // it still allows readers.
    Locked,

    // A writer has exclusive access; all other readers and writers are blocked.
    WriteLock,
}

impl LockManager {
    pub fn new() -> Self {
        LockManager { locks: Mutex::new(Locks { keys: HashMap::default() }) }
    }

    /// Acquires the locks.  It is the caller's responsibility to ensure that drop_transaction is
    /// called when a transaction is dropped i.e. the filesystem's drop_transaction method should
    /// call LockManager's drop_transaction method.
    pub async fn txn_lock<'a>(&'a self, lock_keys: LockKeys) -> TransactionLocks<'a> {
        TransactionLocks(
            debug_assert_not_too_long!(self.lock(lock_keys, LockState::Locked)).right().unwrap(),
        )
    }

    // `state` indicates the kind of lock required.  ReadLock means acquire a read lock.  Locked
    // means lock other writers, but still allow readers.  WriteLock means acquire a write lock.
    async fn lock<'a>(
        &'a self,
        mut lock_keys: LockKeys,
        target_state: LockState,
    ) -> Either<ReadGuard<'a>, WriteGuard<'a>> {
        let mut guard = match &target_state {
            LockState::ReadLock => Left(ReadGuard {
                manager: self.into(),
                lock_keys: LockKeys::with_capacity(lock_keys.len()),
            }),
            LockState::Locked | LockState::WriteLock => Right(WriteGuard {
                manager: self.into(),
                lock_keys: LockKeys::with_capacity(lock_keys.len()),
            }),
        };
        let guard_keys = match &mut guard {
            Left(g) => &mut g.lock_keys,
            Right(g) => &mut g.lock_keys,
        };
        lock_keys.sort_unstable();
        lock_keys.dedup();
        for lock in lock_keys.iter() {
            let lock_waker = None;
            pin_mut!(lock_waker);
            {
                let mut locks = self.locks.lock();
                match locks.keys.entry(*lock) {
                    Entry::Vacant(vacant) => {
                        vacant.insert(LockEntry {
                            read_count: if let LockState::ReadLock = target_state {
                                guard_keys.push(*lock);
                                1
                            } else {
                                guard_keys.push(*lock);
                                0
                            },
                            state: target_state,
                            head: std::ptr::null(),
                            tail: std::ptr::null(),
                        });
                    }
                    Entry::Occupied(mut occupied) => {
                        let entry = occupied.get_mut();
                        // SAFETY: We've acquired the lock.
                        if unsafe { entry.is_allowed(target_state, entry.head.is_null()) } {
                            if let LockState::ReadLock = target_state {
                                entry.read_count += 1;
                                guard_keys.push(*lock);
                            } else {
                                entry.state = target_state;
                                guard_keys.push(*lock);
                            }
                        } else {
                            // Initialise a waker and push it on the tail of the list.
                            // SAFETY: `lock_waker` isn't used prior to this point.
                            unsafe {
                                *lock_waker.as_mut().get_unchecked_mut() = Some(LockWaker {
                                    next: UnsafeCell::new(std::ptr::null()),
                                    prev: UnsafeCell::new(entry.tail),
                                    key: *lock,
                                    waker: UnsafeCell::new(WakerState::Pending),
                                    target_state: target_state,
                                    is_upgrade: false,
                                    _pin: PhantomPinned,
                                });
                            }
                            let waker = (*lock_waker).as_ref().unwrap();
                            if entry.tail.is_null() {
                                entry.head = waker;
                            } else {
                                // SAFETY: We've acquired the lock.
                                unsafe {
                                    *(*entry.tail).next.get() = waker;
                                }
                            }
                            entry.tail = waker;
                        }
                    }
                }
            }
            if let Some(waker) = &*lock_waker {
                waker.wait(self).await;
                guard_keys.push(*lock);
            }
        }
        guard
    }

    /// This should be called by the filesystem's drop_transaction implementation.
    pub fn drop_transaction(&self, transaction: &mut Transaction<'_>) {
        let mut locks = self.locks.lock();
        locks.drop_write_locks(std::mem::take(&mut transaction.txn_locks));
    }

    /// Prepares to commit by waiting for readers to finish.
    pub async fn commit_prepare(&self, transaction: &Transaction<'_>) {
        self.commit_prepare_keys(&transaction.txn_locks).await;
    }

    async fn commit_prepare_keys(&self, lock_keys: &LockKeys) {
        for lock in lock_keys.iter() {
            let lock_waker = None;
            pin_mut!(lock_waker);
            {
                let mut locks = self.locks.lock();
                let entry = locks.keys.get_mut(lock).unwrap();
                // Callers may invoke `Transaction::commit_prepare` explicitly before committing
                // (e.g. to upgrade locks and verify invariants after disk I/O has finished).
                // Skipping keys already in `LockState::WriteLock` makes `commit_prepare`
                // idempotent when called again during `commit_transaction`.
                if entry.state == LockState::WriteLock {
                    continue;
                }
                assert_eq!(entry.state, LockState::Locked);

                if entry.read_count == 0 {
                    entry.state = LockState::WriteLock;
                } else {
                    // Initialise a waker and push it on the head of the list.
                    // SAFETY: `lock_waker` isn't used prior to this point.
                    unsafe {
                        *lock_waker.as_mut().get_unchecked_mut() = Some(LockWaker {
                            next: UnsafeCell::new(entry.head),
                            prev: UnsafeCell::new(std::ptr::null()),
                            key: *lock,
                            waker: UnsafeCell::new(WakerState::Pending),
                            target_state: LockState::WriteLock,
                            is_upgrade: true,
                            _pin: PhantomPinned,
                        });
                    }
                    let waker = (*lock_waker).as_ref().unwrap();
                    if entry.head.is_null() {
                        entry.tail = (*lock_waker).as_ref().unwrap();
                    } else {
                        // SAFETY: We've acquired the lock.
                        unsafe {
                            *(*entry.head).prev.get() = waker;
                        }
                    }
                    entry.head = waker;
                }
            }

            if let Some(waker) = &*lock_waker {
                waker.wait(self).await;
            }
        }
    }

    /// Acquires a read lock for the given keys.  Read locks are only blocked whilst a transaction
    /// is being committed for the same locks.  They are only necessary where consistency is
    /// required between different mutations within a transaction.  For example, a write might
    /// change the size and extents for an object, in which case a read lock is required so that
    /// observed size and extents are seen together or not at all.
    pub async fn read_lock<'a>(&'a self, lock_keys: LockKeys) -> ReadGuard<'a> {
        debug_assert_not_too_long!(self.lock(lock_keys, LockState::ReadLock)).left().unwrap()
    }

    /// Acquires a write lock for the given keys.  Write locks provide exclusive access to the
    /// requested lock keys.
    pub async fn write_lock<'a>(&'a self, lock_keys: LockKeys) -> WriteGuard<'a> {
        debug_assert_not_too_long!(self.lock(lock_keys, LockState::WriteLock)).right().unwrap()
    }

    /// Downgrades locks from the WriteLock state to Locked state.  This will panic if the locks are
    /// not in the WriteLock state.
    pub fn downgrade_locks(&self, lock_keys: &LockKeys) {
        self.locks.lock().downgrade_locks(lock_keys);
    }
}

// These unsafe functions require that `locks` in LockManager is locked.
impl LockEntry {
    unsafe fn wake(&mut self) {
        // If the lock's state is WriteLock, or there's nothing waiting, return early.
        if self.head.is_null() || self.state == LockState::WriteLock {
            return;
        }

        let waker = unsafe { &*self.head };

        if waker.is_upgrade {
            if self.read_count > 0 {
                return;
            }
        } else if !unsafe { self.is_allowed(waker.target_state, true) } {
            return;
        }

        unsafe { self.pop_and_wake() };

        // If the waker was a write lock, we can't wake any more up, but otherwise, we can keep
        // waking up readers.
        if waker.target_state == LockState::WriteLock {
            return;
        }

        while !self.head.is_null() && unsafe { (*self.head).target_state } == LockState::ReadLock {
            unsafe { self.pop_and_wake() };
        }
    }

    unsafe fn pop_and_wake(&mut self) {
        let waker = unsafe { &*self.head };

        // Pop the waker.
        self.head = unsafe { *waker.next.get() };
        if self.head.is_null() {
            self.tail = std::ptr::null()
        } else {
            unsafe { *(*self.head).prev.get() = std::ptr::null() };
        }

        // Adjust our state accordingly.
        if waker.target_state == LockState::ReadLock {
            self.read_count += 1;
        } else {
            self.state = waker.target_state;
        }

        // Now wake the task.
        if let WakerState::Registered(waker) =
            std::mem::replace(unsafe { &mut *waker.waker.get() }, WakerState::Woken)
        {
            waker.wake();
        }
    }

    fn can_remove(&self) -> bool {
        self.state == LockState::ReadLock && self.read_count == 0
    }

    unsafe fn remove_waker(&mut self, waker: &LockWaker) {
        unsafe {
            let is_first = (*waker.prev.get()).is_null();
            if is_first {
                self.head = *waker.next.get();
            } else {
                *(**waker.prev.get()).next.get() = *waker.next.get();
            }
            if (*waker.next.get()).is_null() {
                self.tail = *waker.prev.get();
            } else {
                *(**waker.next.get()).prev.get() = *waker.prev.get();
            }
            if is_first {
                // We must call wake in case we erased a pending write lock and readers can now
                // proceed.
                self.wake();
            }
        }
    }

    // Returns whether or not a lock with given `target_state` can proceed.  `is_head` should be
    // true if this is something at the head of the waker list (or the waker list is empty) and
    // false if there are other items on the waker list that are prior.
    unsafe fn is_allowed(&self, target_state: LockState, is_head: bool) -> bool {
        match self.state {
            LockState::ReadLock => {
                // Allow ReadLock and Locked so long as nothing else is waiting.
                (self.read_count == 0
                    || target_state == LockState::Locked
                    || target_state == LockState::ReadLock)
                    && is_head
            }
            LockState::Locked => {
                // Always allow reads unless there's an upgrade waiting.  We have to
                // always allow reads in this state because tasks that have locks in
                // the Locked state can later try and acquire ReadLock.
                target_state == LockState::ReadLock
                    && (is_head || unsafe { !(*self.head).is_upgrade })
            }
            LockState::WriteLock => false,
        }
    }

    unsafe fn downgrade_lock(&mut self) {
        assert_eq!(std::mem::replace(&mut self.state, LockState::Locked), LockState::WriteLock);
        unsafe { self.wake() };
    }
}

#[must_use]
pub struct ReadGuard<'a> {
    manager: LockManagerRef<'a>,
    lock_keys: LockKeys,
}

impl ReadGuard<'_> {
    pub fn fs(&self) -> Option<&Arc<FxFilesystem>> {
        if let LockManagerRef::Owned(fs) = &self.manager { Some(fs) } else { None }
    }

    pub fn into_owned(mut self, fs: Arc<FxFilesystem>) -> ReadGuard<'static> {
        ReadGuard {
            manager: LockManagerRef::Owned(fs),
            lock_keys: std::mem::replace(&mut self.lock_keys, LockKeys::None),
        }
    }
}

impl Drop for ReadGuard<'_> {
    fn drop(&mut self) {
        let mut locks = self.manager.locks.lock();
        locks.drop_read_locks(std::mem::take(&mut self.lock_keys));
    }
}

impl fmt::Debug for ReadGuard<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadGuard")
            .field("manager", &(&self.manager as *const _))
            .field("lock_keys", &self.lock_keys)
            .finish()
    }
}

#[must_use]
pub struct WriteGuard<'a> {
    manager: LockManagerRef<'a>,
    lock_keys: LockKeys,
}

impl WriteGuard<'_> {
    pub fn into_owned(mut self, fs: Arc<FxFilesystem>) -> WriteGuard<'static> {
        WriteGuard {
            manager: LockManagerRef::Owned(fs),
            lock_keys: std::mem::replace(&mut self.lock_keys, LockKeys::None),
        }
    }
}

impl Drop for WriteGuard<'_> {
    fn drop(&mut self) {
        let mut locks = self.manager.locks.lock();
        locks.drop_write_locks(std::mem::take(&mut self.lock_keys));
    }
}

impl fmt::Debug for WriteGuard<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteGuard")
            .field("manager", &(&self.manager as *const _))
            .field("lock_keys", &self.lock_keys)
            .finish()
    }
}

enum LockManagerRef<'a> {
    Borrowed(&'a LockManager),
    Owned(Arc<FxFilesystem>),
}

impl Deref for LockManagerRef<'_> {
    type Target = LockManager;

    fn deref(&self) -> &Self::Target {
        match self {
            LockManagerRef::Borrowed(m) => m,
            LockManagerRef::Owned(f) => f.lock_manager(),
        }
    }
}

impl<'a> From<&'a LockManager> for LockManagerRef<'a> {
    fn from(value: &'a LockManager) -> Self {
        LockManagerRef::Borrowed(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AssocObj, AttributeId, LockKey, LockKeys, LockManager, LockState, Mutation,
        ObjectMutationIterator, Options, TxnMutation,
    };
    use crate::filesystem::FxFilesystem;
    use fuchsia_async as fasync;
    use fuchsia_sync::Mutex;
    use futures::channel::oneshot::channel;
    use futures::future::FutureExt;
    use futures::stream::FuturesUnordered;
    use futures::{StreamExt, join, pin_mut};
    use std::collections::BTreeSet;
    use std::task::Poll;
    use std::time::Duration;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;

    #[fuchsia::test]
    async fn test_simple() {
        let device = DeviceHolder::new(FakeDevice::new(4096, 1024));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let mut t = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        t.add(1, Mutation::BeginFlush);
        assert!(!t.is_empty());
    }

    #[fuchsia::test]
    async fn test_locks() {
        let device = DeviceHolder::new(FakeDevice::new(4096, 1024));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let (send1, recv1) = channel();
        let (send2, recv2) = channel();
        let (send3, recv3) = channel();
        let done = Mutex::new(false);
        let mut futures = FuturesUnordered::new();
        futures.push(
            async {
                let _t = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object_attribute(1, 2, AttributeId::TEST_ID)],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                send1.send(()).unwrap(); // Tell the next future to continue.
                send3.send(()).unwrap(); // Tell the last future to continue.
                recv2.await.unwrap();
                // This is a halting problem so all we can do is sleep.
                fasync::Timer::new(Duration::from_millis(100)).await;
                assert!(!*done.lock());
            }
            .boxed(),
        );
        futures.push(
            async {
                recv1.await.unwrap();
                // This should not block since it is a different key.
                let _t = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object_attribute(2, 2, AttributeId::TEST_ID)],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                // Tell the first future to continue.
                send2.send(()).unwrap();
            }
            .boxed(),
        );
        futures.push(
            async {
                // This should block until the first future has completed.
                recv3.await.unwrap();
                let _t = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object_attribute(1, 2, AttributeId::TEST_ID)],
                        Options::default(),
                    )
                    .await;
                *done.lock() = true;
            }
            .boxed(),
        );
        while let Some(()) = futures.next().await {}
    }

    #[fuchsia::test]
    async fn test_read_lock_after_write_lock() {
        let device = DeviceHolder::new(FakeDevice::new(4096, 1024));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let (send1, recv1) = channel();
        let (send2, recv2) = channel();
        let done = Mutex::new(false);
        join!(
            async {
                let t = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object_attribute(1, 2, AttributeId::TEST_ID)],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                send1.send(()).unwrap(); // Tell the next future to continue.
                recv2.await.unwrap();
                t.commit().await.expect("commit failed");
                *done.lock() = true;
            },
            async {
                recv1.await.unwrap();
                // Reads should not be blocked until the transaction is committed.
                let _guard = fs
                    .lock_manager()
                    .read_lock(lock_keys![LockKey::object_attribute(1, 2, AttributeId::TEST_ID)])
                    .await;
                // Tell the first future to continue.
                send2.send(()).unwrap();
                // It shouldn't proceed until we release our read lock, but it's a halting
                // problem, so sleep.
                fasync::Timer::new(Duration::from_millis(100)).await;
                assert!(!*done.lock());
            },
        );
    }

    #[fuchsia::test]
    async fn test_write_lock_after_read_lock() {
        let device = DeviceHolder::new(FakeDevice::new(4096, 1024));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let (send1, recv1) = channel();
        let (send2, recv2) = channel();
        let done = Mutex::new(false);
        join!(
            async {
                // Reads should not be blocked until the transaction is committed.
                let _guard = fs
                    .lock_manager()
                    .read_lock(lock_keys![LockKey::object_attribute(1, 2, AttributeId::TEST_ID)])
                    .await;
                // Tell the next future to continue and then wait.
                send1.send(()).unwrap();
                recv2.await.unwrap();
                // It shouldn't proceed until we release our read lock, but it's a halting
                // problem, so sleep.
                fasync::Timer::new(Duration::from_millis(100)).await;
                assert!(!*done.lock());
            },
            async {
                recv1.await.unwrap();
                let t = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object_attribute(1, 2, AttributeId::TEST_ID)],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                send2.send(()).unwrap(); // Tell the first future to continue;
                t.commit().await.expect("commit failed");
                *done.lock() = true;
            },
        );
    }

    #[fuchsia::test]
    async fn test_drop_uncommitted_transaction() {
        let device = DeviceHolder::new(FakeDevice::new(4096, 1024));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let key = lock_keys![LockKey::object(1, 1)];

        // Dropping while there's a reader.
        {
            let _write_lock = fs
                .root_store()
                .new_transaction(key.clone(), Options::default())
                .await
                .expect("new_transaction failed");
            let _read_lock = fs.lock_manager().read_lock(key.clone()).await;
        }
        // Dropping while there's no reader.
        {
            let _write_lock = fs
                .root_store()
                .new_transaction(key.clone(), Options::default())
                .await
                .expect("new_transaction failed");
        }
        // Make sure we can take the lock again (i.e. it was actually released).
        fs.root_store()
            .new_transaction(key.clone(), Options::default())
            .await
            .expect("new_transaction failed");
    }

    #[fuchsia::test]
    async fn test_drop_waiting_write_lock() {
        let manager = LockManager::new();
        let keys = lock_keys![LockKey::object(1, 1)];
        {
            let _guard = manager.lock(keys.clone(), LockState::ReadLock).await;
            if let Poll::Ready(_) =
                futures::poll!(manager.lock(keys.clone(), LockState::WriteLock).boxed())
            {
                assert!(false);
            }
        }
        let _ = manager.lock(keys, LockState::WriteLock).await;
    }

    #[fuchsia::test]
    async fn test_write_lock_blocks_everything() {
        let manager = LockManager::new();
        let keys = lock_keys![LockKey::object(1, 1)];
        {
            let _guard = manager.lock(keys.clone(), LockState::WriteLock).await;
            if let Poll::Ready(_) =
                futures::poll!(manager.lock(keys.clone(), LockState::WriteLock).boxed())
            {
                assert!(false);
            }
            if let Poll::Ready(_) =
                futures::poll!(manager.lock(keys.clone(), LockState::ReadLock).boxed())
            {
                assert!(false);
            }
        }
        {
            let _guard = manager.lock(keys.clone(), LockState::WriteLock).await;
        }
        {
            let _guard = manager.lock(keys, LockState::ReadLock).await;
        }
    }

    #[fuchsia::test]
    async fn test_downgrade_locks() {
        let manager = LockManager::new();
        let keys = lock_keys![LockKey::object(1, 1)];
        let _guard = manager.txn_lock(keys.clone()).await;
        manager.commit_prepare_keys(&keys).await;

        // Use FuturesUnordered so that we can check that the waker is woken.
        let mut read_lock: FuturesUnordered<_> =
            std::iter::once(manager.read_lock(keys.clone())).collect();

        // Trying to acquire a read lock now should be blocked.
        assert!(futures::poll!(read_lock.next()).is_pending());

        manager.downgrade_locks(&keys);

        // After downgrading, it should be possible to take a read lock.
        assert!(futures::poll!(read_lock.next()).is_ready());
    }

    #[fuchsia::test]
    async fn test_dropped_write_lock_wakes() {
        let manager = LockManager::new();
        let keys = lock_keys![LockKey::object(1, 1)];
        let _guard = manager.lock(keys.clone(), LockState::ReadLock).await;
        let mut read_lock = FuturesUnordered::new();
        read_lock.push(manager.lock(keys.clone(), LockState::ReadLock));

        {
            let write_lock = manager.lock(keys, LockState::WriteLock);
            pin_mut!(write_lock);

            // The write lock should be blocked because of the read lock.
            assert!(futures::poll!(write_lock).is_pending());

            // Another read lock should be blocked because of the write lock.
            assert!(futures::poll!(read_lock.next()).is_pending());
        }

        // Dropping the write lock should allow the read lock to proceed.
        assert!(futures::poll!(read_lock.next()).is_ready());
    }

    #[fuchsia::test]
    async fn test_drop_upgrade() {
        let manager = LockManager::new();
        let keys = lock_keys![LockKey::object(1, 1)];
        let _guard = manager.lock(keys.clone(), LockState::Locked).await;

        {
            let commit_prepare = manager.commit_prepare_keys(&keys);
            pin_mut!(commit_prepare);
            let _read_guard = manager.lock(keys.clone(), LockState::ReadLock).await;
            assert!(futures::poll!(commit_prepare).is_pending());

            // Now we test dropping read_guard which should wake commit_prepare and
            // then dropping commit_prepare.
        }

        // We should be able to still commit_prepare.
        manager.commit_prepare_keys(&keys).await;
    }

    #[fuchsia::test]
    async fn test_woken_upgrade_blocks_reads() {
        let manager = LockManager::new();
        let keys = lock_keys![LockKey::object(1, 1)];
        // Start with a transaction lock.
        let guard = manager.lock(keys.clone(), LockState::Locked).await;

        // Take a read lock.
        let read1 = manager.lock(keys.clone(), LockState::ReadLock).await;

        // Try and upgrade the transaction lock, which should not be possible because of the read.
        let commit_prepare = manager.commit_prepare_keys(&keys);
        pin_mut!(commit_prepare);
        assert!(futures::poll!(commit_prepare.as_mut()).is_pending());

        // Taking another read should also be blocked.
        let read2 = manager.lock(keys.clone(), LockState::ReadLock);
        pin_mut!(read2);
        assert!(futures::poll!(read2.as_mut()).is_pending());

        // Drop the first read and the upgrade should complete.
        std::mem::drop(read1);
        assert!(futures::poll!(commit_prepare).is_ready());

        // But the second read should still be blocked.
        assert!(futures::poll!(read2.as_mut()).is_pending());

        // If we drop the write lock now, the read should be unblocked.
        std::mem::drop(guard);
        assert!(futures::poll!(read2).is_ready());
    }

    static LOCK_KEY_1: LockKey = LockKey::flush(1);
    static LOCK_KEY_2: LockKey = LockKey::flush(2);
    static LOCK_KEY_3: LockKey = LockKey::flush(3);

    // The keys, storage method, and capacity must all match.
    fn assert_lock_keys_equal(value: &LockKeys, expected: &LockKeys) {
        match (value, expected) {
            (LockKeys::None, LockKeys::None) => {}
            (LockKeys::Inline(key1), LockKeys::Inline(key2)) => {
                if key1 != key2 {
                    panic!("{key1:?} != {key2:?}");
                }
            }
            (LockKeys::Vec(vec1), LockKeys::Vec(vec2)) => {
                if vec1 != vec2 {
                    panic!("{vec1:?} != {vec2:?}");
                }
                if vec1.capacity() != vec2.capacity() {
                    panic!(
                        "LockKeys have different capacity: {} != {}",
                        vec1.capacity(),
                        vec2.capacity()
                    );
                }
            }
            (_, _) => panic!("{value:?} != {expected:?}"),
        }
    }

    // Only the keys must match. Storage method and capacity don't matter.
    fn assert_lock_keys_equivalent(value: &LockKeys, expected: &LockKeys) {
        let value: Vec<_> = value.iter().collect();
        let expected: Vec<_> = expected.iter().collect();
        assert_eq!(value, expected);
    }

    #[test]
    fn test_lock_keys_macro() {
        assert_lock_keys_equal(&lock_keys![], &LockKeys::None);
        assert_lock_keys_equal(&lock_keys![LOCK_KEY_1], &LockKeys::Inline(LOCK_KEY_1));
        assert_lock_keys_equal(
            &lock_keys![LOCK_KEY_1, LOCK_KEY_2],
            &LockKeys::Vec(vec![LOCK_KEY_1, LOCK_KEY_2]),
        );
    }

    #[test]
    fn test_lock_keys_with_capacity() {
        assert_lock_keys_equal(&LockKeys::with_capacity(0), &LockKeys::None);
        assert_lock_keys_equal(&LockKeys::with_capacity(1), &LockKeys::None);
        assert_lock_keys_equal(&LockKeys::with_capacity(2), &LockKeys::Vec(Vec::with_capacity(2)));
    }

    #[test]
    fn test_lock_keys_len() {
        assert_eq!(lock_keys![].len(), 0);
        assert_eq!(lock_keys![LOCK_KEY_1].len(), 1);
        assert_eq!(lock_keys![LOCK_KEY_1, LOCK_KEY_2].len(), 2);
    }

    #[test]
    fn test_lock_keys_contains() {
        assert_eq!(lock_keys![].contains(&LOCK_KEY_1), false);
        assert_eq!(lock_keys![LOCK_KEY_1].contains(&LOCK_KEY_1), true);
        assert_eq!(lock_keys![LOCK_KEY_1].contains(&LOCK_KEY_2), false);
        assert_eq!(lock_keys![LOCK_KEY_1, LOCK_KEY_2].contains(&LOCK_KEY_1), true);
        assert_eq!(lock_keys![LOCK_KEY_1, LOCK_KEY_2].contains(&LOCK_KEY_2), true);
        assert_eq!(lock_keys![LOCK_KEY_1, LOCK_KEY_2].contains(&LOCK_KEY_3), false);
    }

    #[test]
    fn test_lock_keys_push() {
        let mut keys = lock_keys![];
        keys.push(LOCK_KEY_1);
        assert_lock_keys_equal(&keys, &LockKeys::Inline(LOCK_KEY_1));
        keys.push(LOCK_KEY_2);
        assert_lock_keys_equal(&keys, &LockKeys::Vec(vec![LOCK_KEY_1, LOCK_KEY_2]));
        keys.push(LOCK_KEY_3);
        assert_lock_keys_equivalent(
            &keys,
            &LockKeys::Vec(vec![LOCK_KEY_1, LOCK_KEY_2, LOCK_KEY_3]),
        );
    }

    #[test]
    fn test_lock_keys_sort_unstable() {
        let mut keys = lock_keys![];
        keys.sort_unstable();
        assert_lock_keys_equal(&keys, &lock_keys![]);

        let mut keys = lock_keys![LOCK_KEY_1];
        keys.sort_unstable();
        assert_lock_keys_equal(&keys, &lock_keys![LOCK_KEY_1]);

        let mut keys = lock_keys![LOCK_KEY_2, LOCK_KEY_1];
        keys.sort_unstable();
        assert_lock_keys_equal(&keys, &lock_keys![LOCK_KEY_1, LOCK_KEY_2]);
    }

    #[test]
    fn test_lock_keys_dedup() {
        let mut keys = lock_keys![];
        keys.dedup();
        assert_lock_keys_equal(&keys, &lock_keys![]);

        let mut keys = lock_keys![LOCK_KEY_1];
        keys.dedup();
        assert_lock_keys_equal(&keys, &lock_keys![LOCK_KEY_1]);

        let mut keys = lock_keys![LOCK_KEY_1, LOCK_KEY_1];
        keys.dedup();
        assert_lock_keys_equivalent(&keys, &lock_keys![LOCK_KEY_1]);
    }

    #[test]
    fn test_lock_keys_truncate() {
        let mut keys = lock_keys![];
        keys.truncate(5);
        assert_lock_keys_equal(&keys, &lock_keys![]);
        keys.truncate(0);
        assert_lock_keys_equal(&keys, &lock_keys![]);

        let mut keys = lock_keys![LOCK_KEY_1];
        keys.truncate(5);
        assert_lock_keys_equal(&keys, &lock_keys![LOCK_KEY_1]);
        keys.truncate(0);
        assert_lock_keys_equal(&keys, &lock_keys![]);

        let mut keys = lock_keys![LOCK_KEY_1, LOCK_KEY_2];
        keys.truncate(5);
        assert_lock_keys_equal(&keys, &lock_keys![LOCK_KEY_1, LOCK_KEY_2]);
        keys.truncate(1);
        // Although there's only 1 key after truncate the key is not stored inline.
        assert_lock_keys_equivalent(&keys, &lock_keys![LOCK_KEY_1]);
    }

    #[test]
    fn test_lock_keys_iter() {
        assert_eq!(lock_keys![].iter().collect::<Vec<_>>(), Vec::<&LockKey>::new());

        assert_eq!(lock_keys![LOCK_KEY_1].iter().collect::<Vec<_>>(), vec![&LOCK_KEY_1]);

        assert_eq!(
            lock_keys![LOCK_KEY_1, LOCK_KEY_2].iter().collect::<Vec<_>>(),
            vec![&LOCK_KEY_1, &LOCK_KEY_2]
        );
    }

    #[test]
    fn test_object_mutation_iterator() {
        let mut mutations = BTreeSet::new();
        mutations.insert(TxnMutation {
            object_id: 1,
            mutation: Mutation::BeginFlush,
            associated_object: AssocObj::None,
        });
        mutations.insert(TxnMutation {
            object_id: 1,
            mutation: Mutation::EndFlush,
            associated_object: AssocObj::None,
        });
        mutations.insert(TxnMutation {
            object_id: 2,
            mutation: Mutation::DeleteVolume,
            associated_object: AssocObj::None,
        });

        let mut iter = mutations.iter().peekable();

        {
            let mut obj_iter = ObjectMutationIterator::new(&mut iter).expect("expected object 1");
            assert_eq!(obj_iter.object_id(), 1);
            assert_eq!(obj_iter.next(), Some(&Mutation::BeginFlush));
            assert_eq!(obj_iter.next(), Some(&Mutation::EndFlush));
            assert_eq!(obj_iter.next(), None);
        }

        {
            let mut obj_iter = ObjectMutationIterator::new(&mut iter).expect("expected object 2");
            assert_eq!(obj_iter.object_id(), 2);
            assert_eq!(obj_iter.next(), Some(&Mutation::DeleteVolume));
            assert_eq!(obj_iter.next(), None);
        }

        // No more objects
        assert!(ObjectMutationIterator::new(&mut iter).is_none());
    }

    #[test]
    fn test_object_mutation_iterator_drop_drains_remaining() {
        let mut mutations = BTreeSet::new();
        // Object 1 with 3 mutations
        mutations.insert(TxnMutation {
            object_id: 1,
            mutation: Mutation::BeginFlush,
            associated_object: AssocObj::None,
        });
        mutations.insert(TxnMutation {
            object_id: 1,
            mutation: Mutation::EndFlush,
            associated_object: AssocObj::None,
        });
        mutations.insert(TxnMutation {
            object_id: 1,
            mutation: Mutation::DeleteVolume,
            associated_object: AssocObj::None,
        });
        // Object 2 with 2 mutations
        mutations.insert(TxnMutation {
            object_id: 2,
            mutation: Mutation::BeginFlush,
            associated_object: AssocObj::None,
        });
        mutations.insert(TxnMutation {
            object_id: 2,
            mutation: Mutation::EndFlush,
            associated_object: AssocObj::None,
        });

        let mut iter = mutations.iter().peekable();

        {
            let mut obj_iter = ObjectMutationIterator::new(&mut iter).expect("expected object 1");
            assert_eq!(obj_iter.object_id(), 1);
            assert_eq!(obj_iter.next(), Some(&Mutation::BeginFlush));
            // Drop without reading EndFlush or DeleteVolume
        }

        {
            let obj_iter = ObjectMutationIterator::new(&mut iter).expect("expected object 2");
            assert_eq!(obj_iter.object_id(), 2);
            // Drop immediately without reading any mutations for object 2
        }

        // Dropping obj_iter for object 2 should have drained all object 2 mutations as well.
        assert!(ObjectMutationIterator::new(&mut iter).is_none());
    }
}
