// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod allocator;
pub mod caching_object_handle;
pub mod data_object_handle;
pub mod directory;
pub mod extent;
mod extent_mapping_iterator;
mod extent_record;
pub mod flush;
use crate::filesystem::FlushReason;
pub mod graveyard;
mod install;
pub mod journal;
mod key_manager;
pub(crate) mod merge;
pub mod object_manager;
pub mod object_record;
pub mod project_id;
mod store_object_handle;
pub mod transaction;
mod tree;
mod tree_cache;
pub mod volume;

pub use data_object_handle::{
    DataObjectHandle, DataObjectState, DirectWriter, FileExtent, FsverityStateInner, RangeType,
};
pub use directory::Directory;
pub use object_record::{ChildValue, DirType, ObjectDescriptor, PosixAttributes, Timestamp};
pub use store_object_handle::{MAX_INLINE_XATTR_SIZE, SetExtendedAttributeMode, StoreObjectHandle};

use crate::errors::FxfsError;
use crate::filesystem::{
    ApplyContext, ApplyMode, FxFilesystem, JournalingObject, MAX_FILE_SIZE, SyncOptions,
    TruncateGuard,
};
use crate::log::*;
use crate::lsm_tree::cache::ObjectCache;
use crate::lsm_tree::types::{Existence, Item, ItemRef, LayerIterator};
use crate::lsm_tree::{LSMTree, Query};
use crate::object_handle::{INVALID_OBJECT_ID, ObjectHandle, ObjectProperties, ReadObjectHandle};
use crate::object_store::allocator::Allocator;
use crate::object_store::graveyard::Graveyard;
use crate::object_store::journal::{JournalCheckpoint, JournalCheckpointV32, JournaledTransaction};
use crate::object_store::key_manager::KeyManager;
use crate::object_store::transaction::{
    AssocObj, AssociatedObject, LockKey, LockKeys, ObjectMutationIterator, ObjectStoreMutation,
    Operation, Options, Transaction, WriteGuard, lock_keys,
};
use crate::serialized_types::{
    AES_JOURNAL_ENCRYPTION_VERSION, DEFAULT_MAX_SERIALIZED_RECORD_SIZE, Version, Versioned,
    VersionedLatest,
};
use anyhow::{Context, Error, anyhow, bail, ensure};
use async_trait::async_trait;
use fidl_fuchsia_io as fio;
use fprint::TypeFingerprint;
use fuchsia_sync::Mutex;
use fxfs_crypto::ff1::Ff1;
use fxfs_crypto::{
    CipherHolder, Crypt, JournalCipher, JournalXtsCipher, KeyPurpose, ObjectType, StreamCipher,
    UnwrappedKey, WrappingKeyId, key_to_cipher,
};
use rand::Rng as _;
use scopeguard::ScopeGuard;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use storage_device::Device;
use storage_units::BlockSize;
use uuid::Uuid;

pub use extent::Extent;
pub use extent_record::{ExtentMode, ExtentValue};
pub use object_record::{
    AttributeId, AttributeKey, EncryptionKey, EncryptionKeys, ExtendedAttributeValue,
    FsverityMetadata, FxfsKey, FxfsKeyV49, ObjectAttributes, ObjectKey, ObjectKeyData, ObjectKind,
    ObjectValue, ProjectProperty, RootDigest,
};
pub use project_id::{ProjectId, ProjectIdExt};
pub use transaction::Mutation;

// For encrypted stores, the lower 32 bits of the object ID are encrypted to make side-channel
// attacks more difficult. This mask can be used to extract the hi part of the object ID.
const OBJECT_ID_HI_MASK: u64 = 0xffffffff00000000;

// At time of writing, this threshold limits transactions that delete extents to about 10,000 bytes.
const TRANSACTION_MUTATION_THRESHOLD: usize = 200;

// The number of keys we want to pre-cache for flushes.  We cache up to 2 keys.  One is for the
// current flush, and one is pre-cached for the next flush so that we don't need to call the crypt
// service during a flush.  To understand why, consider two threads T1 and T2.  Just prior to
// committing a transaction, T1 ensures there are two keys.  Then, before T1 has committed the
// transaction, T2 flushes and consumes one of the keys.  T1 then commits the transaction. The next
// time a flush occurs, there's a key ready. If T1 had only ensured there was one key, there'd be no
// key.
const CACHED_KEYS_LIMIT: usize = 2;

// Encrypted files and directories use the fscrypt key (identified by `FSCRYPT_KEY_ID`) to encrypt
// file contents and filenames respectively. All non-fscrypt encrypted files otherwise default to
// using the `VOLUME_DATA_KEY_ID` key. Note, the filesystem always uses the `VOLUME_DATA_KEY_ID`
// key to encrypt large extended attributes. Thus, encrypted files and directories with large
// xattrs will have both an fscrypt and volume data key.
pub const VOLUME_DATA_KEY_ID: u64 = 0;
pub const FSCRYPT_KEY_ID: u64 = 1;

/// DataObjectHandle stores an owner that must implement this trait, which allows the handle to get
/// back to an ObjectStore.
pub trait HandleOwner: AsRef<ObjectStore> + Send + Sync + 'static {}

/// StoreInfo stores information about the object store.  This is stored within the parent object
/// store, and is used, for example, to get the persistent layer objects.
pub type StoreInfo = StoreInfoV52;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TypeFingerprint, Versioned)]
pub struct StoreInfoV52 {
    /// The globally unique identifier for the associated object store. If unset, will be all zero.
    guid: [u8; 16],

    /// The last used object ID.  Note that this field is not accurate in memory; ObjectStore's
    /// last_object_id field is the one to use in that case.  Technically, this might not be the
    /// last object ID used for the latest transaction that created an object because we use this at
    /// the point of creating the object but before we commit the transaction.  Transactions can
    /// then get committed in an arbitrary order (or not at all).
    last_object_id: LastObjectIdInfo,

    /// Object ids for layers.  TODO(https://fxbug.dev/42178036): need a layer of indirection here
    /// so we can support snapshots.
    pub layers: Vec<u64>,

    /// The object ID for the root directory.
    root_directory_object_id: u64,

    /// The object ID for the graveyard.
    graveyard_directory_object_id: u64,

    /// The number of live objects in the store.  This should *not* be trusted; it can be invalid
    /// due to filesystem inconsistencies.
    object_count: u64,

    /// The (wrapped) key that encrypted mutations should use.
    mutations_key: Option<FxfsKeyV49>,

    /// Mutations for the store are encrypted using a stream cipher.  To decrypt the mutations, we
    /// need to know the offset in the cipher stream to start it.
    mutations_cipher_offset: u64,

    /// If we have to flush the store whilst we do not have the key, we need to write the encrypted
    /// mutations to an object. This is the object ID of that file if it exists.
    pub encrypted_mutations_object_id: u64,

    /// A directory for storing internal files in a directory structure. Holds INVALID_OBJECT_ID
    /// when the directory doesn't yet exist.
    internal_directory_object_id: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypeFingerprint)]
enum LastObjectIdInfo {
    Unencrypted {
        id: u64,
    },
    Encrypted {
        /// The *unencrypted* value of the last object ID.
        id: u64,

        /// Object IDs are encrypted to reduce the amount of information that sequential object IDs
        /// reveal (such as the number of files in the system and the ordering of their creation in
        /// time).  Only the bottom 32 bits of the object ID are encrypted whilst the top 32 bits
        /// will increment after 2^32 object IDs have been used and this allows us to roll the key.
        key: FxfsKeyV49,
    },
    Low32Bit,
}

impl Default for LastObjectIdInfo {
    fn default() -> Self {
        LastObjectIdInfo::Unencrypted { id: 0 }
    }
}

impl StoreInfo {
    /// Returns the parent objects for this store.
    pub fn parent_objects(&self) -> Vec<u64> {
        // We should not include the ID of the store itself, since that should be referred to in the
        // volume directory.
        let mut objects = self.layers.to_vec();
        if self.encrypted_mutations_object_id != INVALID_OBJECT_ID {
            objects.push(self.encrypted_mutations_object_id);
        }
        objects
    }
}

// TODO(https://fxbug.dev/42178037): We should test or put checks in place to ensure this limit isn't exceeded.
// It will likely involve placing limits on the maximum number of layers.
pub const MAX_STORE_INFO_SERIALIZED_SIZE: usize = 131072;

// This needs to be large enough to accommodate the maximum amount of unflushed data (data that is
// in the journal but hasn't yet been written to layer files) for a store.  We set a limit because
// we want to limit the amount of memory use in the case the filesystem is corrupt or under attack.
pub const MAX_ENCRYPTED_MUTATIONS_SIZE: usize = 8 * journal::DEFAULT_RECLAIM_SIZE as usize;

#[derive(Default)]
pub struct HandleOptions {
    /// If true, transactions used by this handle will skip journal space checks.
    pub skip_journal_checks: bool,
    /// If true, data written to any attribute of this handle will not have per-block checksums
    /// computed.
    pub skip_checksums: bool,
    /// If true, any files using fsverity will not attempt to perform any verification. This is
    /// useful to open an object without the correct encryption keys to look at the metadata.
    pub skip_fsverity: bool,
}

/// Parameters for encrypting a newly created object.
pub struct ObjectEncryptionOptions {
    /// If set, the keys are treated as permanent and never evicted from the KeyManager cache.
    /// This is necessary when keys are managed by another store; for example, the layer files
    /// of a child store are objects in the root store, but they are encrypted with keys from the
    /// child store.  Generally, most objects should have this set to `false`.
    pub permanent: bool,
    pub key_id: u64,
    pub key: EncryptionKey,
    pub unwrapped_key: UnwrappedKey,
}

pub struct StoreOptions {
    /// The store is unencrypted if store is none.
    pub crypt: Option<Arc<dyn Crypt>>,
}

impl Default for StoreOptions {
    fn default() -> Self {
        Self { crypt: None }
    }
}

#[derive(Default)]
pub struct NewChildStoreOptions {
    pub options: StoreOptions,

    /// Specifies the object ID in the root store to be used for the store.  If set to
    /// INVALID_OBJECT_ID (the default and typical case), a suitable ID will be chosen.
    pub object_id: u64,

    /// If true, reserve all 32 bit object_ids.  All new objects will start with IDs exceeding
    /// 0x1_0000_0000.
    pub reserve_32bit_object_ids: bool,

    /// Object IDs will be restricted to 32 bits.  This involves a less performant algorithm and so
    /// should not be used unless necessary.
    pub low_32_bit_object_ids: bool,

    /// If set, use this GUID for the new store.
    pub guid: Option<[u8; 16]>,
}

pub type EncryptedMutations = EncryptedMutationsV49;

#[derive(Clone, Default, Deserialize, Serialize, TypeFingerprint)]
pub struct EncryptedMutationsV49 {
    // Information about the mutations are held here, but the actual encrypted data is held within
    // data.  For each transaction, we record the checkpoint and the count of mutations within the
    // transaction.  The checkpoint is required for the log file offset (which we need to apply the
    // mutations), and the version so that we can correctly decode the mutation after it has been
    // decrypted. The count specifies the number of serialized mutations encoded in |data|.
    transactions: Vec<(JournalCheckpointV32, u64)>,

    // The encrypted mutations.
    #[serde(with = "crate::zerocopy_serialization")]
    data: Vec<u8>,

    // If the mutations key was rolled, this holds the offset in `data` where the new key should
    // apply.
    mutations_key_roll: Vec<(usize, FxfsKeyV49)>,
}

impl std::fmt::Debug for EncryptedMutations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        f.debug_struct("EncryptedMutations")
            .field("transactions", &self.transactions)
            .field("len", &self.data.len())
            .field(
                "mutations_key_roll",
                &self.mutations_key_roll.iter().map(|k| k.0).collect::<Vec<usize>>(),
            )
            .finish()
    }
}

impl Versioned for EncryptedMutations {
    fn max_serialized_size() -> Option<u64> {
        Some(MAX_ENCRYPTED_MUTATIONS_SIZE as u64)
    }
}

impl EncryptedMutations {
    fn from_replayed_mutations(
        store_object_id: u64,
        transactions: Vec<JournaledTransaction>,
    ) -> Self {
        let mut this = Self::default();
        for JournaledTransaction { checkpoint, non_root_mutations, .. } in transactions {
            for (object_id, mutation) in non_root_mutations {
                if store_object_id == object_id {
                    if let Mutation::EncryptedObjectStore(data) = mutation {
                        this.push(&checkpoint, data);
                    } else if let Mutation::UpdateMutationsKey(key) = mutation {
                        this.mutations_key_roll.push((this.data.len(), key.into()));
                    }
                }
            }
        }
        this
    }

    fn extend(&mut self, other: &EncryptedMutations) {
        self.transactions.extend_from_slice(&other.transactions[..]);
        self.mutations_key_roll.extend(
            other
                .mutations_key_roll
                .iter()
                .map(|(offset, key)| (offset + self.data.len(), key.clone())),
        );
        self.data.extend_from_slice(&other.data[..]);
    }

    fn push(&mut self, checkpoint: &JournalCheckpoint, data: Box<[u8]>) {
        let len = data.len();
        self.data.append(&mut data.into());
        if checkpoint.version >= AES_JOURNAL_ENCRYPTION_VERSION {
            // Chunks of the same transaction share the same checkpoint, so coalesce their lengths.
            if let Some((last_checkpoint, total_len)) = self.transactions.last_mut() {
                if last_checkpoint.file_offset == checkpoint.file_offset {
                    *total_len += len as u64;
                    return;
                }
            }
            self.transactions.push((checkpoint.clone(), len as u64));
        } else {
            // If the checkpoint is the same as the last mutation we pushed, increment the count.
            if let Some((last_checkpoint, count)) = self.transactions.last_mut() {
                if last_checkpoint.file_offset == checkpoint.file_offset {
                    *count += 1;
                    return;
                }
            }
            self.transactions.push((checkpoint.clone(), 1));
        }
    }
}

pub enum LockState {
    Locked,
    Unencrypted,
    Unlocked {
        crypt: Arc<dyn Crypt>,
        cached_keys: Vec<(NonZero<u64>, EncryptionKey, UnwrappedKey)>,
        mutations_cipher: JournalCipher,
    },

    // The store is unlocked, but in a read-only state, and no flushes or other operations will be
    // performed on the store.
    UnlockedReadOnly(Arc<dyn Crypt>),

    // The store is encrypted but is now in an unusable state (due to a failure to sync the journal
    // after locking the store).  The store cannot be unlocked.
    Invalid,

    // Before we've read the StoreInfo we might not know whether the store is Locked or Unencrypted.
    // This can happen when lazily opening stores (ObjectManager::lazy_open_store).
    Unknown,

    // The store is in the process of being locked.  Whilst the store is being locked, the store
    // isn't usable; assertions will trip if any mutations are applied.
    Locking,

    // Whilst we're unlocking, we will replay encrypted mutations.  The store isn't usable until
    // it's in the Unlocked state.
    Unlocking,

    // The store has been deleted.
    Deleted,
}

impl fmt::Debug for LockState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            LockState::Locked => "Locked",
            LockState::Unencrypted => "Unencrypted",
            LockState::Unlocked { .. } => "Unlocked",
            LockState::UnlockedReadOnly(..) => "UnlockedReadOnly",
            LockState::Invalid => "Invalid",
            LockState::Unknown => "Unknown",
            LockState::Locking => "Locking",
            LockState::Unlocking => "Unlocking",
            LockState::Deleted => "Deleted",
        })
    }
}

enum LastObjectId {
    // This is used when the store is encrypted, but the key and ID isn't yet available.
    Pending,

    Unencrypted {
        id: u64,
    },

    Encrypted {
        // The *unencrypted* value of the last object ID.
        id: u64,

        // Encrypted stores will use a cipher to obfuscate the object ID.
        cipher: Box<Ff1>,
    },

    Low32Bit {
        reserved: HashSet<u32>,
        unreserved: Vec<u32>,
    },
}

impl LastObjectId {
    /// Returns true if object IDs require reservations.
    fn uses_reserved_ids(&self) -> bool {
        matches!(self, LastObjectId::Low32Bit { .. })
    }

    /// Tries to get the next object ID.  Returns None if a new cipher is required because all
    /// object IDs that can be generated with the current cipher have been exhausted, or if only
    /// using the lower 32 bits which requires an async algorithm.
    fn try_get_next(&mut self) -> Option<NonZero<u64>> {
        match self {
            LastObjectId::Unencrypted { id } => {
                NonZero::new(id.wrapping_add(1)).inspect(|next| *id = next.get())
            }
            LastObjectId::Encrypted { id, cipher } => {
                let mut next = *id;
                let hi = next & OBJECT_ID_HI_MASK;
                loop {
                    if next as u32 == u32::MAX {
                        return None;
                    }
                    next += 1;
                    let candidate = hi | cipher.encrypt(next as u32) as u64;
                    if let Some(candidate) = NonZero::new(candidate) {
                        *id = next;
                        return Some(candidate);
                    }
                }
            }
            _ => None,
        }
    }

    /// Returns INVALID_OBJECT_ID if it's not possible to peek at the next object ID.
    fn peek_next(&self) -> u64 {
        match self {
            LastObjectId::Unencrypted { id } => id.wrapping_add(1),
            LastObjectId::Encrypted { id, cipher } => {
                let mut next = *id;
                let hi = next & OBJECT_ID_HI_MASK;
                loop {
                    if next as u32 == u32::MAX {
                        return INVALID_OBJECT_ID;
                    }
                    next += 1;
                    let candidate = hi | cipher.encrypt(next as u32) as u64;
                    if candidate != INVALID_OBJECT_ID {
                        return candidate;
                    }
                }
            }
            _ => INVALID_OBJECT_ID,
        }
    }

    /// Returns INVALID_OBJECT_ID for algorithms that don't use the last ID.
    fn id(&self) -> u64 {
        match self {
            LastObjectId::Unencrypted { id } | LastObjectId::Encrypted { id, .. } => *id,
            _ => INVALID_OBJECT_ID,
        }
    }

    /// Returns true if `id` is reserved (it must be 32 bits).
    fn is_reserved(&self, id: u64) -> bool {
        match self {
            LastObjectId::Low32Bit { reserved, .. } => {
                if let Ok(id) = id.try_into() {
                    reserved.contains(&id)
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Reserves `id`.
    fn reserve(&mut self, id: u64) {
        match self {
            LastObjectId::Low32Bit { reserved, .. } => {
                assert!(reserved.insert(id.try_into().unwrap()))
            }
            _ => unreachable!(),
        }
    }

    /// Unreserves `id`.
    fn unreserve(&mut self, id: u64) {
        match self {
            LastObjectId::Low32Bit { unreserved, .. } => {
                // To avoid races, where a reserved ID transitions from being reserved to being
                // actually used in a committed transaction, we delay updating `reserved` until a
                // suitable point.
                //
                // On thread A, we might have:
                //
                //   A1. Commit transaction (insert a record into the LSM tree that uses ID)
                //   A2. `unreserve`
                //
                // And on another thread B, we might have:
                //
                //   B1. Drain `unreserved`.
                //   B2. Check tree and `reserved` to see if ID is used.
                //
                // B2 will involve calling `LsmTree::layer_set` which should be thought of as a
                // snapshot, so the change A1 might not be visible to thread B, but it won't matter
                // because `reserved` will still include the ID.  So long as each thread does the
                // operations in this order, it should be safe.
                unreserved.push(id.try_into().unwrap())
            }
            _ => {}
        }
    }

    /// Removes `unreserved` IDs from the `reserved` list.
    fn drain_unreserved(&mut self) {
        match self {
            LastObjectId::Low32Bit { reserved, unreserved } => {
                for u in unreserved.drain(..) {
                    assert!(reserved.remove(&u));
                }
            }
            _ => {}
        }
    }
}

pub struct ReservedId<'a>(&'a ObjectStore, NonZero<u64>);

impl<'a> ReservedId<'a> {
    pub fn new(store: &'a ObjectStore, id: NonZero<u64>) -> Self {
        Self(store, id)
    }

    pub fn get(&self) -> u64 {
        self.1.get()
    }

    /// The caller takes responsibility for this id.
    #[must_use]
    pub fn release(self) -> NonZero<u64> {
        let id = self.1;
        std::mem::forget(self);
        id
    }
}

impl Drop for ReservedId<'_> {
    fn drop(&mut self) {
        self.0.last_object_id.lock().unreserve(self.1.get());
    }
}

/// An object store supports a file like interface for objects.  Objects are keyed by a 64 bit
/// identifier.  And object store has to be backed by a parent object store (which stores metadata
/// for the object store).  The top-level object store (a.k.a. the root parent object store) is
/// in-memory only.
pub struct ObjectStore {
    parent_store: Option<Arc<ObjectStore>>,
    store_object_id: u64,
    device: Arc<dyn Device>,
    block_size: BlockSize,
    filesystem: Weak<FxFilesystem>,
    // Lock ordering: This must be taken before `lock_state`.
    store_info: Mutex<Option<StoreInfo>>,
    tree: LSMTree<ObjectKey, ObjectValue>,

    // When replaying the journal, the store cannot read StoreInfo until the whole journal
    // has been replayed, so during that time, store_info_handle will be None and records
    // just get sent to the tree. Once the journal has been replayed, we can open the store
    // and load all the other layer information.
    store_info_handle: OnceLock<DataObjectHandle<ObjectStore>>,

    // Current lock state of the store.
    // Lock ordering: This must be taken after `store_info`.
    lock_state: Mutex<LockState>,
    pub key_manager: KeyManager,

    // Enable/disable tracing.
    trace: AtomicBool,

    // Informational counters for events occurring within the store.
    counters: Mutex<ObjectStoreCounters>,

    // These are updated in performance-sensitive code paths so we use atomics instead of counters.
    device_read_ops: AtomicU64,
    device_write_ops: AtomicU64,
    logical_read_ops: AtomicU64,
    logical_write_ops: AtomicU64,
    graveyard_entries: AtomicU64,

    // Contains the last object ID and, optionally, a cipher to be used when generating new object
    // IDs.
    last_object_id: Mutex<LastObjectId>,

    // An optional callback to be invoked each time the ObjectStore flushes.  The callback is
    // invoked at the end of flush, while the write lock is still held.
    flush_callback: Mutex<Option<Box<dyn Fn(&ObjectStore) + Send + Sync + 'static>>>,
}

#[derive(Clone, Default)]
struct ObjectStoreCounters {
    mutations_applied: u64,
    mutations_dropped: u64,
    num_flushes: u64,
    last_flush_time: Option<std::time::SystemTime>,
}

impl ObjectStore {
    fn new(
        parent_store: Option<Arc<ObjectStore>>,
        store_object_id: u64,
        filesystem: Arc<FxFilesystem>,
        store_info: Option<StoreInfo>,
        object_cache: Option<Box<dyn ObjectCache<ObjectKey, ObjectValue>>>,
        lock_state: LockState,
        last_object_id: LastObjectId,
    ) -> Arc<ObjectStore> {
        let device = filesystem.device();
        let block_size = filesystem.block_size();
        Arc::new(ObjectStore {
            parent_store,
            store_object_id,
            device,
            block_size,
            filesystem: Arc::downgrade(&filesystem),
            store_info: Mutex::new(store_info),
            tree: LSMTree::new(merge::merge, object_cache),
            store_info_handle: OnceLock::new(),
            lock_state: Mutex::new(lock_state),
            key_manager: KeyManager::new(),
            trace: AtomicBool::new(false),
            counters: Mutex::new(ObjectStoreCounters::default()),
            device_read_ops: AtomicU64::new(0),
            device_write_ops: AtomicU64::new(0),
            logical_read_ops: AtomicU64::new(0),
            logical_write_ops: AtomicU64::new(0),
            graveyard_entries: AtomicU64::new(0),
            last_object_id: Mutex::new(last_object_id),
            flush_callback: Mutex::new(None),
        })
    }

    fn new_empty(
        parent_store: Option<Arc<ObjectStore>>,
        store_object_id: u64,
        filesystem: Arc<FxFilesystem>,
        object_cache: Option<Box<dyn ObjectCache<ObjectKey, ObjectValue>>>,
    ) -> Arc<Self> {
        Self::new(
            parent_store,
            store_object_id,
            filesystem,
            Some(StoreInfo::default()),
            object_cache,
            LockState::Unencrypted,
            LastObjectId::Unencrypted { id: 0 },
        )
    }

    /// Cycle breaker constructor that returns an ObjectStore without a filesystem.
    /// This should only be used from super block code.
    pub fn new_root_parent(
        device: Arc<dyn Device>,
        block_size: BlockSize,
        store_object_id: u64,
    ) -> Self {
        ObjectStore {
            parent_store: None,
            store_object_id,
            device,
            block_size,
            filesystem: Weak::<FxFilesystem>::new(),
            store_info: Mutex::new(Some(StoreInfo::default())),
            tree: LSMTree::new(merge::merge, None),
            store_info_handle: OnceLock::new(),
            lock_state: Mutex::new(LockState::Unencrypted),
            key_manager: KeyManager::new(),
            trace: AtomicBool::new(false),
            counters: Mutex::new(ObjectStoreCounters::default()),
            device_read_ops: AtomicU64::new(0),
            device_write_ops: AtomicU64::new(0),
            logical_read_ops: AtomicU64::new(0),
            logical_write_ops: AtomicU64::new(0),
            graveyard_entries: AtomicU64::new(0),
            last_object_id: Mutex::new(LastObjectId::Unencrypted { id: 0 }),
            flush_callback: Mutex::new(None),
        }
    }

    /// Used to set filesystem on root_parent stores at bootstrap time after the filesystem has
    /// been created.
    pub fn attach_filesystem(mut this: ObjectStore, filesystem: Arc<FxFilesystem>) -> ObjectStore {
        this.filesystem = Arc::downgrade(&filesystem);
        this
    }

    /// Acquires appropriate locks and starts a new transaction.  A transaction should be associated
    /// with a store, even though it may have mutations for other objects and parent stores.
    pub async fn new_transaction<'a>(
        &self,
        locks: LockKeys,
        options: Options<'a>,
    ) -> Result<Transaction<'a>, Error> {
        let fs = self.filesystem();
        Transaction::new(fs, options, locks).await
    }

    /// Ensures that `cached_keys` is filled up to `CACHED_KEYS_LIMIT`.
    async fn pre_cache_keys(&self) -> Result<(), Error> {
        let crypt = match &*self.lock_state.lock() {
            LockState::Unlocked { cached_keys, crypt, .. }
                if cached_keys.len() < CACHED_KEYS_LIMIT =>
            {
                crypt.clone()
            }
            _ => return Ok(()),
        };
        loop {
            let parent_store = self.parent_store.as_ref().unwrap();

            // We assert that the parent store is not using object IDs that need reservation, since
            // if it did, release() would leak the reservation below.
            assert!(!parent_store.last_object_id.lock().uses_reserved_ids());

            // Allocate a raw ID from the parent store.  Since the parent store is unencrypted, this
            // is fast and won't block.
            let raw_id = {
                let reserved_id = parent_store
                    .maybe_get_next_object_id()
                    .expect("maybe_get_next_object_id failed on parent store");
                reserved_id.release()
            };

            let (wrapped, unwrapped) = match crypt.create_key(raw_id.get(), KeyPurpose::Data).await
            {
                Ok(v) => v,
                Err(error) => {
                    log::warn!(
                        error:?,
                        store_id = self.store_object_id();
                        "Failed to pre-cache key"
                    );
                    return Err(error.into());
                }
            };

            let mut lock_state = self.lock_state.lock();
            if let LockState::Unlocked { cached_keys, .. } = &mut *lock_state {
                cached_keys.push((raw_id, EncryptionKey::Fxfs(wrapped), unwrapped));
                if cached_keys.len() >= CACHED_KEYS_LIMIT {
                    break;
                }
            } else {
                // Store was locked while we were awaiting; discard the key.
                break;
            }
        }
        Ok(())
    }

    /// Create a child store. It is a multi-step process:
    ///
    ///   1. Call `ObjectStore::new_child_store`.
    ///   2. Register the store with the object-manager.
    ///   3. Call `ObjectStore::create` to write the store-info.
    ///
    /// If the procedure fails, care must be taken to unregister store with the object-manager.
    ///
    /// The steps have to be separate because of lifetime issues when working with a transaction.
    async fn new_child_store(
        self: &Arc<Self>,
        transaction: &mut Transaction<'_>,
        options: NewChildStoreOptions,
        object_cache: Option<Box<dyn ObjectCache<ObjectKey, ObjectValue>>>,
    ) -> Result<Arc<Self>, Error> {
        ensure!(
            !options.reserve_32bit_object_ids || !options.low_32_bit_object_ids,
            FxfsError::InvalidArgs
        );
        let handle = if let Some(object_id) = NonZero::new(options.object_id) {
            self.update_last_object_id(object_id.get());
            let handle = ObjectStore::create_object_with_id(
                self,
                transaction,
                ReservedId::new(self, object_id),
                HandleOptions::default(),
                None,
            )?;
            handle
        } else {
            ObjectStore::create_object(self, transaction, HandleOptions::default(), None).await?
        };
        let filesystem = self.filesystem();
        let id = if options.reserve_32bit_object_ids { 0x1_0000_0000 } else { 0 };
        let (last_object_id, last_object_id_in_memory) = if options.low_32_bit_object_ids {
            (
                LastObjectIdInfo::Low32Bit,
                LastObjectId::Low32Bit { reserved: HashSet::new(), unreserved: Vec::new() },
            )
        } else if let Some(crypt) = &options.options.crypt {
            let (object_id_wrapped, object_id_unwrapped) =
                crypt.create_key(handle.object_id(), KeyPurpose::Metadata).await?;
            (
                LastObjectIdInfo::Encrypted { id, key: object_id_wrapped },
                LastObjectId::Encrypted { id, cipher: Box::new(Ff1::new(&object_id_unwrapped)) },
            )
        } else {
            (LastObjectIdInfo::Unencrypted { id }, LastObjectId::Unencrypted { id })
        };
        let store = if let Some(crypt) = options.options.crypt {
            let (wrapped_key, unwrapped_key) =
                crypt.create_key(handle.object_id(), KeyPurpose::Metadata).await?;
            Self::new(
                Some(self.clone()),
                handle.object_id(),
                filesystem.clone(),
                Some(StoreInfo {
                    mutations_key: Some(wrapped_key),
                    last_object_id,
                    guid: options.guid.unwrap_or_else(|| *Uuid::new_v4().as_bytes()),
                    ..Default::default()
                }),
                object_cache,
                LockState::Unlocked {
                    crypt,
                    cached_keys: Vec::new(),
                    mutations_cipher: JournalCipher::new_aes256_xts(&unwrapped_key, 0),
                },
                last_object_id_in_memory,
            )
        } else {
            Self::new(
                Some(self.clone()),
                handle.object_id(),
                filesystem.clone(),
                Some(StoreInfo {
                    last_object_id,
                    guid: options.guid.unwrap_or_else(|| *Uuid::new_v4().as_bytes()),
                    ..Default::default()
                }),
                object_cache,
                LockState::Unencrypted,
                last_object_id_in_memory,
            )
        };
        assert!(store.store_info_handle.set(handle).is_ok());
        Ok(store)
    }

    /// Actually creates the store in a transaction.  This will also create a root directory and
    /// graveyard directory for the store.  See `new_child_store` above.
    async fn create<'a>(
        self: &'a Arc<Self>,
        transaction: &mut Transaction<'a>,
    ) -> Result<(), Error> {
        let buf = {
            // Create a root directory and graveyard directory.
            let graveyard_directory_object_id = Graveyard::create(transaction, &self).await?;
            let root_directory = Directory::create(transaction, &self, None).await?;

            let serialized_info = {
                let mut store_info = self.store_info.lock();
                let store_info = store_info.as_mut().unwrap();

                store_info.graveyard_directory_object_id = graveyard_directory_object_id;
                store_info.root_directory_object_id = root_directory.object_id();

                let mut serialized_info = Vec::new();
                store_info.serialize_with_version(&mut serialized_info)?;
                serialized_info
            };
            let mut buf = self.device.allocate_buffer(serialized_info.len()).await;
            buf.copy_from_slice(&serialized_info[..]);
            buf
        };

        if self.filesystem().options().image_builder_mode.is_some() {
            // If we're in image builder mode, we want to avoid writing to disk unless explicitly
            // asked to. New object stores will have their StoreInfo written when we compact in
            // FxFilesystem::finalize().
            Ok(())
        } else {
            self.store_info_handle.get().unwrap().txn_write(transaction, 0u64, buf.as_ref()).await
        }
    }

    pub fn set_trace(&self, trace: bool) {
        let old_value = self.trace.swap(trace, Ordering::Relaxed);
        if trace != old_value {
            info!(store_id = self.store_object_id(), trace; "OS: trace",);
        }
    }

    /// Sets a callback to be invoked each time the ObjectStore flushes.  The callback is invoked at
    /// the end of flush, while the write lock is still held.
    pub fn set_flush_callback<F: Fn(&ObjectStore) + Send + Sync + 'static>(&self, callback: F) {
        let mut flush_callback = self.flush_callback.lock();
        *flush_callback = Some(Box::new(callback));
    }

    pub fn is_root(&self) -> bool {
        if let Some(parent) = &self.parent_store {
            parent.parent_store.is_none()
        } else {
            // The root parent store isn't the root store.
            false
        }
    }

    /// Populates an inspect node with store statistics.
    pub fn record_data(self: &Arc<Self>, root: &fuchsia_inspect::Node) {
        // TODO(https://fxbug.dev/42069513): Push-back or rate-limit to prevent DoS.
        let counters = self.counters.lock();
        if let Some(store_info) = self.store_info() {
            root.record_string("guid", Uuid::from_bytes(store_info.guid).to_string());
        };
        root.record_uint("store_object_id", self.store_object_id);
        root.record_uint("mutations_applied", counters.mutations_applied);
        root.record_uint("mutations_dropped", counters.mutations_dropped);
        root.record_uint("num_flushes", counters.num_flushes);
        if let Some(last_flush_time) = counters.last_flush_time.as_ref() {
            root.record_uint(
                "last_flush_time_ms",
                last_flush_time
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or(std::time::Duration::ZERO)
                    .as_millis()
                    .try_into()
                    .unwrap_or(0u64),
            );
        }
        root.record_uint("device_read_ops", self.device_read_ops.load(Ordering::Relaxed));
        root.record_uint("device_write_ops", self.device_write_ops.load(Ordering::Relaxed));
        root.record_uint("logical_read_ops", self.logical_read_ops.load(Ordering::Relaxed));
        root.record_uint("logical_write_ops", self.logical_write_ops.load(Ordering::Relaxed));
        root.record_uint("graveyard_entries", self.graveyard_entries.load(Ordering::Relaxed));
        {
            let last_object_id = self.last_object_id.lock();
            root.record_uint("object_id_hi", last_object_id.id() >> 32);
            root.record_bool(
                "low_32_bit_object_ids",
                matches!(&*last_object_id, LastObjectId::Low32Bit { .. }),
            );
        }

        let this = self.clone();
        root.record_child("lsm_tree", move |node| this.tree().record_inspect_data(node));
    }

    pub fn device(&self) -> &Arc<dyn Device> {
        &self.device
    }

    pub fn block_size(&self) -> BlockSize {
        self.block_size
    }

    pub fn filesystem(&self) -> Arc<FxFilesystem> {
        self.filesystem.upgrade().unwrap()
    }

    pub fn store_object_id(&self) -> u64 {
        self.store_object_id
    }

    pub fn tree(&self) -> &LSMTree<ObjectKey, ObjectValue> {
        &self.tree
    }

    pub fn root_directory_object_id(&self) -> u64 {
        self.store_info.lock().as_ref().unwrap().root_directory_object_id
    }

    pub fn guid(&self) -> [u8; 16] {
        self.store_info.lock().as_ref().unwrap().guid
    }

    pub fn graveyard_directory_object_id(&self) -> u64 {
        self.store_info.lock().as_ref().unwrap().graveyard_directory_object_id
    }

    fn set_graveyard_directory_object_id(&self, oid: u64) {
        assert_eq!(
            std::mem::replace(
                &mut self.store_info.lock().as_mut().unwrap().graveyard_directory_object_id,
                oid
            ),
            INVALID_OBJECT_ID
        );
    }

    pub fn object_count(&self) -> u64 {
        self.store_info.lock().as_ref().unwrap().object_count
    }

    /// Returns INVALID_OBJECT_ID for algorithms that don't use the last ID.
    pub(crate) fn unencrypted_last_object_id(&self) -> u64 {
        self.last_object_id.lock().id()
    }

    pub fn key_manager(&self) -> &KeyManager {
        &self.key_manager
    }

    /// Clears all in-memory caches (LSM tree object cache, persistent layer chunk caches, and
    /// non-permanent unwrapped keys) for this object store.
    pub fn clear_caches(&self) {
        self.tree.clear_cache();
        self.key_manager.clear_cached_keys();
    }

    pub fn parent_store(&self) -> Option<&Arc<ObjectStore>> {
        self.parent_store.as_ref()
    }

    /// Returns the crypt object for the store.  Returns None if the store is unencrypted.
    pub fn crypt(&self) -> Option<Arc<dyn Crypt>> {
        match &*self.lock_state.lock() {
            LockState::Locked => panic!("Store is locked"),
            LockState::Invalid
            | LockState::Unencrypted
            | LockState::Locking
            | LockState::Unlocking
            | LockState::Deleted => None,
            LockState::Unlocked { crypt, .. } => Some(crypt.clone()),
            LockState::UnlockedReadOnly(crypt) => Some(crypt.clone()),
            LockState::Unknown => {
                panic!("Store is of unknown lock state; has the journal been replayed yet?")
            }
        }
    }

    /// Returns the id of the internal directory. Returns a NotFound error if this has not been
    /// initialized.
    pub fn get_internal_directory_id(self: &Arc<Self>) -> Result<u64, Error> {
        if let Some(store_info) = self.store_info.lock().as_ref() {
            if store_info.internal_directory_object_id == INVALID_OBJECT_ID {
                Err(FxfsError::NotFound.into())
            } else {
                Ok(store_info.internal_directory_object_id)
            }
        } else {
            Err(FxfsError::Unavailable.into())
        }
    }

    pub async fn get_or_create_internal_directory_id(self: &Arc<Self>) -> Result<u64, Error> {
        // Create the transaction first to use the object store lock.
        let mut transaction = self
            .new_transaction(
                lock_keys![LockKey::InternalDirectory { store_object_id: self.store_object_id }],
                Options::default(),
            )
            .await?;
        let obj_id = self.store_info.lock().as_ref().unwrap().internal_directory_object_id;
        if obj_id != INVALID_OBJECT_ID {
            return Ok(obj_id);
        }

        // Need to create an internal directory.
        let directory = Directory::create(&mut transaction, self, None).await?;

        transaction.add(self.store_object_id, Mutation::CreateInternalDir(directory.object_id()));
        transaction.commit().await?;
        Ok(directory.object_id())
    }

    /// Returns the file size for the object without opening the object.
    async fn get_file_size(&self, object_id: u64) -> Result<u64, Error> {
        self.tree
            .find_map(
                &ObjectKey::attribute(object_id, AttributeId::DATA, AttributeKey::Attribute),
                |item| match item.value {
                    ObjectValue::Attribute { size, .. } => Ok(*size),
                    _ => Err(anyhow!(FxfsError::NotFile)),
                },
            )
            .await?
            .ok_or(FxfsError::NotFound)?
    }

    #[cfg(feature = "migration")]
    pub fn last_object_id(&self) -> u64 {
        self.last_object_id.lock().id()
    }

    /// Provides access to the allocator to mark a specific region of the device as allocated.
    #[cfg(feature = "migration")]
    pub fn mark_allocated(
        &self,
        transaction: &mut Transaction<'_>,
        store_object_id: u64,
        device_range: std::ops::Range<u64>,
    ) -> Result<(), Error> {
        self.allocator().mark_allocated(transaction, store_object_id, device_range)
    }

    /// `crypt` can be provided if the crypt service should be different to the default; see the
    /// comment on create_object.  Users should avoid having more than one handle open for the same
    /// object at the same time because they might get out-of-sync; there is no code that will
    /// prevent this.  One example where this can cause an issue is if the object ends up using a
    /// permanent key (which is the case if a value is passed for `crypt`), the permanent key is
    /// dropped when a handle is dropped, which will impact any other handles for the same object.
    pub async fn open_object<S: HandleOwner>(
        owner: &Arc<S>,
        obj_id: u64,
        options: HandleOptions,
        crypt: Option<Arc<dyn Crypt>>,
    ) -> Result<DataObjectHandle<S>, Error> {
        let store = owner.as_ref().as_ref();
        let mut fsverity_descriptor = None;
        let mut overwrite_ranges = Vec::new();
        let value = store
            .tree
            .find_value(&ObjectKey::attribute(obj_id, AttributeId::DATA, AttributeKey::Attribute))
            .await?
            .ok_or(FxfsError::NotFound)?;

        let (size, track_overwrite_extents) = match value {
            ObjectValue::Attribute { size, has_overwrite_extents } => (size, has_overwrite_extents),
            ObjectValue::VerifiedAttribute { size, fsverity_metadata } => {
                if !options.skip_fsverity {
                    fsverity_descriptor = Some(fsverity_metadata);
                }
                // We only track the overwrite extents in memory for writes, reads handle them
                // implicitly, which means verified files (where the data won't change anymore)
                // don't need to track them.
                (size, false)
            }
            _ => bail!(anyhow!(FxfsError::Inconsistent).context("open_object: Expected attibute")),
        };

        ensure!(size <= MAX_FILE_SIZE, FxfsError::Inconsistent);

        if track_overwrite_extents {
            let layer_set = store.tree.layer_set();
            let mut merger = layer_set.merger();
            let mut iter = merger
                .query(Query::FullRange(&ObjectKey::attribute(
                    obj_id,
                    AttributeId::DATA,
                    AttributeKey::Extent(Extent::search_key_from_offset(0)),
                )))
                .await?;
            loop {
                match iter.get() {
                    Some(ItemRef {
                        key:
                            ObjectKey {
                                object_id,
                                data:
                                    ObjectKeyData::Attribute(
                                        AttributeId::DATA,
                                        AttributeKey::Extent(extent),
                                    ),
                            },
                        value,
                        ..
                    }) if *object_id == obj_id => {
                        match value {
                            ObjectValue::Extent(ExtentValue::None)
                            | ObjectValue::Extent(ExtentValue::Some {
                                mode: ExtentMode::Raw,
                                ..
                            })
                            | ObjectValue::Extent(ExtentValue::Some {
                                mode: ExtentMode::Cow(_),
                                ..
                            }) => (),
                            ObjectValue::Extent(ExtentValue::Some {
                                mode: ExtentMode::OverwritePartial(_),
                                ..
                            })
                            | ObjectValue::Extent(ExtentValue::Some {
                                mode: ExtentMode::Overwrite,
                                ..
                            }) => overwrite_ranges.push(extent.clone().into()),
                            _ => bail!(
                                anyhow!(FxfsError::Inconsistent)
                                    .context("open_object: Expected extent")
                            ),
                        }
                        iter.advance().await?;
                    }
                    _ => break,
                }
            }
        }

        // If a crypt service has been specified, it needs to be a permanent key because cached
        // keys can only use the store's crypt service.
        let permanent = if let Some(crypt) = crypt {
            store
                .key_manager
                .get_keys(
                    obj_id,
                    crypt.as_ref(),
                    &mut Some(async || store.get_keys(obj_id).await),
                    /* permanent= */ true,
                    /* force= */ false,
                )
                .await?;
            true
        } else {
            false
        };
        let data_object_handle = DataObjectHandle::new(
            owner.clone(),
            obj_id,
            permanent,
            AttributeId::DATA,
            size,
            options,
            false,
            &overwrite_ranges,
        );
        if let Some(descriptor) = fsverity_descriptor {
            data_object_handle
                .set_fsverity_state_some(descriptor)
                .await
                .context("Invalid or mismatched merkle tree")?;
        }
        Ok(data_object_handle)
    }

    pub fn create_object_with_id<S: HandleOwner>(
        owner: &Arc<S>,
        transaction: &mut Transaction<'_>,
        reserved_object_id: ReservedId<'_>,
        options: HandleOptions,
        encryption_options: Option<ObjectEncryptionOptions>,
    ) -> Result<DataObjectHandle<S>, Error> {
        let store = owner.as_ref().as_ref();
        // Don't permit creating unencrypted objects in an encrypted store.  The converse is OK.
        debug_assert!(store.crypt().is_none() || encryption_options.is_some());
        let now = Timestamp::now();
        let object_id = reserved_object_id.get();
        assert!(
            transaction
                .add(
                    store.store_object_id(),
                    Mutation::insert_object(
                        ObjectKey::object(reserved_object_id.release().get()),
                        ObjectValue::file(
                            1,
                            0,
                            now.clone(),
                            now.clone(),
                            now.clone(),
                            now,
                            None,
                            None
                        ),
                    ),
                )
                .is_none()
        );
        let mut permanent_keys = false;
        if let Some(ObjectEncryptionOptions { permanent, key_id, key, unwrapped_key }) =
            encryption_options
        {
            permanent_keys = permanent;
            let cipher = key_to_cipher(&key, &unwrapped_key)?;
            transaction.add(
                store.store_object_id(),
                Mutation::insert_object(
                    ObjectKey::keys(object_id),
                    ObjectValue::keys(vec![(key_id, key)].into()),
                ),
            );
            store.key_manager.insert(
                object_id,
                Arc::new(vec![(key_id, CipherHolder::Cipher(cipher))].into()),
                permanent,
            );
        }
        transaction.add(
            store.store_object_id(),
            Mutation::insert_object(
                ObjectKey::attribute(object_id, AttributeId::DATA, AttributeKey::Attribute),
                // This is a new object so nothing has pre-allocated overwrite extents yet.
                ObjectValue::attribute(0, false),
            ),
        );
        Ok(DataObjectHandle::new(
            owner.clone(),
            object_id,
            permanent_keys,
            AttributeId::DATA,
            0,
            options,
            false,
            &[],
        ))
    }

    /// Creates an object in the store.
    ///
    /// If the store is encrypted, the object will be automatically encrypted as well.
    /// If `wrapping_key_id` is set, the new keys will be wrapped with that specific key, and
    /// otherwise the default data key is used.
    pub async fn create_object<S: HandleOwner>(
        owner: &Arc<S>,
        mut transaction: &mut Transaction<'_>,
        options: HandleOptions,
        wrapping_key_id: Option<WrappingKeyId>,
    ) -> Result<DataObjectHandle<S>, Error> {
        let store = owner.as_ref().as_ref();
        let object_id = store.get_next_object_id().await?;
        let crypt = store.crypt();
        let encryption_options = if let Some(crypt) = crypt {
            let key_id =
                if wrapping_key_id.is_some() { FSCRYPT_KEY_ID } else { VOLUME_DATA_KEY_ID };
            let (key, unwrapped_key) = if let Some(wrapping_key_id) = wrapping_key_id {
                crypt.create_key_with_id(object_id.get(), wrapping_key_id, ObjectType::File).await?
            } else {
                let (fxfs_key, unwrapped_key) =
                    crypt.create_key(object_id.get(), KeyPurpose::Data).await?;
                (EncryptionKey::Fxfs(fxfs_key), unwrapped_key)
            };
            Some(ObjectEncryptionOptions { permanent: false, key_id, key, unwrapped_key })
        } else {
            None
        };
        ObjectStore::create_object_with_id(
            owner,
            &mut transaction,
            object_id,
            options,
            encryption_options,
        )
    }

    /// Creates an object using explicitly provided keys.
    ///
    /// There are some cases where an encrypted object needs to be created in an unencrypted store.
    /// For example, when layer files for a child store are created in the root store, but they must
    /// be encrypted using the child store's keys.  This method exists for that purpose.
    pub(crate) async fn create_object_with_key<S: HandleOwner>(
        owner: &Arc<S>,
        mut transaction: &mut Transaction<'_>,
        object_id: ReservedId<'_>,
        options: HandleOptions,
        key: EncryptionKey,
        unwrapped_key: UnwrappedKey,
    ) -> Result<DataObjectHandle<S>, Error> {
        ObjectStore::create_object_with_id(
            owner,
            &mut transaction,
            object_id,
            options,
            Some(ObjectEncryptionOptions {
                permanent: true,
                key_id: VOLUME_DATA_KEY_ID,
                key,
                unwrapped_key,
            }),
        )
    }

    /// Adjusts the reference count for a given object.  If the reference count reaches zero, the
    /// object is moved into the graveyard and true is returned.
    pub async fn adjust_refs(
        &self,
        transaction: &mut Transaction<'_>,
        object_id: u64,
        delta: i64,
    ) -> Result<bool, Error> {
        self.adjust_refs_impl(transaction, object_id, delta, true).await
    }

    /// Adjusts the reference count for a given object.  If the reference count reaches zero and
    /// `add_to_graveyard` is true, the object is moved into the graveyard.  Returns whether the
    /// reference count reached zero.
    pub async fn adjust_refs_impl(
        &self,
        transaction: &mut Transaction<'_>,
        object_id: u64,
        delta: i64,
        add_to_graveyard: bool,
    ) -> Result<bool, Error> {
        let mut mutation = self.txn_get_object_mutation(transaction, object_id).await?;
        let refs = if let ObjectValue::Object {
            kind:
                ObjectKind::File { refs, .. }
                | ObjectKind::Symlink { refs, .. }
                | ObjectKind::EncryptedSymlink { refs, .. },
            ..
        } = &mut mutation.item.value
        {
            *refs =
                refs.checked_add_signed(delta).ok_or_else(|| anyhow!("refs underflow/overflow"))?;
            refs
        } else {
            bail!(FxfsError::NotFile);
        };
        if *refs == 0 {
            if add_to_graveyard {
                self.add_to_graveyard(transaction, object_id);

                // We might still need to adjust the reference count if delta was something other
                // than -1.
                if delta != -1 {
                    *refs = 1;
                    transaction.add(self.store_object_id, Mutation::ObjectStore(mutation));
                }

                // Otherwise, we don't commit the mutation as we want to keep reference count as 1
                // for objects in graveyard.
            }
            Ok(true)
        } else {
            transaction.add(self.store_object_id, Mutation::ObjectStore(mutation));
            Ok(false)
        }
    }

    /// Purges an object that is in the graveyard. If a truncate guard is not provided, one will be
    /// acquired.
    pub async fn tombstone_object(
        &self,
        object_id: u64,
        txn_options: Options<'_>,
        truncate_guard: Option<&TruncateGuard<'_>>,
    ) -> Result<(), Error> {
        debug_assert!(
            self.tree.exists(&ObjectKey::object(object_id)).await?,
            "Tombstoning missing object"
        );
        debug_assert!(
            self.tree
                .exists(&ObjectKey::graveyard_entry(
                    self.graveyard_directory_object_id(),
                    object_id
                ))
                .await?,
            "Tombstoning object not in graveyard"
        );
        self.key_manager.remove(object_id).await;
        let _fs;
        let _guard;
        let truncate_guard = match truncate_guard {
            Some(guard) => guard,
            None => {
                _fs = self.filesystem();
                _guard = _fs.truncate_guard(self.store_object_id, object_id).await;
                &_guard
            }
        };
        self.trim_or_tombstone(object_id, true, txn_options, truncate_guard).await
    }

    /// Trim extents beyond the end of a file for all attributes.  This will remove the entry from
    /// the graveyard when done.
    pub async fn trim(
        &self,
        object_id: u64,
        truncate_guard: &TruncateGuard<'_>,
    ) -> Result<(), Error> {
        // For the root and root parent store, we would need to use the metadata reservation which
        // we don't currently support, so assert that we're not those stores.
        assert!(self.parent_store.as_ref().unwrap().parent_store.is_some());

        self.trim_or_tombstone(
            object_id,
            false,
            Options { borrow_metadata_space: true, ..Default::default() },
            truncate_guard,
        )
        .await
    }

    /// Trims or tombstones an object.
    async fn trim_or_tombstone(
        &self,
        object_id: u64,
        for_tombstone: bool,
        txn_options: Options<'_>,
        _truncate_guard: &TruncateGuard<'_>,
    ) -> Result<(), Error> {
        let mut next_attribute = Some(AttributeId::SORTED_START);
        while let Some(attribute_id) = next_attribute.take() {
            let mut transaction = self
                .new_transaction(
                    lock_keys![
                        LockKey::object_attribute(self.store_object_id, object_id, attribute_id),
                        LockKey::object(self.store_object_id, object_id),
                    ],
                    txn_options,
                )
                .await?;

            match self
                .trim_some(
                    &mut transaction,
                    object_id,
                    attribute_id,
                    if for_tombstone {
                        TrimMode::Tombstone(TombstoneMode::Object)
                    } else {
                        TrimMode::UseSize
                    },
                )
                .await?
            {
                TrimResult::Incomplete => next_attribute = Some(attribute_id),
                TrimResult::Done(None) => {
                    if for_tombstone
                        || matches!(
                            self.tree
                                .find_value(&ObjectKey::graveyard_entry(
                                    self.graveyard_directory_object_id(),
                                    object_id,
                                ))
                                .await?,
                            Some(ObjectValue::Trim)
                        )
                    {
                        self.remove_from_graveyard(&mut transaction, object_id);
                    }
                    // The last attribute was not the default attribute, it may have been added to
                    // the graveyard alongside the object.
                    if for_tombstone && attribute_id != AttributeId::DATA {
                        self.remove_attribute_from_graveyard(
                            &mut transaction,
                            object_id,
                            attribute_id,
                        );
                    }
                }
                TrimResult::Done(id) => {
                    // Moved to the next attribute. This one is finished and it may have been
                    // added to the graveyard alongside the object.
                    if for_tombstone && attribute_id != AttributeId::DATA {
                        self.remove_attribute_from_graveyard(
                            &mut transaction,
                            object_id,
                            attribute_id,
                        );
                    }
                    next_attribute = id;
                }
            }

            if !transaction.mutations().is_empty() {
                transaction.commit().await?;
            }
        }
        Ok(())
    }

    // Purges an object's attribute that is in the graveyard.
    pub async fn tombstone_attribute(
        &self,
        object_id: u64,
        attribute_id: AttributeId,
        txn_options: Options<'_>,
    ) -> Result<(), Error> {
        // Ensure that we don't double-delete things, it should still exist and be in the graveyard.
        debug_assert!(
            self.tree
                .exists(&ObjectKey::attribute(object_id, attribute_id, AttributeKey::Attribute))
                .await?,
            "Tombstoning missing attribute"
        );
        debug_assert!(
            self.tree
                .exists(&ObjectKey::graveyard_attribute_entry(
                    self.graveyard_directory_object_id(),
                    object_id,
                    attribute_id
                ))
                .await?,
            "Tombstoning attribute not in graveyard"
        );
        let mut trim_result = TrimResult::Incomplete;
        while matches!(trim_result, TrimResult::Incomplete) {
            let mut transaction = self
                .new_transaction(
                    lock_keys![
                        LockKey::object_attribute(self.store_object_id, object_id, attribute_id),
                        LockKey::object(self.store_object_id, object_id),
                    ],
                    txn_options,
                )
                .await?;
            trim_result = self
                .trim_some(
                    &mut transaction,
                    object_id,
                    attribute_id,
                    TrimMode::Tombstone(TombstoneMode::Attribute),
                )
                .await?;
            if let TrimResult::Done(..) = trim_result {
                self.remove_attribute_from_graveyard(&mut transaction, object_id, attribute_id)
            }
            if !transaction.mutations().is_empty() {
                transaction.commit().await?;
            }
        }
        Ok(())
    }

    /// Deletes extents for attribute `attribute_id` in object `object_id`.  Also see the comments
    /// for TrimMode and TrimResult. Should hold a lock on the attribute, and the object as it
    /// performs a read-modify-write on the sizes.
    pub async fn trim_some(
        &self,
        transaction: &mut Transaction<'_>,
        object_id: u64,
        attribute_id: AttributeId,
        mode: TrimMode,
    ) -> Result<TrimResult, Error> {
        let layer_set = self.tree.layer_set();
        let mut merger = layer_set.merger();

        let aligned_offset = match mode {
            TrimMode::FromOffset(offset) => {
                self.block_size.align_up(offset).ok_or(FxfsError::Inconsistent)?
            }
            TrimMode::Tombstone(..) => 0,
            TrimMode::UseSize => {
                let iter = merger
                    .query(Query::FullRange(&ObjectKey::attribute(
                        object_id,
                        attribute_id,
                        AttributeKey::Attribute,
                    )))
                    .await?;
                if let Some(item_ref) = iter.get() {
                    if item_ref.key.object_id != object_id {
                        return Ok(TrimResult::Done(None));
                    }

                    if let ItemRef {
                        key:
                            ObjectKey {
                                data:
                                    ObjectKeyData::Attribute(size_attribute_id, AttributeKey::Attribute),
                                ..
                            },
                        value: ObjectValue::Attribute { size, .. },
                        ..
                    } = item_ref
                    {
                        // If we found a different attribute_id, return so we can get the
                        // right lock.
                        if *size_attribute_id != attribute_id {
                            return Ok(TrimResult::Done(Some(*size_attribute_id)));
                        }
                        self.block_size.align_up(*size).ok_or(FxfsError::Inconsistent)?
                    } else {
                        // At time of writing, we should always see a size record or None here, but
                        // asserting here would be brittle so just skip to the the next attribute
                        // instead.
                        return Ok(TrimResult::Done(Some(attribute_id.next())));
                    }
                } else {
                    // End of the tree.
                    return Ok(TrimResult::Done(None));
                }
            }
        };

        // Loop over the extents and deallocate them.
        let mut iter = merger
            .query(Query::FullRange(&ObjectKey::from_extent(
                object_id,
                attribute_id,
                Extent::search_key_from_offset(aligned_offset),
            )))
            .await?;
        let mut end = 0;
        let allocator = self.allocator();
        let mut result = TrimResult::Done(None);
        let mut deallocated = 0;
        let block_size = self.block_size;

        while let Some(item_ref) = iter.get() {
            if item_ref.key.object_id != object_id {
                break;
            }
            if let ObjectKey {
                data: ObjectKeyData::Attribute(extent_attribute_id, attribute_key),
                ..
            } = item_ref.key
            {
                if *extent_attribute_id != attribute_id {
                    result = TrimResult::Done(Some(*extent_attribute_id));
                    break;
                }
                if let (
                    AttributeKey::Extent(extent),
                    ObjectValue::Extent(ExtentValue::Some { device_offset, .. }),
                ) = (attribute_key, item_ref.value)
                {
                    let start = std::cmp::max(extent.start, aligned_offset);
                    ensure!(start < extent.end, FxfsError::Inconsistent);
                    let device_offset = device_offset
                        .checked_add(start - extent.start)
                        .ok_or(FxfsError::Inconsistent)?;
                    end = extent.end;
                    let len = end - start;
                    let device_range = device_offset..device_offset + len;
                    ensure!(block_size.is_aligned(&device_range), FxfsError::Inconsistent);
                    allocator.deallocate(transaction, self.store_object_id, device_range).await?;
                    deallocated += len;
                    // Stop if the transaction is getting too big.
                    if transaction.mutations().len() >= TRANSACTION_MUTATION_THRESHOLD {
                        result = TrimResult::Incomplete;
                        break;
                    }
                }
            }
            iter.advance().await?;
        }

        let finished_tombstone_object = matches!(mode, TrimMode::Tombstone(TombstoneMode::Object))
            && matches!(result, TrimResult::Done(None));
        let finished_tombstone_attribute =
            matches!(mode, TrimMode::Tombstone(TombstoneMode::Attribute))
                && !matches!(result, TrimResult::Incomplete);
        let mut object_mutation = None;
        let nodes = if finished_tombstone_object { -1 } else { 0 };
        if nodes != 0 || deallocated != 0 {
            let mutation = self.txn_get_object_mutation(transaction, object_id).await?;
            if let ObjectValue::Object { attributes: ObjectAttributes { project_id, .. }, .. } =
                mutation.item.value
            {
                if let Some(project_id) = project_id {
                    transaction.add(
                        self.store_object_id,
                        Mutation::merge_object(
                            ObjectKey::project_usage(self.root_directory_object_id(), project_id),
                            ObjectValue::BytesAndNodes {
                                bytes: -i64::try_from(deallocated).unwrap(),
                                nodes,
                            },
                        ),
                    );
                }
                object_mutation = Some(mutation);
            } else {
                panic!("Inconsistent object type.");
            }
        }

        // Deletion marker records *must* be merged so as to consume all other records for the
        // object.
        if finished_tombstone_object {
            transaction.add(
                self.store_object_id,
                Mutation::merge_object(ObjectKey::object(object_id), ObjectValue::None),
            );
        } else {
            if finished_tombstone_attribute {
                transaction.add(
                    self.store_object_id,
                    Mutation::merge_object(
                        ObjectKey::attribute(object_id, attribute_id, AttributeKey::Attribute),
                        ObjectValue::None,
                    ),
                );
            }
            if deallocated > 0 {
                let mut mutation = match object_mutation {
                    Some(mutation) => mutation,
                    None => self.txn_get_object_mutation(transaction, object_id).await?,
                };
                transaction.add(
                    self.store_object_id,
                    Mutation::merge_object(
                        ObjectKey::extent(object_id, attribute_id, aligned_offset..end),
                        ObjectValue::deleted_extent(),
                    ),
                );
                // Update allocated size.
                if let ObjectValue::Object {
                    attributes: ObjectAttributes { allocated_size, .. },
                    ..
                } = &mut mutation.item.value
                {
                    // The only way for these to fail are if the volume is inconsistent.
                    *allocated_size = allocated_size.checked_sub(deallocated).ok_or_else(|| {
                        anyhow!(FxfsError::Inconsistent).context("Allocated size overflow")
                    })?;
                } else {
                    panic!("Unexpected object value");
                }
                transaction.add(self.store_object_id, Mutation::ObjectStore(mutation));
            }
        }
        Ok(result)
    }

    /// Returns all objects that exist in the parent store that pertain to this object store.
    /// Note that this doesn't include the object_id of the store itself which is generally
    /// referenced externally.
    pub fn parent_objects(&self) -> Vec<u64> {
        assert!(self.store_info_handle.get().is_some());
        self.store_info.lock().as_ref().unwrap().parent_objects()
    }

    /// Returns root objects for this store.
    pub fn root_objects(&self) -> Vec<u64> {
        let mut objects = Vec::new();
        let store_info = self.store_info.lock();
        let info = store_info.as_ref().unwrap();
        if info.root_directory_object_id != INVALID_OBJECT_ID {
            objects.push(info.root_directory_object_id);
        }
        if info.graveyard_directory_object_id != INVALID_OBJECT_ID {
            objects.push(info.graveyard_directory_object_id);
        }
        if info.internal_directory_object_id != INVALID_OBJECT_ID {
            objects.push(info.internal_directory_object_id);
        }
        objects
    }

    pub fn store_info(&self) -> Option<StoreInfo> {
        self.store_info.lock().as_ref().cloned()
    }

    /// Returns None if called during journal replay.
    pub fn store_info_handle_object_id(&self) -> Option<u64> {
        self.store_info_handle.get().map(|h| h.object_id())
    }

    pub fn graveyard_count(&self) -> u64 {
        self.graveyard_entries.load(Ordering::Relaxed)
    }

    /// Called to open a store, before replay of this store's mutations.
    async fn open(
        parent_store: &Arc<ObjectStore>,
        store_object_id: u64,
        object_cache: Option<Box<dyn ObjectCache<ObjectKey, ObjectValue>>>,
    ) -> Result<Arc<ObjectStore>, Error> {
        let handle =
            ObjectStore::open_object(parent_store, store_object_id, HandleOptions::default(), None)
                .await?;

        let info = load_store_info(parent_store, store_object_id).await?;
        let is_encrypted = info.mutations_key.is_some();

        let mut total_layer_size = 0;
        let last_object_id;

        // TODO(https://fxbug.dev/42178043): the layer size here could be bad and cause overflow.

        // If the store is encrypted, we can't open the object tree layers now, but we need to
        // compute the size of the layers.
        if is_encrypted {
            for &oid in &info.layers {
                total_layer_size += parent_store.get_file_size(oid).await?;
            }
            if info.encrypted_mutations_object_id != INVALID_OBJECT_ID {
                total_layer_size += layer_size_from_encrypted_mutations_size(
                    parent_store.get_file_size(info.encrypted_mutations_object_id).await?,
                );
            }
            last_object_id = LastObjectId::Pending;
            ensure!(
                matches!(
                    info.last_object_id,
                    LastObjectIdInfo::Encrypted { .. } | LastObjectIdInfo::Low32Bit { .. }
                ),
                FxfsError::Inconsistent
            );
        } else {
            last_object_id = match info.last_object_id {
                LastObjectIdInfo::Unencrypted { id } => LastObjectId::Unencrypted { id },
                LastObjectIdInfo::Low32Bit => {
                    LastObjectId::Low32Bit { reserved: HashSet::new(), unreserved: Vec::new() }
                }
                _ => bail!(FxfsError::Inconsistent),
            };
        }

        let fs = parent_store.filesystem();

        let store = ObjectStore::new(
            Some(parent_store.clone()),
            store_object_id,
            fs.clone(),
            if is_encrypted { None } else { Some(info) },
            object_cache,
            if is_encrypted { LockState::Locked } else { LockState::Unencrypted },
            last_object_id,
        );

        assert!(store.store_info_handle.set(handle).is_ok(), "Failed to set store_info_handle!");

        if !is_encrypted {
            let object_tree_layer_object_ids =
                store.store_info.lock().as_ref().unwrap().layers.clone();
            let object_layers = store.open_layers(object_tree_layer_object_ids, None).await?;
            total_layer_size = object_layers.iter().map(|h| h.get_size()).sum();
            store
                .tree
                .append_layers(object_layers)
                .await
                .context("Failed to read object store layers")?;
        }

        fs.object_manager().update_reservation(
            store_object_id,
            tree::reservation_amount_from_layer_size(total_layer_size),
        );

        Ok(store)
    }

    async fn load_store_info(&self) -> Result<StoreInfo, Error> {
        load_store_info_from_handle(self.store_info_handle.get().unwrap()).await
    }

    async fn open_layers(
        &self,
        object_ids: impl std::iter::IntoIterator<Item = u64>,
        crypt: Option<Arc<dyn Crypt>>,
    ) -> Result<Vec<DataObjectHandle<ObjectStore>>, Error> {
        let parent_store = self.parent_store.as_ref().unwrap();
        let mut handles = Vec::new();
        for object_id in object_ids {
            let handle = ObjectStore::open_object(
                &parent_store,
                object_id,
                HandleOptions::default(),
                crypt.clone(),
            )
            .await
            .with_context(|| format!("Failed to open layer file {}", object_id))?;
            handles.push(handle);
        }
        Ok(handles)
    }

    /// Unlocks a store so that it is ready to be used.
    /// This is not thread-safe.
    pub async fn unlock(self: &Arc<Self>, crypt: Arc<dyn Crypt>) -> Result<(), Error> {
        self.unlock_inner(crypt, /*read_only=*/ false).await
    }

    /// Unlocks a store so that it is ready to be read from.
    /// The store will generally behave like it is still locked: when flushed, the store will
    /// write out its mutations into the encrypted mutations file, rather than directly updating
    /// the layer files of the object store.
    /// Re-locking the store (which *must* be done with `Self::lock_read_only` will not trigger a
    /// flush, although the store might still be flushed during other operations.
    /// This is not thread-safe.
    pub async fn unlock_read_only(self: &Arc<Self>, crypt: Arc<dyn Crypt>) -> Result<(), Error> {
        self.unlock_inner(crypt, /*read_only=*/ true).await
    }

    // This function is *not* thread-safe.  Callers must ensure mutual exclusion when unlocking.
    async fn unlock_inner(
        self: &Arc<Self>,
        crypt: Arc<dyn Crypt>,
        read_only: bool,
    ) -> Result<(), Error> {
        // To avoid blocking compactions while waiting for a stalled crypt service during store
        // unlock, we perform all crypt operations outside of the flush lock. `CryptFields` tracks
        // the `StoreInfo` fields we depend on for these crypt operations, so we can verify they
        // haven't changed once we acquire the flush lock.
        #[derive(Debug, PartialEq)]
        struct CryptFields {
            mutations_key: Option<FxfsKey>,
            mutations_cipher_offset: u64,
            last_object_id: LastObjectIdInfo,
            layers: Vec<u64>,
        }
        impl From<&StoreInfo> for CryptFields {
            fn from(info: &StoreInfo) -> Self {
                Self {
                    mutations_key: info.mutations_key.clone(),
                    mutations_cipher_offset: info.mutations_cipher_offset,
                    last_object_id: info.last_object_id.clone(),
                    layers: info.layers.clone(),
                }
            }
        }

        // Unless we are unlocking the store as read-only, the filesystem must not be read-only.
        assert!(read_only || !self.filesystem().options().read_only);
        match &*self.lock_state.lock() {
            LockState::Locked => {}
            LockState::Unencrypted => bail!(FxfsError::InvalidArgs),
            LockState::Invalid | LockState::Deleted => bail!(FxfsError::Internal),
            LockState::Unlocked { .. } | LockState::UnlockedReadOnly(..) => {
                bail!(FxfsError::AlreadyBound)
            }
            LockState::Unknown => panic!("Store was unlocked before replay"),
            LockState::Locking => panic!("Store is being locked"),
            LockState::Unlocking => panic!("Store is being unlocked"),
        }

        // --- PHASE 1: Do everything that uses the crypt service outside of the flush lock ---

        let mut last_num_flushes = self.counters.lock().num_flushes;
        let mut store_info = self.load_store_info().await?;
        let crypt_fields = CryptFields::from(&store_info);

        let mut update_store_info = async |store_info: &mut StoreInfo| -> Result<(), Error> {
            let new_num_flushes = self.counters.lock().num_flushes;
            if last_num_flushes == new_num_flushes {
                return Ok(());
            }
            last_num_flushes = new_num_flushes;
            *store_info = self.load_store_info().await?;
            if crypt_fields != CryptFields::from(&*store_info) {
                return Err(
                    anyhow!(FxfsError::Inconsistent).context("Crypt fields changed during unlock")
                );
            }
            Ok(())
        };

        // Open layers (uses crypt).
        let layers = self
            .open_layers(store_info.layers.iter().cloned(), Some(crypt.clone()))
            .await
            .context("Failed to read object tree layer file contents")?;

        // Unwrap mutations key.
        let wrapped_key =
            fxfs_crypto::WrappedKey::Fxfs(store_info.mutations_key.clone().unwrap().into());
        let unwrapped_key = crypt
            .unwrap_key(&wrapped_key, self.store_object_id)
            .await
            .context("Failed to unwrap mutations keys")?;

        // Unwrap last object ID key.
        let last_object_id_cipher = match &store_info.last_object_id {
            LastObjectIdInfo::Encrypted { id: _, key } => {
                let wrapped_key = fxfs_crypto::WrappedKey::Fxfs(key.clone().into());
                let unwrapped = crypt
                    .unwrap_key(&wrapped_key, self.store_object_id)
                    .await
                    .context("Failed to unwrap last object ID key")?;
                Some(Box::new(Ff1::new(&unwrapped)))
            }
            _ => None,
        };

        // Roll mutations key (create new key outside of lock).
        let (new_wrapped_mutations_key, new_unwrapped_mutations_key) =
            crypt.create_key(self.store_object_id, KeyPurpose::Metadata).await?;

        // Pre-cache keys outside of lock.
        let mut keys_to_cache = Vec::new();
        if !read_only && !self.filesystem().options().read_only {
            let parent_store = self.parent_store.as_ref().unwrap();
            for _ in 0..CACHED_KEYS_LIMIT {
                let raw_id = {
                    let reserved_id = parent_store
                        .maybe_get_next_object_id()
                        .expect("maybe_get_next_object_id failed on parent store");
                    reserved_id.release()
                };
                let (wrapped, unwrapped) = crypt
                    .create_key(raw_id.get(), KeyPurpose::Data)
                    .await
                    .context("Failed to pre-cache key during unlock")?;
                keys_to_cache.push((raw_id, EncryptionKey::Fxfs(wrapped), unwrapped));
            }
        }

        // --- PHASE 2: Take the flush lock to read mutations ---

        let do_not_use_crypt = crypt;

        let fs = self.filesystem();
        let guard =
            fs.lock_manager().write_lock(lock_keys![LockKey::flush(self.store_object_id())]).await;

        update_store_info(&mut store_info).await?;

        // Read mutations.
        let mut mutations = {
            if store_info.encrypted_mutations_object_id == INVALID_OBJECT_ID {
                EncryptedMutations::default()
            } else {
                let parent_store = self.parent_store.as_ref().unwrap();
                let handle = ObjectStore::open_object(
                    &parent_store,
                    store_info.encrypted_mutations_object_id,
                    HandleOptions::default(),
                    None,
                )
                .await?;
                let mut cursor = std::io::Cursor::new(
                    handle
                        .contents(MAX_ENCRYPTED_MUTATIONS_SIZE)
                        .await
                        .context(FxfsError::Inconsistent)?,
                );
                let mut mutations = EncryptedMutations::deserialize_with_version(&mut cursor)
                    .context("Failed to deserialize EncryptedMutations")?
                    .0;
                let len = cursor.get_ref().len() as u64;
                while cursor.position() < len {
                    mutations.extend(
                        &EncryptedMutations::deserialize_with_version(&mut cursor)
                            .context("Failed to deserialize EncryptedMutations")?
                            .0,
                    );
                }
                mutations
            }
        };

        // This assumes that the journal has no buffered mutations for this store (see Self::lock).
        let journaled = EncryptedMutations::from_replayed_mutations(
            self.store_object_id,
            fs.journal()
                .read_transactions_for_object(self.store_object_id)
                .await
                .context("Failed to read encrypted mutations from journal")?,
        );
        mutations.extend(&journaled);

        // Drop the lock before we do any crypt operations (decryption/unwrapping).
        std::mem::drop(guard);

        // It's safe to use the crypt service now because we're not holding the flush lock.
        let crypt = do_not_use_crypt;

        self.filesystem().hooks().on_unlock_resources_acquired();

        // --- PHASE 3: Decrypt mutations (outside of lock) ---

        let EncryptedMutations { transactions, mut data, mutations_key_roll } = mutations;

        let mut unwrapped_key_rolls = Vec::with_capacity(mutations_key_roll.len());
        for (offset, key) in mutations_key_roll {
            let unwrapped = crypt
                .unwrap_key(&fxfs_crypto::WrappedKey::Fxfs(key.into()), self.store_object_id)
                .await
                .context("Failed to unwrap mutations keys")?;
            unwrapped_key_rolls.push((offset, unwrapped));
        }

        let mut mutations_to_apply = Vec::new();

        // All the data, transactions and keyrolls will be split with the first half being in the
        // older version and the rest in the new. Find that split so that they can work separately
        // from each other. Though it is very likely that they're all in one version or the other.
        let split_idx = transactions
            .partition_point(|(checkpoint, _)| checkpoint.version < AES_JOURNAL_ENCRYPTION_VERSION);
        let (chacha_transactions, aes_transactions) = transactions.split_at(split_idx);
        let data_offset = if chacha_transactions.is_empty() {
            0
        } else {
            let aes_len = aes_transactions
                .iter()
                .try_fold(0usize, |total, (_, next)| total.checked_add(*next as usize))
                .ok_or(FxfsError::Inconsistent)?;
            ensure!(aes_len <= data.len(), FxfsError::Inconsistent);
            data.len() - aes_len
        };
        let (chacha_data, aes_data) = data.split_at_mut(data_offset);

        // Split the keyrolls based on the data offset found above, remap AES rolls based on the
        // partial data set it receives.
        let split_key_idx =
            unwrapped_key_rolls.partition_point(|(offset, _)| *offset < data_offset);
        let aes_key_rolls: Vec<(usize, UnwrappedKey)> = unwrapped_key_rolls
            .split_off(split_key_idx)
            .into_iter()
            .map(|(offset, key)| (offset - data_offset, key))
            .collect();
        let chacha_key_rolls = unwrapped_key_rolls;
        let mut cipher_sequence_number = store_info.mutations_cipher_offset;

        if !chacha_transactions.is_empty() {
            ensure!(store_info.mutations_cipher_offset <= u32::MAX as u64, FxfsError::Inconsistent);
            let mut cipher = StreamCipher::new(&unwrapped_key, cipher_sequence_number);
            mutations_to_apply.append(&mut Self::decrypt_mutations_chacha20(
                &mut cipher,
                chacha_transactions,
                chacha_data,
                &chacha_key_rolls,
            )?);
            cipher_sequence_number = cipher.offset();
        }

        if !aes_transactions.is_empty() {
            let mut cipher = JournalXtsCipher::new(&unwrapped_key, cipher_sequence_number);
            mutations_to_apply.append(&mut Self::decrypt_mutations_aes256_xts(
                &mut cipher,
                aes_transactions,
                aes_data,
                &aes_key_rolls,
            )?);
            cipher_sequence_number = cipher.current_tweak();
        }

        // --- PHASE 4: Re-acquire the flush lock and apply changes ---

        let do_not_use_crypt = crypt;

        let guard =
            fs.lock_manager().write_lock(lock_keys![LockKey::flush(self.store_object_id())]).await;

        update_store_info(&mut store_info).await?;

        let _ = std::mem::replace(&mut *self.lock_state.lock(), LockState::Unlocking);

        store_info.mutations_key = Some(new_wrapped_mutations_key);
        *self.store_info.lock() = Some(store_info.clone());

        let clean_up = scopeguard::guard((), |_| {
            *self.lock_state.lock() = LockState::Locked;
            *self.store_info.lock() = None;
            *self.last_object_id.lock() = LastObjectId::Pending;
            // Make sure we don't leave unencrypted data lying around in memory.
            self.tree.reset();
        });

        // Apply layers.
        self.tree.append_layers(layers).await.context("Failed to append layers to object tree")?;

        // Set last object ID.
        match &store_info.last_object_id {
            LastObjectIdInfo::Encrypted { id, .. } => {
                *self.last_object_id.lock() =
                    LastObjectId::Encrypted { id: *id, cipher: last_object_id_cipher.unwrap() };
            }
            LastObjectIdInfo::Low32Bit => {
                *self.last_object_id.lock() = LastObjectId::Low32Bit {
                    reserved: Default::default(),
                    unreserved: Default::default(),
                }
            }
            _ => unreachable!(),
        }

        // Apply mutations.
        for (checkpoint, mutation) in mutations_to_apply {
            let context = ApplyContext { mode: ApplyMode::Replay, checkpoint };
            self.apply_mutation(mutation, &context, AssocObj::None)
                .context("failed to apply encrypted mutation")?;
        }

        // Transition to Unlocked.
        *self.lock_state.lock() = if read_only {
            LockState::UnlockedReadOnly(do_not_use_crypt)
        } else {
            LockState::Unlocked {
                crypt: do_not_use_crypt,
                cached_keys: keys_to_cache,
                mutations_cipher: JournalCipher::new_aes256_xts(
                    &new_unwrapped_mutations_key,
                    cipher_sequence_number,
                ),
            }
        };

        // To avoid unbounded memory growth, we should flush the encrypted mutations now. Otherwise
        // it's possible for more writes to be queued and for the store to be locked before we can
        // flush anything and that can repeat.
        std::mem::drop(guard);

        if !read_only && !self.filesystem().options().read_only {
            self.flush_with_reason(FlushReason::EncryptedMutations).await?;

            // Reap purged files within this store.
            let _ = self.filesystem().graveyard().initial_reap(&self).await?;
        }

        // Return and cancel the clean up.
        Ok(ScopeGuard::into_inner(clean_up))
    }

    fn decrypt_mutations_aes256_xts(
        cipher: &mut JournalXtsCipher,
        transactions: &[(JournalCheckpoint, u64)],
        data: &mut [u8],
        key_rolls: &[(usize, UnwrappedKey)],
    ) -> Result<Vec<(JournalCheckpoint, Mutation)>, Error> {
        let mut mutations_to_apply = Vec::new();
        let mut key_roll_iter = key_rolls.iter().peekable();
        let mut offset = 0;

        for (checkpoint, chunk_len) in transactions {
            ensure!(checkpoint.version >= AES_JOURNAL_ENCRYPTION_VERSION, FxfsError::Inconsistent);
            while let Some((roll_offset, unwrapped_key)) = key_roll_iter.peek() {
                if offset >= *roll_offset {
                    key_roll_iter.next();
                    *cipher = JournalXtsCipher::new(unwrapped_key, cipher.current_tweak());
                } else {
                    break;
                }
            }
            let chunk_len = *chunk_len as usize;
            ensure!(offset + chunk_len <= data.len(), FxfsError::Inconsistent);
            let chunk = &mut data[offset..offset + chunk_len];
            let transaction =
                EncryptedTransaction::decrypt_and_deserialize(cipher, chunk, checkpoint.version)?;
            for mutation in transaction.0 {
                mutations_to_apply.push((checkpoint.clone(), mutation));
            }
            offset += chunk_len;
        }
        Ok(mutations_to_apply)
    }

    fn decrypt_mutations_chacha20(
        cipher: &mut StreamCipher,
        transactions: &[(JournalCheckpoint, u64)],
        data: &mut [u8],
        key_rolls: &[(usize, UnwrappedKey)],
    ) -> Result<Vec<(JournalCheckpoint, Mutation)>, Error> {
        let mut mutations_to_apply = Vec::new();
        let mut slice = &mut data[..];
        let mut last_offset = 0;
        for (offset, unwrapped_key) in key_rolls {
            let split_offset = offset.checked_sub(last_offset).ok_or(FxfsError::Inconsistent)?;
            ensure!(split_offset <= slice.len(), FxfsError::Inconsistent);
            last_offset = *offset;
            let (old, new) = slice.split_at_mut(split_offset);
            cipher.decrypt(old);
            *cipher = StreamCipher::new(unwrapped_key, 0);
            slice = new;
        }
        cipher.decrypt(slice);

        let mut cursor = std::io::Cursor::new(&*data);
        for (checkpoint, count) in transactions {
            for _ in 0..*count {
                let mutation = Mutation::deserialize_from_version(&mut cursor, checkpoint.version)
                    .context("failed to deserialize encrypted mutation")?;
                mutations_to_apply.push((checkpoint.clone(), mutation));
            }
        }
        Ok(mutations_to_apply)
    }

    pub fn is_locked(&self) -> bool {
        matches!(
            *self.lock_state.lock(),
            LockState::Locked | LockState::Locking | LockState::Unknown
        )
    }

    /// NB: This is not the converse of `is_locked`, as there are lock states where neither are
    /// true.
    pub fn is_unlocked(&self) -> bool {
        matches!(
            *self.lock_state.lock(),
            LockState::Unlocked { .. } | LockState::UnlockedReadOnly(..) | LockState::Unlocking
        )
    }

    pub fn is_unknown(&self) -> bool {
        matches!(*self.lock_state.lock(), LockState::Unknown)
    }

    pub fn is_encrypted(&self) -> bool {
        self.store_info.lock().as_ref().unwrap().mutations_key.is_some()
    }

    // Locks a store.
    // This operation will take a flush lock on the store, in case any flushes are ongoing.  Any
    // ongoing store accesses might be interrupted by this.  See `Self::crypt`.
    // Whilst this can return an error, the store will be placed into an unusable but safe state
    // (i.e. no lingering unencrypted data) if an error is encountered.
    pub async fn lock(&self) -> Result<(), Error> {
        // We must lock flushing since it is not safe for that to be happening whilst we are locking
        // the store.
        let keys = lock_keys![LockKey::flush(self.store_object_id())];
        let fs = self.filesystem();
        let _guard = fs.lock_manager().write_lock(keys).await;

        {
            let mut lock_state = self.lock_state.lock();
            if let LockState::Unlocked { .. } = &*lock_state {
                *lock_state = LockState::Locking;
            } else {
                panic!("Unexpected lock state: {:?}", *lock_state);
            }
        }

        // Sync the journal now to ensure that any buffered mutations for this store make it out to
        // disk.  This is necessary to be able to unlock the store again.
        // We need to establish a barrier at this point (so that the journaled writes are observable
        // by any future attempts to unlock the store), hence the flush_device.
        let sync_result =
            self.filesystem().sync(SyncOptions { flush_device: true, ..Default::default() }).await;

        *self.lock_state.lock() = if let Err(error) = &sync_result {
            error!(error:?; "Failed to sync journal; store will no longer be usable");
            LockState::Invalid
        } else {
            LockState::Locked
        };
        self.key_manager.clear();
        *self.store_info.lock() = None;
        *self.last_object_id.lock() = LastObjectId::Pending;
        self.tree.reset();

        sync_result
    }

    // Locks a store which was previously unlocked read-only (see `Self::unlock_read_only`).  Data
    // is not flushed, and instead any journaled mutations are buffered back into the ObjectStore
    // and will be replayed next time the store is unlocked.
    pub fn lock_read_only(&self) {
        *self.lock_state.lock() = LockState::Locked;
        *self.store_info.lock() = None;
        *self.last_object_id.lock() = LastObjectId::Pending;
        self.tree.reset();
    }

    // Returns None if the object ID cipher needs to be created or rolled, or a more expensive
    // algorithm needs to be used.
    fn maybe_get_next_object_id(&self) -> Option<ReservedId<'_>> {
        self.last_object_id.lock().try_get_next().map(|id| ReservedId::new(self, id))
    }

    /// Returns a new object ID that can be used.  This will create an object ID cipher if needed.
    ///
    /// If the object ID key needs to be rolled, a new transaction will be created and committed.
    pub(super) async fn get_next_object_id(&self) -> Result<ReservedId<'_>, Error> {
        {
            let mut last_object_id = self.last_object_id.lock();
            if let Some(id) = last_object_id.try_get_next() {
                return Ok(ReservedId::new(self, id));
            }
            ensure!(
                !matches!(&*last_object_id, LastObjectId::Unencrypted { .. }),
                FxfsError::Inconsistent
            );
        }

        let parent_store = self.parent_store().unwrap();

        // Create a transaction (which has a lock) and then check again.
        //
        // NOTE: Since this can be a nested transaction, we must take care to avoid deadlocks; no
        // more locks should be taken whilst we hold this lock.
        let mut transaction = parent_store
            .new_transaction(
                lock_keys![LockKey::object(parent_store.store_object_id, self.store_object_id)],
                Options {
                    // We must skip journal checks because this transaction might be needed to
                    // compact.
                    skip_journal_checks: true,
                    borrow_metadata_space: true,
                    ..Default::default()
                },
            )
            .await?;

        let mut next_id_hi = 0;

        let is_low_32_bit = {
            let mut last_object_id = self.last_object_id.lock();
            if let Some(id) = last_object_id.try_get_next() {
                // Something else raced and created/rolled the cipher.
                return Ok(ReservedId::new(self, id));
            }

            match &*last_object_id {
                LastObjectId::Encrypted { id, .. } => {
                    // It shouldn't be possible for last_object_id to wrap within our lifetime, so
                    // if this happens, it's most likely due to corruption.
                    next_id_hi =
                        id.checked_add(1 << 32).ok_or(FxfsError::Inconsistent)? & OBJECT_ID_HI_MASK;

                    info!(store_id = self.store_object_id; "Rolling object ID key");

                    false
                }
                LastObjectId::Low32Bit { .. } => true,
                _ => unreachable!(),
            }
        };

        if is_low_32_bit {
            // Keep picking an object ID at random until we find one free.

            // To avoid races, this must be before we capture the layer set.
            self.last_object_id.lock().drain_unreserved();

            let layer_set = self.tree.layer_set();
            let mut key = ObjectKey::object(0);
            loop {
                let next_id = rand::rng().next_u32() as u64;
                let Some(next_id) = NonZero::new(next_id) else { continue };
                if self.last_object_id.lock().is_reserved(next_id.get()) {
                    continue;
                }
                key.object_id = next_id.get();
                if layer_set.key_exists(&key).await? == Existence::Missing {
                    self.last_object_id.lock().reserve(next_id.get());
                    return Ok(ReservedId::new(self, next_id));
                }
            }
        } else {
            // Create a key.
            let (object_id_wrapped, object_id_unwrapped) = self
                .crypt()
                .unwrap()
                .create_key(self.store_object_id, KeyPurpose::Metadata)
                .await?;

            // Normally we would use a mutation to note the updated key, but that would complicate
            // replay.  During replay, we need to keep track of the highest used object ID and this
            // is done by watching mutations to see when we create objects, and then decrypting
            // the object ID.  This relies on the unwrapped key being available, so as soon as
            // we detect the key has changed, we would need to immediately unwrap the key via the
            // crypt service.  Currently, this isn't easy to do during replay.  An option we could
            // consider would be to include the unencrypted object ID when we create objects, which
            // would avoid us having to decrypt the object ID during replay.
            //
            // For now and for historical reasons, the approach we take is to just write a new
            // version of StoreInfo here.  We must take care that we only update the key and not any
            // other information contained within StoreInfo because other information should only be
            // updated when we flush.  We are holding the lock on the StoreInfo file, so this will
            // prevent potential races with flushing.  To make sure we only change the key, we read
            // StoreInfo from storage rather than using our in-memory copy.  This won't be
            // performant, but rolling the object ID key will be extremely rare.
            let new_store_info = StoreInfo {
                last_object_id: LastObjectIdInfo::Encrypted {
                    id: next_id_hi,
                    key: object_id_wrapped.clone(),
                },
                ..self.load_store_info().await?
            };

            self.write_store_info(&mut transaction, &new_store_info).await?;

            transaction
                .commit_with_callback(|_| {
                    self.store_info.lock().as_mut().unwrap().last_object_id =
                        new_store_info.last_object_id;
                    match &mut *self.last_object_id.lock() {
                        LastObjectId::Encrypted { id, cipher } => {
                            **cipher = Ff1::new(&object_id_unwrapped);
                            *id = next_id_hi;
                            ReservedId::new(
                                self,
                                NonZero::new(next_id_hi | cipher.encrypt(0) as u64).unwrap(),
                            )
                        }
                        _ => unreachable!(),
                    }
                })
                .await
        }
    }

    /// Query the next object ID that will be used. Intended for use when checking filesystem
    /// consistency. Prefer [`Self::get_next_object_id()`] for general use.
    pub(crate) fn query_next_object_id(&self) -> u64 {
        self.last_object_id.lock().peek_next()
    }

    fn allocator(&self) -> Arc<Allocator> {
        self.filesystem().allocator()
    }

    // If |transaction| has an impending mutation for the underlying object, returns that.
    // Otherwise, looks up the object from the tree and returns a suitable mutation for it.  The
    // mutation is returned here rather than the item because the mutation includes the operation
    // which has significance: inserting an object implies it's the first of its kind unlike
    // replacing an object.
    async fn txn_get_object_mutation(
        &self,
        transaction: &Transaction<'_>,
        object_id: u64,
    ) -> Result<ObjectStoreMutation, Error> {
        if let Some(mutation) =
            transaction.get_object_mutation(self.store_object_id, ObjectKey::object(object_id))
        {
            Ok(mutation.clone())
        } else {
            Ok(ObjectStoreMutation {
                item: self
                    .tree
                    .find(&ObjectKey::object(object_id))
                    .await?
                    .ok_or(FxfsError::Inconsistent)
                    .context("Object id missing")?,
                op: Operation::ReplaceOrInsert,
            })
        }
    }

    /// Like txn_get_object_mutation but with expanded visibility.
    /// Only available in migration code.
    #[cfg(feature = "migration")]
    pub async fn get_object_mutation(
        &self,
        transaction: &Transaction<'_>,
        object_id: u64,
    ) -> Result<ObjectStoreMutation, Error> {
        self.txn_get_object_mutation(transaction, object_id).await
    }

    fn update_last_object_id(&self, object_id: u64) {
        let mut last_object_id = self.last_object_id.lock();
        match &mut *last_object_id {
            LastObjectId::Pending => unreachable!(),
            LastObjectId::Unencrypted { id } => {
                if object_id > *id {
                    *id = object_id
                }
            }
            LastObjectId::Encrypted { id, cipher } => {
                // For encrypted stores, object_id will be encrypted here, so we must decrypt first.

                // If the object ID cipher has been rolled, then it's possible we might see object
                // IDs that were generated using a different cipher so the decrypt here will return
                // the wrong value, but that won't matter because the hi part of the object ID
                // should still discriminate.
                let object_id =
                    object_id & OBJECT_ID_HI_MASK | cipher.decrypt(object_id as u32) as u64;
                if object_id > *id {
                    *id = object_id;
                }
            }
            LastObjectId::Low32Bit { .. } => {}
        }
    }

    /// If possible, converts the given object ID to its unencrypted value.  Returns None if it is
    /// not possible to convert to its unencrypted value because the key is unavailable.
    pub fn to_unencrypted_object_id(&self, object_id: u64) -> Option<u64> {
        let last_object_id = self.last_object_id.lock();
        match &*last_object_id {
            LastObjectId::Pending => None,
            LastObjectId::Unencrypted { .. } | LastObjectId::Low32Bit { .. } => Some(object_id),
            LastObjectId::Encrypted { id, cipher } => {
                if id & OBJECT_ID_HI_MASK != object_id & OBJECT_ID_HI_MASK {
                    None
                } else {
                    Some(object_id & OBJECT_ID_HI_MASK | cipher.decrypt(object_id as u32) as u64)
                }
            }
        }
    }

    /// Adds the specified object to the graveyard.
    pub fn add_to_graveyard(&self, transaction: &mut Transaction<'_>, object_id: u64) {
        let graveyard_id = self.graveyard_directory_object_id();
        assert_ne!(graveyard_id, INVALID_OBJECT_ID);
        transaction.add(
            self.store_object_id,
            Mutation::replace_or_insert_object(
                ObjectKey::graveyard_entry(graveyard_id, object_id),
                ObjectValue::Some,
            ),
        );
    }

    /// Removes the specified object from the graveyard.  NB: Care should be taken when calling
    /// this because graveyard entries are used for purging deleted files *and* for trimming
    /// extents.  For example, consider the following sequence:
    ///
    ///     1. Add Trim graveyard entry.
    ///     2. Replace with Some graveyard entry (see above).
    ///     3. Remove graveyard entry.
    ///
    /// If the desire in #3 is just to cancel the effect of the Some entry, then #3 should
    /// actually be:
    ///
    ///     3. Replace with Trim graveyard entry.
    pub fn remove_from_graveyard(&self, transaction: &mut Transaction<'_>, object_id: u64) {
        transaction.add(
            self.store_object_id,
            Mutation::replace_or_insert_object(
                ObjectKey::graveyard_entry(self.graveyard_directory_object_id(), object_id),
                ObjectValue::None,
            ),
        );
    }

    /// Removes the specified attribute from the graveyard. Unlike object graveyard entries,
    /// attribute graveyard entries only have one functionality (i.e. to purge deleted attributes)
    /// so the caller does not need to be concerned about replacing the graveyard attribute entry
    /// with its prior state when cancelling it. See comment on `remove_from_graveyard()`.
    pub fn remove_attribute_from_graveyard(
        &self,
        transaction: &mut Transaction<'_>,
        object_id: u64,
        attribute_id: AttributeId,
    ) {
        transaction.add(
            self.store_object_id,
            Mutation::replace_or_insert_object(
                ObjectKey::graveyard_attribute_entry(
                    self.graveyard_directory_object_id(),
                    object_id,
                    attribute_id,
                ),
                ObjectValue::None,
            ),
        );
    }

    // When the symlink is unlocked, this function decrypts `link` and returns a bag of bytes that
    // is identical to that which was passed in as the target on `create_symlink`.
    // If the symlink is locked, this function hashes the encrypted `link` with Sha256 in order to
    // get a standard length and then base64 encodes the hash and returns that to the caller.
    pub async fn read_encrypted_symlink(
        &self,
        object_id: u64,
        link: Vec<u8>,
    ) -> Result<Vec<u8>, Error> {
        let mut link = link;
        let key = self
            .key_manager()
            .get_fscrypt_key(object_id, self.crypt().unwrap().as_ref(), async || {
                self.get_keys(object_id).await
            })
            .await?;
        if let Some(key) = key.into_cipher() {
            key.decrypt_symlink(object_id, &mut link)?;
            Ok(link)
        } else {
            // Locked symlinks are encoded using a hash_code of 0.
            let proxy_filename =
                fscrypt::proxy_filename::ProxyFilename::new_with_hash_code(0, &link);
            let proxy_filename_str: String = proxy_filename.into();
            Ok(proxy_filename_str.into_bytes())
        }
    }

    /// Returns the link of a symlink object.
    pub async fn read_symlink(&self, object_id: u64) -> Result<Vec<u8>, Error> {
        match self.tree.find_value(&ObjectKey::object(object_id)).await? {
            None => bail!(FxfsError::NotFound),
            Some(ObjectValue::Object {
                kind: ObjectKind::EncryptedSymlink { link, .. }, ..
            }) => self.read_encrypted_symlink(object_id, link.into_vec()).await,
            Some(ObjectValue::Object { kind: ObjectKind::Symlink { link, .. }, .. }) => {
                Ok(link.into_vec())
            }
            Some(value) => Err(anyhow!(FxfsError::Inconsistent)
                .context(format!("Unexpected value in symlink lookup: {value:?}"))),
        }
    }

    /// Fail if any keys in the root store are not basic Fxfs keys. Fscrypt keys shouldn't be
    /// there and use a weaker key derivation function. Prevent accidentally using it on a volume's
    /// root key.
    fn fail_on_illegal_keys(&self, keys: &EncryptionKeys) -> Result<(), Error> {
        if !self.is_root() {
            return Ok(());
        }

        for key in keys.iter() {
            if !matches!(key.1, EncryptionKey::Fxfs(_)) {
                return Err(
                    anyhow!(FxfsError::IntegrityError).context("Illegal key type in root store")
                );
            }
        }
        Ok(())
    }

    /// Retrieves the wrapped keys for the given object.  The keys *should* be known to exist and it
    /// will be considered an inconsistency if they don't.
    pub async fn get_keys(&self, object_id: u64) -> Result<EncryptionKeys, Error> {
        self.tree
            .find_map(&ObjectKey::keys(object_id), |item| match item.value {
                ObjectValue::Keys(keys) => {
                    self.fail_on_illegal_keys(keys)?;
                    Ok(keys.clone())
                }
                _ => Err(anyhow!(FxfsError::Inconsistent).context("open_object: Expected keys")),
            })
            .await?
            .ok_or(FxfsError::Inconsistent)?
    }

    pub async fn update_attributes<'a>(
        &self,
        transaction: &mut Transaction<'a>,
        object_id: u64,
        node_attributes: Option<&fio::MutableNodeAttributes>,
        change_time: Option<Timestamp>,
    ) -> Result<(), Error> {
        if change_time.is_none() {
            if let Some(attributes) = node_attributes {
                let empty_attributes = fio::MutableNodeAttributes { ..Default::default() };
                if *attributes == empty_attributes {
                    return Ok(());
                }
            } else {
                return Ok(());
            }
        }
        let mut mutation = self.txn_get_object_mutation(transaction, object_id).await?;
        if let ObjectValue::Object { ref mut attributes, .. } = mutation.item.value {
            if let Some(time) = change_time {
                attributes.change_time = time;
            }
            if let Some(node_attributes) = node_attributes {
                if let Some(time) = node_attributes.creation_time {
                    attributes.creation_time = Timestamp::from_nanos(time);
                }
                if let Some(time) = node_attributes.modification_time {
                    attributes.modification_time = Timestamp::from_nanos(time);
                }
                if let Some(time) = node_attributes.access_time {
                    attributes.access_time = Timestamp::from_nanos(time);
                }
                if node_attributes.mode.is_some()
                    || node_attributes.uid.is_some()
                    || node_attributes.gid.is_some()
                    || node_attributes.rdev.is_some()
                {
                    if let Some(a) = &mut attributes.posix_attributes {
                        if let Some(mode) = node_attributes.mode {
                            a.mode = mode;
                        }
                        if let Some(uid) = node_attributes.uid {
                            a.uid = uid;
                        }
                        if let Some(gid) = node_attributes.gid {
                            a.gid = gid;
                        }
                        if let Some(rdev) = node_attributes.rdev {
                            a.rdev = rdev;
                        }
                    } else {
                        attributes.posix_attributes = Some(PosixAttributes {
                            mode: node_attributes.mode.unwrap_or_default(),
                            uid: node_attributes.uid.unwrap_or_default(),
                            gid: node_attributes.gid.unwrap_or_default(),
                            rdev: node_attributes.rdev.unwrap_or_default(),
                        });
                    }
                }
            }
        } else {
            bail!(
                anyhow!(FxfsError::Inconsistent)
                    .context("ObjectStore.update_attributes: Expected object value")
            );
        };
        transaction.add(self.store_object_id(), Mutation::ObjectStore(mutation));
        Ok(())
    }

    // Updates and commits the changes to access time in ObjectProperties. The update matches
    // Linux's RELATIME. That is, access time is updated to the current time if access time is less
    // than or equal to the last modification or status change, or if it has been more than a day
    // since the last access.  `precondition` is a condition to be checked *after* taking the lock
    // on the object.  If `precondition` returns false, no update will be performed.
    pub async fn update_access_time(
        &self,
        object_id: u64,
        props: &mut ObjectProperties,
        precondition: impl FnOnce() -> bool,
    ) -> Result<(), Error> {
        let access_time = props.access_time.as_nanos();
        let modification_time = props.modification_time.as_nanos();
        let change_time = props.change_time.as_nanos();
        let now = Timestamp::now();
        if access_time <= modification_time
            || access_time <= change_time
            || access_time
                < now.as_nanos()
                    - Timestamp::from(std::time::Duration::from_secs(24 * 60 * 60)).as_nanos()
        {
            let mut transaction = self
                .new_transaction(
                    lock_keys![LockKey::object(self.store_object_id, object_id,)],
                    Options { borrow_metadata_space: true, ..Default::default() },
                )
                .await?;
            if precondition() {
                self.update_attributes(
                    &mut transaction,
                    object_id,
                    Some(&fio::MutableNodeAttributes {
                        access_time: Some(now.as_nanos()),
                        ..Default::default()
                    }),
                    None,
                )
                .await?;
                transaction.commit().await?;
                props.access_time = now;
            }
        }
        Ok(())
    }

    async fn write_store_info<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        info: &StoreInfo,
    ) -> Result<(), Error> {
        let mut serialized_info = Vec::new();
        info.serialize_with_version(&mut serialized_info)?;
        let mut buf = self.device.allocate_buffer(serialized_info.len()).await;
        buf.copy_from_slice(&serialized_info);
        self.store_info_handle.get().unwrap().txn_write(transaction, 0u64, buf.as_ref()).await
    }

    pub fn mark_deleted(&self) {
        *self.lock_state.lock() = LockState::Deleted;
    }

    #[cfg(test)]
    pub(crate) fn test_set_last_object_id(&self, object_id: u64) {
        match &mut *self.last_object_id.lock() {
            LastObjectId::Encrypted { id, .. } => *id = object_id,
            _ => unreachable!(),
        }
    }

    /// Looks up the size of the attribute. Returns an error if either the object or attribute
    /// doesn't exist.
    pub async fn get_attribute_size(
        &self,
        object_id: u64,
        attribute_id: AttributeId,
    ) -> Result<u64, Error> {
        let size = self
            .tree
            .find_map(
                &ObjectKey::attribute(object_id, attribute_id, AttributeKey::Attribute),
                |item| match item.value {
                    ObjectValue::Attribute { size, .. }
                    | ObjectValue::VerifiedAttribute { size, .. } => Ok(*size),
                    _ => Err(anyhow!(FxfsError::Inconsistent)),
                },
            )
            .await?
            .ok_or(FxfsError::NotFound)??;
        Ok(size)
    }
}

#[async_trait]
impl JournalingObject for ObjectStore {
    fn apply_mutation(
        &self,
        mutation: Mutation,
        context: &ApplyContext<'_, '_>,
        _assoc_obj: AssocObj<'_>,
    ) -> Result<(), Error> {
        match &*self.lock_state.lock() {
            LockState::Locked | LockState::Locking => {
                ensure!(
                    matches!(mutation, Mutation::BeginFlush | Mutation::EndFlush)
                        || matches!(
                            mutation,
                            Mutation::EncryptedObjectStore(_) | Mutation::UpdateMutationsKey(_)
                                if context.mode.is_replay()
                        ),
                    anyhow!(FxfsError::Inconsistent)
                        .context(format!("Unexpected mutation for encrypted store: {mutation:?}"))
                );
            }
            LockState::Invalid
            | LockState::Unlocking
            | LockState::Unencrypted
            | LockState::Unlocked { .. }
            | LockState::UnlockedReadOnly(..)
            | LockState::Deleted => {}
            lock_state @ _ => panic!("Unexpected lock state: {lock_state:?}"),
        }
        match mutation {
            Mutation::ObjectStore(ObjectStoreMutation { item, op }) => {
                match op {
                    Operation::Insert => {
                        let mut unreserve_id = INVALID_OBJECT_ID;
                        // If we are inserting an object record for the first time, it signifies the
                        // birth of the object so we need to adjust the object count.
                        if matches!(item.value, ObjectValue::Object { .. }) {
                            {
                                let info = &mut self.store_info.lock();
                                let object_count = &mut info.as_mut().unwrap().object_count;
                                *object_count = object_count.saturating_add(1);
                            }
                            if context.mode.is_replay() {
                                self.update_last_object_id(item.key.object_id);
                            } else {
                                unreserve_id = item.key.object_id;
                            }
                        } else if !context.mode.is_replay()
                            && matches!(
                                item.key.data,
                                ObjectKeyData::GraveyardEntry { .. }
                                    | ObjectKeyData::GraveyardAttributeEntry { .. }
                            )
                        {
                            if matches!(item.value, ObjectValue::Some | ObjectValue::Trim) {
                                self.graveyard_entries.fetch_add(1, Ordering::Relaxed);
                            } else if matches!(item.value, ObjectValue::None) {
                                self.graveyard_entries.fetch_sub(1, Ordering::Relaxed);
                            }
                        }
                        self.tree.insert(item)?;
                        if unreserve_id != INVALID_OBJECT_ID {
                            // To avoid races, this *must* be after the `tree.insert(..)` above.
                            self.last_object_id.lock().unreserve(unreserve_id);
                        }
                    }
                    Operation::ReplaceOrInsert => {
                        if !context.mode.is_replay()
                            && matches!(
                                item.key.data,
                                ObjectKeyData::GraveyardEntry { .. }
                                    | ObjectKeyData::GraveyardAttributeEntry { .. }
                            )
                        {
                            if matches!(item.value, ObjectValue::Some | ObjectValue::Trim) {
                                self.graveyard_entries.fetch_add(1, Ordering::Relaxed);
                            } else if matches!(item.value, ObjectValue::None) {
                                self.graveyard_entries.fetch_sub(1, Ordering::Relaxed);
                            }
                        }
                        self.tree.replace_or_insert(item);
                    }
                    Operation::Merge => {
                        if item.is_tombstone() {
                            let info = &mut self.store_info.lock();
                            let object_count = &mut info.as_mut().unwrap().object_count;
                            *object_count = object_count.saturating_sub(1);
                            if !context.mode.is_replay() {
                                // Evict the object's keys from `key_manager` when applying the
                                // tombstone mutation at commit time (rather than when building the
                                // transaction). This ensures keys are cleaned up for both
                                // single-transaction purges and graveyard tombstones, while
                                // ensuring uncommitted transactions that get rolled back have no
                                // side effects on `key_manager`.
                                let _ = self.key_manager.remove(item.key.object_id);
                            }
                        }
                        if !context.mode.is_replay()
                            && matches!(
                                item.key.data,
                                ObjectKeyData::GraveyardEntry { .. }
                                    | ObjectKeyData::GraveyardAttributeEntry { .. }
                            )
                        {
                            if matches!(item.value, ObjectValue::Some | ObjectValue::Trim) {
                                self.graveyard_entries.fetch_add(1, Ordering::Relaxed);
                            } else if matches!(item.value, ObjectValue::None) {
                                self.graveyard_entries.fetch_sub(1, Ordering::Relaxed);
                            }
                        }
                        let lower_bound = item.key.key_for_merge_into();
                        self.tree.merge_into(item, &lower_bound);
                    }
                }
            }
            Mutation::BeginFlush => {
                ensure!(self.parent_store.is_some(), FxfsError::Inconsistent);
                self.tree.seal();
            }
            Mutation::EndFlush => ensure!(self.parent_store.is_some(), FxfsError::Inconsistent),
            Mutation::EncryptedObjectStore(_) | Mutation::UpdateMutationsKey(_) => {
                // We will process these during Self::unlock.
                ensure!(
                    !matches!(&*self.lock_state.lock(), LockState::Unencrypted),
                    FxfsError::Inconsistent
                );
            }
            Mutation::CreateInternalDir(object_id) => {
                ensure!(object_id != INVALID_OBJECT_ID, FxfsError::Inconsistent);
                self.store_info.lock().as_mut().unwrap().internal_directory_object_id = object_id;
            }
            _ => bail!("unexpected mutation: {:?}", mutation),
        }
        self.counters.lock().mutations_applied += 1;
        Ok(())
    }

    fn drop_mutation(&self, mutation: Mutation, _transaction: &Transaction<'_>) {
        self.counters.lock().mutations_dropped += 1;
        if let Mutation::ObjectStore(ObjectStoreMutation {
            item: Item { key: ObjectKey { object_id, .. }, value: ObjectValue::Object { .. }, .. },
            op: Operation::Insert,
        }) = mutation
        {
            self.last_object_id.lock().unreserve(object_id);
        }
    }

    async fn prepare_commit<'a>(
        &self,
        filesystem: &'a FxFilesystem,
        _transaction: &Transaction<'_>,
    ) -> Result<Option<WriteGuard<'a>>, Error> {
        // Short circuit check to see if this is an encrypted store.
        if !matches!(&*self.lock_state.lock(), LockState::Unlocked { .. }) {
            return Ok(None);
        }

        // We must acquire the keys lock before we can access or modify `cached_keys`.  This guard
        // is returned and held until the transaction commits, ensuring that the keys we cache (or
        // existing keys) remain valid and are not interfered with by other transactions.
        let keys = lock_keys![LockKey::pre_cache_keys(self.store_object_id())];
        let guard = filesystem.lock_manager().write_lock(keys).await;

        self.pre_cache_keys().await?;

        Ok(Some(guard))
    }

    /// Push all in-memory structures to the device. This is not necessary for sync since the
    /// journal will take care of it.  This is supposed to be called when there is either memory or
    /// space pressure (flushing the store will persist in-memory data and allow the journal file to
    /// be trimmed).
    ///
    /// Also returns the earliest version of a struct in the filesystem (when known).
    async fn flush(&self, reason: FlushReason) -> Result<Version, Error> {
        self.flush_with_reason(reason).await
    }

    fn write_mutations(
        &self,
        mutations: ObjectMutationIterator<'_, '_>,
        mut writer: journal::Writer<'_>,
    ) {
        let mut lock_state = self.lock_state.lock();
        if let LockState::Unlocked { mutations_cipher, .. } = &mut *lock_state {
            let mut encrypted_transaction = EncryptedTransaction::new();
            for mutation in mutations.cloned() {
                // Intentionally enumerating all variants to force a decision on any new variants.
                // Encrypt all mutations that could affect an encrypted object store contents or
                // the `StoreInfo` of the encrypted object store. During `unlock()` any mutations
                // which haven't been encrypted won't be replayed after reading `StoreInfo`.
                match mutation {
                    // Whilst CreateInternalDir is a mutation for `StoreInfo`, which isn't
                    // encrypted, we still choose to encrypt the mutation because it makes it
                    // easier to deal with replay. When we replay mutations for an encrypted store,
                    // the only thing we keep in memory are the encrypted mutations; we don't keep
                    // `StoreInfo` or changes to it in memory. So, by encrypting the
                    // CreateInternalDir mutation here, it means we don't have to track both
                    // encrypted mutations bound for the LSM tree and unencrypted mutations for
                    // `StoreInfo` to use in `unlock()`. It'll just bundle CreateInternalDir
                    // mutations with the other encrypted mutations and handled them all in
                    // sequence during `unlock()`.
                    Mutation::ObjectStore(_) | Mutation::CreateInternalDir(_) => {
                        encrypted_transaction.0.push(mutation)
                    }
                    // `EncryptedObjectStore` and `UpdateMutationsKey` are both obviously
                    // associated with encrypted object stores, but are either the encrypted
                    // mutation data itself or metadata governing how the data will be encrypted.
                    // They should only be produced here.
                    Mutation::EncryptedObjectStore(_) | Mutation::UpdateMutationsKey(_) => {
                        debug_assert!(
                            false,
                            "Only this method should generate encrypted mutations"
                        );
                    }
                    // `BeginFlush` and `EndFlush` are not needed during `unlock()` and are needed
                    // during the initial journal replay, so should not be encrypted. `Allocator`,
                    // `DeleteVolume`, `UpdateBorrowed` mutations are never associated with an
                    // encrypted store as we do not encrypt the allocator or root/root-parent
                    // stores so we can avoid the locking.
                    Mutation::Allocator(_)
                    | Mutation::BeginFlush
                    | Mutation::EndFlush
                    | Mutation::DeleteVolume
                    | Mutation::UpdateBorrowed(_) => {
                        writer.write(mutation);
                    }
                }
            }
            if !encrypted_transaction.0.is_empty() {
                // If this is the first time we've used this key, we must write the key out.
                if mutations_cipher.key_is_new() {
                    writer.write(Mutation::update_mutations_key(
                        self.store_info
                            .lock()
                            .as_ref()
                            .unwrap()
                            .mutations_key
                            .as_ref()
                            .unwrap()
                            .clone(),
                    ));
                }
                for encrypted in encrypted_transaction.serialize_and_encrypt(mutations_cipher) {
                    writer.write(Mutation::EncryptedObjectStore(encrypted));
                }
            }
        } else {
            for mutation in mutations {
                writer.write(mutation.clone());
            }
        }
    }
}

/// The plaintext serialization of the mutations for a transaction that are encrypted then wrapped
/// in `Mutation::EncryptedObjectStore`.
pub type EncryptedTransaction = EncryptedTransactionV57;

static_assertions::const_assert!(EncryptedTransaction::MAX_CHUNK_SIZE % 16 == 0);
impl EncryptedTransaction {
    /// The maximum size of encrypted mutation data chunked into a single
    /// `Mutation::EncryptedObjectStore`. This must be a multiple of 16 bytes (AES block size) and
    /// must fit within `DEFAULT_MAX_SERIALIZED_RECORD_SIZE` after accounting for `JournalRecord`
    /// and `Mutation` serialization overhead.
    const MAX_CHUNK_SIZE: usize = DEFAULT_MAX_SERIALIZED_RECORD_SIZE as usize - 48;

    fn new() -> Self {
        Self(Vec::new())
    }

    fn serialize_and_encrypt<'a>(
        &self,
        cipher: &'a mut JournalCipher,
    ) -> impl Iterator<Item = Box<[u8]>> + 'a {
        let mut buffer = Vec::new();
        self.serialize_into(&mut buffer).unwrap();
        // Need to limit the size of the mutations. Cut them into chunks.
        (0..buffer.len()).step_by(Self::MAX_CHUNK_SIZE).map(move |offset| {
            let end = std::cmp::min(offset + Self::MAX_CHUNK_SIZE, buffer.len());
            Box::from(cipher.encrypt(&buffer[offset..end]))
        })
    }

    fn decrypt_and_deserialize(
        cipher: &mut JournalXtsCipher,
        data: &mut [u8],
        version: Version,
    ) -> Result<Self, Error> {
        // Limited by maximum mutation size. So need to rebuild the transaction.
        let (sub_chunks, remainder) = data.as_chunks_mut::<{ Self::MAX_CHUNK_SIZE }>();
        for chunk in sub_chunks {
            cipher.decrypt(chunk.as_mut_slice());
        }
        if remainder.len() > 0 {
            cipher.decrypt(remainder);
        }
        let mut chunk_cursor = std::io::Cursor::new(&*data);
        Self::deserialize_from_version(&mut chunk_cursor, version)
            .context("failed to deserialize encrypted transaction")
    }
}

// When this type is incremented, it must increment `Mutation` since this data will actually be
// wrapped in Mutation and the version associated with it will come from some parent type above
// Mutation.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct EncryptedTransactionV57(pub Vec<Mutation>);

impl TypeFingerprint for EncryptedTransactionV57 {
    fn fingerprint() -> String {
        format!(
            "struct{{MAX_CHUNK_SIZE: {}, {}}}",
            // The MAX_CHUNK_SIZE is an important part of the format.
            Self::MAX_CHUNK_SIZE,
            Vec::<Mutation>::fingerprint()
        )
    }
}

impl Versioned for EncryptedTransactionV57 {
    // This can serialize much larger sizes. They will be broken up before being wrapped in
    // EncryptedObjectStore.
    fn max_serialized_size() -> Option<u64> {
        None
    }
}

impl Drop for ObjectStore {
    fn drop(&mut self) {
        let mut last_object_id = self.last_object_id.lock();
        last_object_id.drain_unreserved();
        match &*last_object_id {
            LastObjectId::Low32Bit { reserved, .. } => debug_assert!(reserved.is_empty()),
            _ => {}
        }
    }
}

impl HandleOwner for ObjectStore {}

impl AsRef<ObjectStore> for ObjectStore {
    fn as_ref(&self) -> &ObjectStore {
        self
    }
}

fn layer_size_from_encrypted_mutations_size(size: u64) -> u64 {
    // This is similar to reserved_space_from_journal_usage. It needs to be a worst case estimate of
    // the amount of metadata space that might need to be reserved to allow the encrypted mutations
    // to be written to layer files.  It needs to be >= than reservation_amount_from_layer_size will
    // return once the data has been written to layer files and <= than
    // reserved_space_from_journal_usage would use.  We can't just use
    // reserved_space_from_journal_usage because the encrypted mutations file includes some extra
    // data (it includes the checkpoints) that isn't written in the same way to the journal.
    size * 3
}

impl AssociatedObject for ObjectStore {}

/// Argument to the trim_some method.
#[derive(Debug)]
pub enum TrimMode {
    /// Trim extents beyond the current size.
    UseSize,

    /// Trim extents beyond the supplied offset.
    FromOffset(u64),

    /// Remove the object (or attribute) from the store once it is fully trimmed.
    Tombstone(TombstoneMode),
}

/// Sets the mode for tombstoning (either at the object or attribute level).
#[derive(Debug)]
pub enum TombstoneMode {
    Object,
    Attribute,
}

/// Result of the trim_some method.
#[derive(Debug)]
pub enum TrimResult {
    /// We reached the limit of the transaction and more extents might follow.
    Incomplete,

    /// We finished this attribute.  Returns the ID of the next attribute for the same object if
    /// there is one.
    Done(Option<AttributeId>),
}

/// Loads store info.
pub async fn load_store_info(
    parent: &Arc<ObjectStore>,
    store_object_id: u64,
) -> Result<StoreInfo, Error> {
    load_store_info_from_handle(
        &ObjectStore::open_object(parent, store_object_id, HandleOptions::default(), None).await?,
    )
    .await
}

async fn load_store_info_from_handle(
    handle: &DataObjectHandle<impl HandleOwner>,
) -> Result<StoreInfo, Error> {
    Ok(if handle.get_size() > 0 {
        let serialized_info = handle.contents(MAX_STORE_INFO_SERIALIZED_SIZE).await?;
        let mut cursor = std::io::Cursor::new(serialized_info);
        let (store_info, _) = StoreInfo::deserialize_with_version(&mut cursor)
            .context("Failed to deserialize StoreInfo")?;
        store_info
    } else {
        // The store_info will be absent for a newly created and empty object store.
        StoreInfo::default()
    })
}

#[cfg(test)]
mod tests {
    use super::{
        AttributeId, DirectWriter, EncryptedMutations, FsverityMetadata, HandleOptions,
        LastObjectId, LastObjectIdInfo, LockKey, LockState, MAX_STORE_INFO_SERIALIZED_SIZE,
        Mutation, NewChildStoreOptions, OBJECT_ID_HI_MASK, ObjectEncryptionOptions, ObjectStore,
        RootDigest, StoreInfo, StoreOptions,
    };
    use crate::errors::FxfsError;
    use crate::filesystem::{
        FlushReason, ForceMajor, FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem,
    };
    use crate::fsck::{fsck, fsck_volume};
    use crate::hooks::{Hooks, HooksHandle};
    use crate::lsm_tree::Query;
    use crate::lsm_tree::types::{ItemRef, LayerIterator};
    use crate::object_handle::{INVALID_OBJECT_ID, ObjectHandle, WriteBytes, WriteObjectHandle};
    use crate::object_store::directory::{Directory, replace_child};
    use crate::object_store::journal::{JournalCheckpoint, JournalOptions};
    use crate::object_store::object_record::{
        AttributeKey, ObjectDescriptor, ObjectKey, ObjectKind, ObjectValue,
    };
    use crate::object_store::store_object_handle::MAX_INLINE_XATTR_SIZE;
    use crate::object_store::transaction::{Options, lock_keys};
    use crate::object_store::volume::root_volume;
    use crate::serialized_types::{
        DEFAULT_MAX_SERIALIZED_RECORD_SIZE, Version, Versioned, VersionedLatest,
    };
    use crate::testing;
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use fuchsia_async as fasync;
    use fuchsia_sync::Mutex;
    use futures::channel::oneshot;
    use futures::{FutureExt, join};
    use fxfs_crypto::ff1::Ff1;
    use fxfs_crypto::{
        Crypt, EncryptionKey, FXFS_KEY_SIZE, FXFS_WRAPPED_KEY_SIZE, FxfsKey, JournalCipher,
        KeyPurpose, ObjectType, StreamCipher, UnwrappedKey, WrappedKey, WrappedKeyBytes,
        WrappingKeyId,
    };
    use fxfs_insecure_crypto::new_insecure_crypt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::time::Duration;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;
    use test_case::test_case;
    use zx_status as zx;

    const TEST_DEVICE_BLOCK_SIZE: u32 = 512;

    async fn test_filesystem_with_hooks(hooks: Arc<HooksHandle>) -> OpenFxFilesystem {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        FxFilesystemBuilder::new()
            .hooks(hooks)
            .format(true)
            .open(device)
            .await
            .expect("open failed")
    }

    async fn test_filesystem() -> OpenFxFilesystem {
        test_filesystem_with_hooks(Default::default()).await
    }

    #[fuchsia::test]
    async fn test_verified_file_with_verified_attribute() {
        let fs: OpenFxFilesystem = test_filesystem().await;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let store = fs.root_store();
        let object = Arc::new(
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed"),
        );

        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(
                    object.object_id(),
                    AttributeId::DATA,
                    AttributeKey::Attribute,
                ),
                ObjectValue::verified_attribute(
                    0,
                    FsverityMetadata::Internal(RootDigest::Sha256([0; 32]), vec![]),
                ),
            ),
        );

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

        transaction.commit().await.unwrap();

        let handle =
            ObjectStore::open_object(&store, object.object_id(), HandleOptions::default(), None)
                .await
                .expect("open_object failed");

        assert!(handle.is_verified_file());

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_verified_file_without_verified_attribute() {
        let fs: OpenFxFilesystem = test_filesystem().await;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let store = fs.root_store();
        let object = Arc::new(
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed"),
        );

        transaction.commit().await.unwrap();

        let handle =
            ObjectStore::open_object(&store, object.object_id(), HandleOptions::default(), None)
                .await
                .expect("open_object failed");

        assert!(!handle.is_verified_file());

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_create_and_open_store() {
        let fs = test_filesystem().await;
        let store_id = {
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            root_volume
                .new_volume(
                    "test",
                    NewChildStoreOptions {
                        options: StoreOptions { crypt: Some(Arc::new(new_insecure_crypt())) },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed")
                .store_object_id()
        };

        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");

        {
            let store = fs.object_manager().store(store_id).expect("store not found");
            store.unlock(Arc::new(new_insecure_crypt())).await.expect("unlock failed");
        }
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_create_and_open_internal_dir() {
        let fs = test_filesystem().await;
        let dir_id;
        let store_id;
        {
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume(
                    "test",
                    NewChildStoreOptions {
                        options: StoreOptions { crypt: Some(Arc::new(new_insecure_crypt())) },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");
            dir_id =
                store.get_or_create_internal_directory_id().await.expect("Create internal dir");
            store_id = store.store_object_id();
        }

        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");

        {
            let store = fs.object_manager().store(store_id).expect("store not found");
            store.unlock(Arc::new(new_insecure_crypt())).await.expect("unlock failed");
            assert_eq!(
                dir_id,
                store.get_or_create_internal_directory_id().await.expect("Retrieving dir")
            );
            let obj = store
                .tree()
                .find_value(&ObjectKey::object(dir_id))
                .await
                .expect("Searching tree for dir")
                .unwrap();
            assert_matches!(obj, ObjectValue::Object { kind: ObjectKind::Directory { .. }, .. });
        }
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_create_and_open_internal_dir_unencrypted() {
        let fs = test_filesystem().await;
        let dir_id;
        let store_id;
        {
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume("test", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");
            dir_id =
                store.get_or_create_internal_directory_id().await.expect("Create internal dir");
            store_id = store.store_object_id();
        }

        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");

        {
            let store = fs.object_manager().store(store_id).expect("store not found");
            assert_eq!(
                dir_id,
                store.get_or_create_internal_directory_id().await.expect("Retrieving dir")
            );
            let obj = store
                .tree()
                .find_value(&ObjectKey::object(dir_id))
                .await
                .expect("Searching tree for dir")
                .unwrap();
            assert_matches!(obj, ObjectValue::Object { kind: ObjectKind::Directory { .. }, .. });
        }
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_old_layers_are_purged() {
        let fs = test_filesystem().await;

        let store = fs.root_store();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let object = Arc::new(
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed"),
        );
        transaction.commit().await.expect("commit failed");

        store.flush().await.expect("flush failed");

        let mut buf = object.allocate_buffer(5).await;
        buf.copy_from_slice(b"hello");
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

        // Getting the layer-set should cause the flush to stall.
        let layer_set = store.tree().layer_set();

        let done = Mutex::new(false);
        let mut object_id = 0;

        join!(
            async {
                store.flush().await.expect("flush failed");
                assert!(*done.lock());
            },
            async {
                // This is a halting problem so all we can do is sleep.
                fasync::Timer::new(Duration::from_secs(1)).await;
                *done.lock() = true;
                object_id = layer_set.layers.last().unwrap().handle().unwrap().object_id();
                std::mem::drop(layer_set);
            }
        );

        if let Err(e) = ObjectStore::open_object(
            &store.parent_store.as_ref().unwrap(),
            object_id,
            HandleOptions::default(),
            store.crypt(),
        )
        .await
        {
            assert!(FxfsError::NotFound.matches(&e));
        } else {
            panic!("open_object succeeded");
        }
    }

    #[fuchsia::test]
    async fn test_tombstone_deletes_data() {
        let fs = test_filesystem().await;
        let root_store = fs.root_store();
        let child_id = {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let child = ObjectStore::create_object(
                &root_store,
                &mut transaction,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("create_object failed");
            root_store.add_to_graveyard(&mut transaction, child.object_id());
            transaction.commit().await.expect("commit failed");

            // Allocate an extent in the file.
            let mut buffer = child.allocate_buffer(8192).await;
            buffer.fill(0xaa);
            child.write_or_append(Some(0), buffer.as_ref()).await.expect("write failed");

            child.object_id()
        };

        root_store
            .tombstone_object(child_id, Options::default(), None)
            .await
            .expect("tombstone failed");

        // Let fsck check allocations.
        fsck(fs.clone()).await.expect("fsck failed");
    }

    #[fuchsia::test]
    async fn test_tombstone_purges_keys() {
        let fs = test_filesystem().await;
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_volume
            .new_volume(
                "test",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(Arc::new(new_insecure_crypt())),
                        ..StoreOptions::default()
                    },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .expect("new_volume failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let child =
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed");
        store.add_to_graveyard(&mut transaction, child.object_id());
        transaction.commit().await.expect("commit failed");
        assert!(store.key_manager.get(child.object_id()).await.unwrap().is_some());
        store
            .tombstone_object(child.object_id(), Options::default(), None)
            .await
            .expect("tombstone_object failed");
        assert!(store.key_manager.get(child.object_id()).await.unwrap().is_none());
        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn test_major_compaction_discards_unnecessary_records() {
        let fs = test_filesystem().await;
        let root_store = fs.root_store();
        let child_id = {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let child = ObjectStore::create_object(
                &root_store,
                &mut transaction,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("create_object failed");
            root_store.add_to_graveyard(&mut transaction, child.object_id());
            transaction.commit().await.expect("commit failed");

            // Allocate an extent in the file.
            let mut buffer = child.allocate_buffer(8192).await;
            buffer.fill(0xaa);
            child.write_or_append(Some(0), buffer.as_ref()).await.expect("write failed");

            child.object_id()
        };

        root_store
            .tombstone_object(child_id, Options::default(), None)
            .await
            .expect("tombstone failed");
        {
            let layers = root_store.tree.layer_set();
            let mut merger = layers.merger();
            let iter = merger
                .query(Query::FullRange(&ObjectKey::object(child_id)))
                .await
                .expect("seek failed");
            // Find at least one object still in the tree.
            match iter.get() {
                Some(ItemRef { key: ObjectKey { object_id, .. }, .. })
                    if *object_id == child_id => {}
                _ => panic!("Objects should still be in the tree."),
            }
        }
        root_store.flush().await.expect("flush failed");

        // There should be no records for the object.
        let layers = root_store.tree.layer_set();
        let mut merger = layers.merger();
        let iter = merger
            .query(Query::FullRange(&ObjectKey::object(child_id)))
            .await
            .expect("seek failed");
        match iter.get() {
            None => {}
            Some(ItemRef { key: ObjectKey { object_id, .. }, .. }) => {
                assert_ne!(*object_id, child_id)
            }
        }
    }

    #[fuchsia::test]
    async fn test_overlapping_extents_in_different_layers() {
        let fs = test_filesystem().await;
        let store = fs.root_store();

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let object = root_directory
            .create_child_file(&mut transaction, "test")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        let buf = object.allocate_buffer(16384).await;
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

        store.flush().await.expect("flush failed");

        object.write_or_append(Some(0), buf.subslice(0..4096)).await.expect("write failed");

        // At this point, we should have an extent for 0..16384 in a layer that has been flushed,
        // and an extent for 0..4096 that partially overwrites it.  Writing to 0..16384 should
        // overwrite both of those extents.
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

        fsck(fs.clone()).await.expect("fsck failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_encrypted_mutations() {
        async fn one_iteration(
            fs: OpenFxFilesystem,
            crypt: Arc<dyn Crypt>,
            iteration: u64,
        ) -> OpenFxFilesystem {
            async fn reopen(fs: OpenFxFilesystem) -> OpenFxFilesystem {
                fs.close().await.expect("Close failed");
                let device = fs.take_device().await;
                device.reopen(false);
                FxFilesystem::open(device).await.expect("FS open failed")
            }

            let fs = reopen(fs).await;

            let (store_object_id, object_id) = {
                let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
                let store = root_volume
                    .volume(
                        "test",
                        StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    )
                    .await
                    .expect("volume failed");

                let mut transaction = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object(
                            store.store_object_id(),
                            store.root_directory_object_id(),
                        )],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                let root_directory = Directory::open(&store, store.root_directory_object_id())
                    .await
                    .expect("open failed");
                let object = root_directory
                    .create_child_file(&mut transaction, &format!("test {}", iteration))
                    .await
                    .expect("create_child_file failed");
                transaction.commit().await.expect("commit failed");

                let mut buf = object.allocate_buffer(1000).await;
                for (i, byte) in buf.as_mut_ptr_slice().iter_as_mut::<u8>().enumerate() {
                    byte.write(i as u8);
                }
                object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

                (store.store_object_id(), object.object_id())
            };

            let fs = reopen(fs).await;

            let check_object = |fs: Arc<FxFilesystem>| {
                let crypt = crypt.clone();
                async move {
                    let root_volume = root_volume(fs).await.expect("root_volume failed");
                    let volume = root_volume
                        .volume(
                            "test",
                            StoreOptions { crypt: Some(crypt), ..StoreOptions::default() },
                        )
                        .await
                        .expect("volume failed");

                    let object = ObjectStore::open_object(
                        &volume,
                        object_id,
                        HandleOptions::default(),
                        None,
                    )
                    .await
                    .expect("open_object failed");
                    let mut buf = object.allocate_buffer(1000).await;
                    assert_eq!(object.read(0, buf.as_mut()).await.expect("read failed"), 1000);
                    for (i, byte) in buf.as_ptr_slice().iter_as::<u8>().enumerate() {
                        assert_eq!(byte, i as u8);
                    }
                }
            };

            check_object(fs.clone()).await;

            let fs = reopen(fs).await;

            // At this point the "test" volume is locked.  Before checking the object, flush the
            // filesystem.  This should leave a file with encrypted mutations.
            fs.object_manager()
                .flush(FlushReason::Journal(ForceMajor::False))
                .await
                .expect("flush failed");

            assert_ne!(
                fs.object_manager()
                    .store(store_object_id)
                    .unwrap()
                    .load_store_info()
                    .await
                    .expect("load_store_info failed")
                    .encrypted_mutations_object_id,
                INVALID_OBJECT_ID
            );

            check_object(fs.clone()).await;

            // Checking the object should have triggered a flush and so now there should be no
            // encrypted mutations object.
            assert_eq!(
                fs.object_manager()
                    .store(store_object_id)
                    .unwrap()
                    .load_store_info()
                    .await
                    .expect("load_store_info failed")
                    .encrypted_mutations_object_id,
                INVALID_OBJECT_ID
            );

            let fs = reopen(fs).await;

            fsck(fs.clone()).await.expect("fsck failed");

            let fs = reopen(fs).await;

            check_object(fs.clone()).await;

            fs
        }

        let mut fs = test_filesystem().await;
        let crypt = Arc::new(new_insecure_crypt());

        {
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let _store = root_volume
                .new_volume(
                    "test",
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
        }

        // Run a few iterations so that we test changes with the stream cipher offset.
        for i in 0..5 {
            fs = one_iteration(fs, crypt.clone(), i).await;
        }
    }

    #[test_case(true; "with a flush")]
    #[test_case(false; "without a flush")]
    #[fuchsia::test(threads = 10)]
    async fn test_object_id_cipher_roll(with_flush: bool) {
        let fs = test_filesystem().await;
        let crypt = Arc::new(new_insecure_crypt());

        let expected_key = {
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume(
                    "test",
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

            // Create some files so that our in-memory copy of StoreInfo has changes (the object
            // count) pending a flush.
            let root_dir_id = store.root_directory_object_id();
            let root_dir =
                Arc::new(Directory::open(&store, root_dir_id).await.expect("open failed"));
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), root_dir_id)],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            for i in 0..10 {
                root_dir.create_child_file(&mut transaction, &format!("file {i}")).await.unwrap();
            }
            transaction.commit().await.expect("commit failed");

            let orig_store_info = store.store_info().unwrap();

            // Hack the last object ID to force a roll of the object ID cipher.
            {
                let mut last_object_id = store.last_object_id.lock();
                match &mut *last_object_id {
                    LastObjectId::Encrypted { id, .. } => {
                        assert_eq!(*id & OBJECT_ID_HI_MASK, 0);
                        *id |= 0xffffffff;
                    }
                    _ => unreachable!(),
                }
            }

            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let object = root_directory
                .create_child_file(&mut transaction, "test")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");

            assert_eq!(object.object_id() & OBJECT_ID_HI_MASK, 1u64 << 32);

            // Check that the key has been changed.
            let key = match (
                store.store_info().unwrap().last_object_id,
                orig_store_info.last_object_id,
            ) {
                (
                    LastObjectIdInfo::Encrypted { key, id },
                    LastObjectIdInfo::Encrypted { key: orig_key, .. },
                ) => {
                    assert_ne!(key, orig_key);
                    assert_eq!(id, 1u64 << 32);
                    key
                }
                _ => unreachable!(),
            };

            if with_flush {
                fs.journal().force_compact().await.unwrap();
            }

            let last_object_id = store.last_object_id.lock();
            assert_eq!(last_object_id.id(), 1u64 << 32);
            key
        };

        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_volume
            .volume("test", StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() })
            .await
            .expect("volume failed");

        assert_matches!(store.store_info().unwrap().last_object_id, LastObjectIdInfo::Encrypted { key, .. } if key == expected_key);
        assert_eq!(store.last_object_id.lock().id(), 1u64 << 32);

        fsck(fs.clone()).await.expect("fsck failed");
        fsck_volume(&fs, store.store_object_id(), None).await.expect("fsck_volume failed");
    }

    #[fuchsia::test(threads = 2)]
    async fn test_race_object_id_cipher_roll_and_flush() {
        let fs = test_filesystem().await;
        let crypt = Arc::new(new_insecure_crypt());

        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_volume
            .new_volume(
                "test",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");

        assert!(matches!(&*store.last_object_id.lock(), LastObjectId::Encrypted { .. }));

        // Create some files so that our in-memory copy of StoreInfo has changes (the object
        // count) pending a flush.
        let root_dir_id = store.root_directory_object_id();
        let root_dir = Arc::new(Directory::open(&store, root_dir_id).await.expect("open failed"));

        let _executor_tasks = testing::force_executor_threads_to_run(2).await;

        for j in 0..100 {
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), root_dir_id)],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            root_dir.create_child_file(&mut transaction, &format!("file {j}")).await.unwrap();
            transaction.commit().await.expect("commit failed");

            let task = {
                let fs = fs.clone();
                fasync::Task::spawn(async move {
                    fs.journal().force_compact().await.unwrap();
                })
            };

            // Hack the last object ID to force a roll of the object ID cipher.
            {
                let mut last_object_id = store.last_object_id.lock();
                let LastObjectId::Encrypted { id, .. } = &mut *last_object_id else {
                    unreachable!()
                };
                assert_eq!(*id >> 32, j);
                *id |= 0xffffffff;
            }

            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            root_directory
                .create_child_file(&mut transaction, "test {j}")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");

            task.await;

            // Check that the key has been changed.
            let new_store_info = store.load_store_info().await.unwrap();

            let LastObjectIdInfo::Encrypted { id, key } = new_store_info.last_object_id else {
                unreachable!()
            };
            assert_eq!(id >> 32, j + 1);
            let LastObjectIdInfo::Encrypted { key: in_memory_key, .. } =
                store.store_info().unwrap().last_object_id
            else {
                unreachable!()
            };
            assert_eq!(key, in_memory_key);
        }

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_object_id_no_roll_for_unencrypted_store() {
        let fs = test_filesystem().await;

        {
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume("test", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");

            // Hack the last object ID.
            {
                let mut last_object_id = store.last_object_id.lock();
                match &mut *last_object_id {
                    LastObjectId::Unencrypted { id } => {
                        assert_eq!(*id & OBJECT_ID_HI_MASK, 0);
                        *id |= 0xffffffff;
                    }
                    _ => unreachable!(),
                }
            }

            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let object = root_directory
                .create_child_file(&mut transaction, "test")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");

            assert_eq!(object.object_id(), 0x1_0000_0000);

            // Check that there is still no key.
            assert_matches!(
                store.store_info().unwrap().last_object_id,
                LastObjectIdInfo::Unencrypted { .. }
            );

            assert_eq!(store.last_object_id.lock().id(), 0x1_0000_0000);
        };

        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let store =
            root_volume.volume("test", StoreOptions::default()).await.expect("volume failed");

        assert_eq!(store.last_object_id.lock().id(), 0x1_0000_0000);
    }

    #[fuchsia::test]
    fn test_object_id_is_not_invalid_object_id() {
        let key = UnwrappedKey::new(vec![0; FXFS_KEY_SIZE]);
        // 1106634048 results in INVALID_OBJECT_ID with this key.
        let mut last_object_id =
            LastObjectId::Encrypted { id: 1106634047, cipher: Box::new(Ff1::new(&key)) };
        assert!(last_object_id.try_get_next().is_some());
        assert!(last_object_id.try_get_next().is_some());
    }

    #[fuchsia::test]
    async fn test_last_object_id_is_correct_after_unlock() {
        let fs = test_filesystem().await;
        let crypt = Arc::new(new_insecure_crypt());

        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_volume
            .new_volume(
                "test",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");

        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        root_directory
            .create_child_file(&mut transaction, "test")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        // Compact so that StoreInfo is written.
        fs.journal().force_compact().await.unwrap();

        let last_object_id = store.last_object_id.lock().id();

        store.lock().await.unwrap();
        store.unlock(crypt.clone()).await.unwrap();

        assert_eq!(store.last_object_id.lock().id(), last_object_id);
    }

    #[fuchsia::test(threads = 20)]
    async fn test_race_when_rolling_last_object_id_cipher() {
        // NOTE: This test is trying to test a race, so if it fails, it might be flaky.

        const NUM_THREADS: usize = 20;

        let fs = test_filesystem().await;
        let crypt = Arc::new(new_insecure_crypt());

        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_volume
            .new_volume(
                "test",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");

        let store_id = store.store_object_id();
        let root_dir_id = store.root_directory_object_id();

        let root_directory =
            Arc::new(Directory::open(&store, root_dir_id).await.expect("open failed"));

        // Create directories.
        let mut directories = Vec::new();
        for _ in 0..NUM_THREADS {
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(store_id, root_dir_id,)],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            directories.push(
                root_directory
                    .create_child_dir(&mut transaction, "test")
                    .await
                    .expect("create_child_file failed"),
            );
            transaction.commit().await.expect("commit failed");
        }

        // Hack the last object ID so that the next ID will require a roll.
        match &mut *store.last_object_id.lock() {
            LastObjectId::Encrypted { id, .. } => *id |= 0xffff_ffff,
            _ => unreachable!(),
        }

        let scope = fasync::Scope::new();

        let _executor_tasks = testing::force_executor_threads_to_run(NUM_THREADS).await;

        for dir in directories {
            let fs = fs.clone();
            scope.spawn(async move {
                let mut transaction = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object(store_id, dir.object_id(),)],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                dir.create_child_file(&mut transaction, "test")
                    .await
                    .expect("create_child_file failed");
                transaction.commit().await.expect("commit failed");
            });
        }

        scope.on_no_tasks().await;

        assert_eq!(store.last_object_id.lock().id(), 0x1_0000_0000 + NUM_THREADS as u64 - 1);
    }

    #[fuchsia::test(threads = 10)]
    async fn test_lock_store() {
        let fs = test_filesystem().await;
        let crypt = Arc::new(new_insecure_crypt());

        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
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
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        root_directory
            .create_child_file(&mut transaction, "test")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");
        store.lock().await.expect("lock failed");

        store.unlock(crypt).await.expect("unlock failed");
        root_directory.lookup("test").await.expect("lookup failed").expect("not found");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_unlock_read_only() {
        let fs = test_filesystem().await;
        let crypt = Arc::new(new_insecure_crypt());

        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
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
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        root_directory
            .create_child_file(&mut transaction, "test")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");
        store.lock().await.expect("lock failed");

        store.unlock_read_only(crypt.clone()).await.expect("unlock failed");
        root_directory.lookup("test").await.expect("lookup failed").expect("not found");
        store.lock_read_only();
        store.unlock_read_only(crypt).await.expect("unlock failed");
        root_directory.lookup("test").await.expect("lookup failed").expect("not found");
    }

    #[fuchsia::test]
    async fn test_mutations_cipher_dropped_on_lock() {
        let fs = test_filesystem().await;
        let crypt = Arc::new(new_insecure_crypt());

        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
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

        // When created/unlocked, mutations_cipher is present in LockState::Unlocked
        // and last_object_id is Encrypted.
        assert_matches!(*store.lock_state.lock(), LockState::Unlocked { .. });
        assert!(matches!(&*store.last_object_id.lock(), LastObjectId::Encrypted { .. }));

        // When locked, LockState transitions to Locked, dropping mutations_cipher,
        // and last_object_id is reset to Pending.
        store.lock().await.expect("lock failed");
        assert_matches!(*store.lock_state.lock(), LockState::Locked);
        assert!(matches!(&*store.last_object_id.lock(), LastObjectId::Pending));

        // When unlocked again, mutations_cipher is re-created in LockState::Unlocked
        // and last_object_id is restored to Encrypted.
        store.unlock(crypt).await.expect("unlock failed");
        assert_matches!(*store.lock_state.lock(), LockState::Unlocked { .. });
        assert!(matches!(&*store.last_object_id.lock(), LastObjectId::Encrypted { .. }));
    }

    #[fuchsia::test(threads = 10)]
    async fn test_key_rolled_when_unlocked() {
        let fs = test_filesystem().await;
        let crypt = Arc::new(new_insecure_crypt());

        let object_id;
        {
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume(
                    "test",
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
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            object_id = root_directory
                .create_child_file(&mut transaction, "test")
                .await
                .expect("create_child_file failed")
                .object_id();
            transaction.commit().await.expect("commit failed");
        }

        fs.close().await.expect("Close failed");
        let mut device = fs.take_device().await;

        // Repeatedly remount so that we can be sure that we can remount when there are many
        // mutations keys.
        for _ in 0..100 {
            device.reopen(false);
            let fs = FxFilesystem::open(device).await.expect("open failed");
            {
                let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
                let store = root_volume
                    .volume(
                        "test",
                        StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    )
                    .await
                    .expect("open_volume failed");

                // The key should get rolled every time we unlock.
                {
                    let lock_state = store.lock_state.lock();
                    let LockState::Unlocked { mutations_cipher, .. } = &*lock_state else {
                        panic!("Unexpected lock state: {lock_state:?}");
                    };
                    assert!(mutations_cipher.key_is_new());
                }

                // Make sure there's an encrypted mutation.
                let handle =
                    ObjectStore::open_object(&store, object_id, HandleOptions::default(), None)
                        .await
                        .expect("open_object failed");
                let buffer = handle.allocate_buffer(100).await;
                handle
                    .write_or_append(Some(0), buffer.as_ref())
                    .await
                    .expect("write_or_append failed");
            }
            fs.close().await.expect("Close failed");
            device = fs.take_device().await;
        }
    }

    #[test]
    fn test_store_info_max_serialized_size() {
        let info = StoreInfo {
            guid: [0xff; 16],
            last_object_id: LastObjectIdInfo::Encrypted {
                id: 0x1234567812345678,
                key: FxfsKey {
                    wrapping_key_id: 0x1234567812345678u128.to_le_bytes(),
                    key: WrappedKeyBytes::from([0xff; FXFS_WRAPPED_KEY_SIZE]),
                },
            },
            // Worst case, each layer should be 3/4 the size of the layer below it (because of the
            // compaction policy we're using).  If the smallest layer is 8,192 bytes, then 120
            // layers would take up a size that exceeds a 64 bit unsigned integer, so if this fits,
            // any size should fit.
            layers: vec![0x1234567812345678; 120],
            root_directory_object_id: 0x1234567812345678,
            graveyard_directory_object_id: 0x1234567812345678,
            object_count: 0x1234567812345678,
            mutations_key: Some(FxfsKey {
                wrapping_key_id: 0x1234567812345678u128.to_le_bytes(),
                key: WrappedKeyBytes::from([0xff; FXFS_WRAPPED_KEY_SIZE]),
            }),
            mutations_cipher_offset: 0x1234567812345678,
            encrypted_mutations_object_id: 0x1234567812345678,
            internal_directory_object_id: INVALID_OBJECT_ID,
        };
        let mut serialized_info = Vec::new();
        info.serialize_with_version(&mut serialized_info).unwrap();
        assert!(
            serialized_info.len() <= MAX_STORE_INFO_SERIALIZED_SIZE,
            "{}",
            serialized_info.len()
        );
    }

    async fn reopen_after_crypt_failure_inner(read_only: bool) {
        let fs = test_filesystem().await;
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");

        let store = {
            let crypt = Arc::new(new_insecure_crypt());
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
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
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
            root_directory
                .create_child_file(&mut transaction, "test")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");

            crypt.shutdown();
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
            root_directory
                .create_child_file(&mut transaction, "test2")
                .await
                .map(|_| ())
                .expect_err("create_child_file should fail");
            store.lock().await.expect("lock failed");
            store
        };

        let crypt = Arc::new(new_insecure_crypt());
        if read_only {
            store.unlock_read_only(crypt).await.expect("unlock failed");
        } else {
            store.unlock(crypt).await.expect("unlock failed");
        }
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        root_directory.lookup("test").await.expect("lookup failed").expect("not found");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_reopen_after_crypt_failure() {
        reopen_after_crypt_failure_inner(false).await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_reopen_read_only_after_crypt_failure() {
        reopen_after_crypt_failure_inner(true).await;
    }

    #[fuchsia::test(threads = 10)]
    #[should_panic(expected = "Insufficient reservation space")]
    #[cfg(debug_assertions)]
    async fn large_transaction_causes_panic_in_debug_builds() {
        let fs = test_filesystem().await;
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_volume
            .new_volume("vol", NewChildStoreOptions::default())
            .await
            .expect("new_volume failed");
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("transaction");
        for i in 0..500 {
            root_directory
                .create_symlink(&mut transaction, b"link", &format!("{}", i))
                .await
                .expect("symlink");
        }
        assert_eq!(transaction.commit().await.expect("commit"), 0);
    }

    #[fuchsia::test]
    async fn test_crypt_failure_does_not_fuse_journal() {
        let fs = test_filesystem().await;

        {
            // Create two stores and a record for each store, so the journal will need to flush them
            // both later.
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store1 = root_volume
                .new_volume(
                    "vol1",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(Arc::new(new_insecure_crypt())),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");
            let crypt = Arc::new(new_insecure_crypt());
            let store2 = root_volume
                .new_volume(
                    "vol2",
                    NewChildStoreOptions {
                        options: StoreOptions { crypt: Some(crypt.clone()) },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");
            for store in [&store1, &store2] {
                let root_directory = Directory::open(store, store.root_directory_object_id())
                    .await
                    .expect("open failed");
                let mut transaction = store
                    .new_transaction(
                        lock_keys![LockKey::object(
                            store.store_object_id(),
                            root_directory.object_id()
                        )],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                root_directory
                    .create_child_file(&mut transaction, "test")
                    .await
                    .expect("create_child_file failed");
                transaction.commit().await.expect("commit failed");
            }

            // Shut down the crypt instance for store2.
            crypt.shutdown();

            // Compact. This flushes store2 (using cached key) and store1.  This consumes 1 cached
            // key from store2.
            fs.journal().force_compact().await.expect("compact failed");

            // Write to store2 again. This should fail because store2 needs to top up its cached
            // keys, which requires calling the (dead) crypt service.
            let root_directory2 = Directory::open(&store2, store2.root_directory_object_id())
                .await
                .expect("open failed");
            let (child_id, _, _) = root_directory2
                .lookup("test")
                .await
                .expect("lookup failed")
                .expect("test file not found");
            let mut transaction2 = store2
                .new_transaction(
                    lock_keys![
                        LockKey::object(store2.store_object_id(), root_directory2.object_id()),
                        LockKey::object(store2.store_object_id(), child_id),
                    ],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            replace_child(&mut transaction2, None, (&root_directory2, "test"))
                .await
                .expect("replace_child failed");
            assert!(transaction2.commit().await.is_err());

            // Write to store1 should still succeed (its crypt is not dead).
            let root_directory1 = Directory::open(&store1, store1.root_directory_object_id())
                .await
                .expect("open failed");
            let mut transaction1 = store1
                .new_transaction(
                    lock_keys![LockKey::object(
                        store1.store_object_id(),
                        root_directory1.object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            root_directory1
                .create_child_file(&mut transaction1, "test2")
                .await
                .expect("create_child_file failed");
            transaction1.commit().await.expect("commit failed");

            // Compact again. Should succeed.
            fs.journal().force_compact().await.expect("compact failed");
        }

        // Close and reopen to verify.
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");

        // vol1 should have "test" and "test2".
        let store1 = root_volume
            .volume(
                "vol1",
                StoreOptions {
                    crypt: Some(Arc::new(new_insecure_crypt())),
                    ..StoreOptions::default()
                },
            )
            .await
            .expect("volume failed");
        let root_directory1 =
            Directory::open(&store1, store1.root_directory_object_id()).await.expect("open failed");
        assert!(root_directory1.lookup("test").await.expect("lookup failed").is_some());
        assert!(root_directory1.lookup("test2").await.expect("lookup failed").is_some());

        // vol2 should only have "test".
        let store2 = root_volume
            .volume(
                "vol2",
                StoreOptions {
                    crypt: Some(Arc::new(new_insecure_crypt())),
                    ..StoreOptions::default()
                },
            )
            .await
            .expect("volume failed");
        let root_directory2 =
            Directory::open(&store2, store2.root_directory_object_id()).await.expect("open failed");
        assert!(root_directory2.lookup("test").await.expect("lookup failed").is_some());
        assert!(root_directory2.lookup("test2").await.expect("lookup failed").is_none());

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn test_crypt_failure_during_unlock_race() {
        let fs = test_filesystem().await;

        let store_object_id = {
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume(
                    "vol",
                    NewChildStoreOptions {
                        options: StoreOptions { crypt: Some(Arc::new(new_insecure_crypt())) },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
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
            root_directory
                .create_child_file(&mut transaction, "test")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            store.store_object_id()
        };

        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);

        let fs = FxFilesystem::open(device).await.expect("open failed");
        {
            let fs_clone = fs.clone();
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");

            let crypt = Arc::new(new_insecure_crypt());
            let crypt_clone = crypt.clone();
            join!(
                async move {
                    // Unlock might fail, so ignore errors.
                    let _ =
                        root_volume.volume("vol", StoreOptions { crypt: Some(crypt_clone) }).await;
                },
                async move {
                    // Block until unlock is finished but before flushing due to unlock is finished, to
                    // maximize the chances of weirdness.
                    let keys = lock_keys![LockKey::flush(store_object_id)];
                    let _ = fs_clone.lock_manager().write_lock(keys).await;
                    crypt.shutdown();
                }
            );
        }

        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);

        let fs = FxFilesystem::open(device).await.expect("open failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_volume
            .volume(
                "vol",
                StoreOptions {
                    crypt: Some(Arc::new(new_insecure_crypt())),
                    ..StoreOptions::default()
                },
            )
            .await
            .expect("open volume failed");
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        assert!(root_directory.lookup("test").await.expect("lookup failed").is_some());

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn test_low_32_bit_object_ids() {
        let device = DeviceHolder::new(FakeDevice::new(16384, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");

        {
            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");

            let store = root_vol
                .new_volume(
                    "test",
                    NewChildStoreOptions { low_32_bit_object_ids: true, ..Default::default() },
                )
                .await
                .expect("new_volume failed");

            let root_dir = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");

            let mut ids = std::collections::HashSet::new();

            for i in 0..100 {
                let mut transaction = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object(store.store_object_id(), root_dir.object_id())],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");

                for j in 0..100 {
                    let object = root_dir
                        .create_child_dir(&mut transaction, &format!("{i}.{j}"))
                        .await
                        .expect("create_child_file failed");

                    assert!(object.object_id() < 1 << 32);
                    assert_ne!(object.object_id(), INVALID_OBJECT_ID);
                    assert!(ids.insert(object.object_id()));
                }

                transaction.commit().await.expect("commit failed");
            }

            assert_matches!(store.store_info().unwrap().last_object_id, LastObjectIdInfo::Low32Bit);

            fsck_volume(&fs, store.store_object_id(), None).await.expect("fsck_volume failed");
        }

        // Verify persistence
        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_vol.volume("test", StoreOptions::default()).await.expect("volume failed");

        // Check that we can still create files and they have low 32-bit IDs.
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

        let object = root_dir
            .create_child_file(&mut transaction, "persistence_check")
            .await
            .expect("create_child_file failed");
        assert!(object.object_id() < 1 << 32);

        transaction.commit().await.expect("commit failed");

        assert_matches!(store.store_info().unwrap().last_object_id, LastObjectIdInfo::Low32Bit);
    }

    struct StallingCrypt {
        delegate: Arc<dyn Crypt>,
        unwrap_stall_counter: AtomicIsize,
        unwrap_stalled_tx: Mutex<Option<oneshot::Sender<oneshot::Sender<()>>>>,
        create_key_stalled_tx: Mutex<Option<oneshot::Sender<oneshot::Sender<()>>>>,
    }

    impl StallingCrypt {
        fn new(delegate: Arc<dyn Crypt>) -> Self {
            Self {
                delegate,
                unwrap_stall_counter: AtomicIsize::new(-1),
                unwrap_stalled_tx: Mutex::new(None),
                create_key_stalled_tx: Mutex::new(None),
            }
        }

        fn stall_on_unwrap(
            self: &Arc<Self>,
            index: usize,
        ) -> oneshot::Receiver<oneshot::Sender<()>> {
            let (tx, rx) = oneshot::channel();
            self.unwrap_stall_counter.store(index as isize, Ordering::Relaxed);
            *self.unwrap_stalled_tx.lock() = Some(tx);
            rx
        }

        fn stall_on_create_key(self: &Arc<Self>) -> oneshot::Receiver<oneshot::Sender<()>> {
            let (tx, rx) = oneshot::channel();
            *self.create_key_stalled_tx.lock() = Some(tx);
            rx
        }
    }

    #[async_trait]
    impl Crypt for StallingCrypt {
        async fn create_key(
            &self,
            owner: u64,
            purpose: KeyPurpose,
        ) -> Result<(FxfsKey, UnwrappedKey), zx::Status> {
            if matches!(purpose, KeyPurpose::Data) {
                let stalled_tx = self.create_key_stalled_tx.lock().take();
                if let Some(tx) = stalled_tx {
                    let (continue_tx, continue_rx) = oneshot::channel();
                    let _ = tx.send(continue_tx);
                    let _ = continue_rx.await;
                }
            }
            self.delegate.create_key(owner, purpose).await
        }

        async fn unwrap_key(
            &self,
            wrapped_key: &WrappedKey,
            owner: u64,
        ) -> Result<UnwrappedKey, zx::Status> {
            let count = self.unwrap_stall_counter.fetch_sub(1, Ordering::Relaxed);
            log::info!("unwrap_key called, count was {}, owner {}", count, owner);
            if count == 0 {
                log::info!("unwrap_key stalling");
                let stalled_tx = self.unwrap_stalled_tx.lock().take();
                if let Some(tx) = stalled_tx {
                    let (continue_tx, continue_rx) = oneshot::channel();
                    let _ = tx.send(continue_tx);
                    let _ = continue_rx.await;
                }
                log::info!("unwrap_key resumed");
            }
            self.delegate.unwrap_key(wrapped_key, owner).await
        }

        async fn create_key_with_id(
            &self,
            owner: u64,
            wrapping_key_id: WrappingKeyId,
            object_type: ObjectType,
        ) -> Result<(EncryptionKey, UnwrappedKey), zx::Status> {
            self.delegate.create_key_with_id(owner, wrapping_key_id, object_type).await
        }
    }

    #[fuchsia::test]
    async fn test_fsck_during_key_pre_cache_stall() {
        let device = DeviceHolder::new(FakeDevice::new(16384, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystemBuilder::new().format(true).open(device).await.expect("open failed");

        // Initialize crypt without blocker so new_volume doesn't stall.
        let crypt = Arc::new(StallingCrypt::new(Arc::new(new_insecure_crypt())));

        let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_vol
            .new_volume(
                "test",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");

        // Now install the blocker.
        let stalled_rx = crypt.stall_on_create_key();

        // The volume was created, so it has 2 cached keys.
        // We want to exhaust them so that the next transaction tries to pre-cache more.
        {
            let mut lock_state = store.lock_state.lock();
            if let super::LockState::Unlocked { cached_keys, .. } = &mut *lock_state {
                cached_keys.clear();
            }
        }

        // Start a transaction in the background. It will try to pre-cache keys,
        // and should stall on the crypt service.
        let store_clone = store.clone();
        let root_dir_id = store.root_directory_object_id();

        let tx_join_handle = fasync::Task::spawn(async move {
            let root_dir = Directory::open(&store_clone, root_dir_id).await.expect("open failed");
            let mut transaction = store_clone
                .new_transaction(
                    lock_keys![LockKey::object(
                        store_clone.store_object_id(),
                        root_dir.object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let name = "foo";
            root_dir
                .create_child_file(&mut transaction, name)
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
        });

        // Wait until the transaction has actually stalled on crypt.
        let continue_tx = stalled_rx.await.expect("stalled_rx failed");

        // While it is blocked, we should still be able to run fsck.
        fsck(fs.clone()).await.expect("fsck failed");

        // Unblock the crypt service so the transaction can complete.
        let _ = continue_tx.send(());
        tx_join_handle.await;

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_writes_to_other_store_not_blocked_by_stall() {
        let device = DeviceHolder::new(FakeDevice::new(16384, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystemBuilder::new()
            .format(true)
            .journal_options(JournalOptions { reclaim_size: 32_768, ..Default::default() })
            .open(device)
            .await
            .expect("open failed");

        // Initialize crypt for store1 with a blocker.
        // We will enable it after volume creation.
        let crypt1 = Arc::new(StallingCrypt::new(Arc::new(new_insecure_crypt())));

        // Initialize normal crypt for store2.
        let crypt2 = Arc::new(new_insecure_crypt());

        let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");

        let store1 = root_vol
            .new_volume(
                "test1",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(crypt1.clone()),
                        ..StoreOptions::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");

        store1.flush().await.expect("flush failed");

        let store2 = root_vol
            .new_volume(
                "test2",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(crypt2.clone()),
                        ..StoreOptions::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");

        // Now install the blocker on crypt1.
        let stalled_rx = crypt1.stall_on_create_key();

        // Exhaust cached keys for store1 so next txn tries to pre-cache.
        {
            let mut lock_state = store1.lock_state.lock();
            if let super::LockState::Unlocked { cached_keys, .. } = &mut *lock_state {
                cached_keys.clear();
            }
        }

        // Start a transaction on store1 in the background.
        // It should stall on crypt1.
        let store1_clone = store1.clone();
        let root_dir1_id = store1.root_directory_object_id();

        let tx_join_handle = fasync::Task::spawn(async move {
            let root_dir1 =
                Directory::open(&store1_clone, root_dir1_id).await.expect("open failed");
            let mut transaction = store1_clone
                .new_transaction(
                    lock_keys![LockKey::object(
                        store1_clone.store_object_id(),
                        root_dir1.object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            root_dir1
                .create_child_file(&mut transaction, "foo")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
        });

        // Wait until store1 transaction has actually stalled.
        let continue_tx = stalled_rx.await.expect("stalled_rx failed");

        // While store1 is blocked, we should still be able to write to store2.
        let root_dir2 =
            Directory::open(&store2, store2.root_directory_object_id()).await.expect("open failed");
        let long_name = "a".repeat(255);
        let mut i = 0;
        while store2.counters.lock().num_flushes < 3 {
            if i > 200 {
                panic!(
                    "Failed to trigger 3 compactions after 200 transactions. Flushes: {}",
                    store2.counters.lock().num_flushes
                );
            }
            let mut transaction = store2
                .new_transaction(
                    lock_keys![LockKey::object(store2.store_object_id(), root_dir2.object_id())],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            root_dir2
                .create_child_file(&mut transaction, &format!("{}-{:03}", long_name, i))
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            i += 1;
            fasync::Timer::new(std::time::Duration::from_millis(5)).await;
        }

        // Unblock crypt1.
        let _ = continue_tx.send(());
        tx_join_handle.await;

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_concurrent_transactions_exhaust_cached_keys() {
        let fs = test_filesystem().await;

        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let crypt = Arc::new(new_insecure_crypt());
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..Default::default() },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");

        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        // Create three directories first, so we can run transactions concurrently
        // without blocking on directory locks.
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let dir1 = root_directory
            .create_child_dir(&mut transaction, "dir1")
            .await
            .expect("create_child_dir failed");
        let dir2 = root_directory
            .create_child_dir(&mut transaction, "dir2")
            .await
            .expect("create_child_dir failed");
        let dir3 = root_directory
            .create_child_dir(&mut transaction, "dir3")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");

        // Start three transactions. They will all see that the cached keys are full
        // (size 2) after the first one tops them up, so they will all succeed to start
        // without calling crypt again (except the first one which tops up).

        // Transaction 1
        let mut transaction1 = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), dir1.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction 1 failed");
        dir1.create_child_file(&mut transaction1, "file1")
            .await
            .expect("create_child_file 1 failed");

        // Transaction 2
        let mut transaction2 = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), dir2.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction 2 failed");
        dir2.create_child_file(&mut transaction2, "file2")
            .await
            .expect("create_child_file 2 failed");

        // Transaction 3
        let mut transaction3 = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), dir3.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction 3 failed");
        dir3.create_child_file(&mut transaction3, "file3")
            .await
            .expect("create_child_file 3 failed");

        // Now shut down the crypt service.
        crypt.shutdown();

        // Commit transaction 1 and compact. This should succeed and consume 1 cached key.
        transaction1.commit().await.expect("commit 1 failed");
        fs.journal().force_compact().await.expect("compact 1 failed");

        // Commit transaction 2 should FAIL because it tries to top up (since cache size is 1 < 2)
        // and the crypt service is dead.
        assert!(transaction2.commit().await.is_err());

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_key_exhaustion_race() {
        let fs;
        let in_hook = AtomicBool::new(false);
        let (mut hooks, fs_hooks) = Hooks::new();
        fs = test_filesystem_with_hooks(fs_hooks).await;

        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let crypt = Arc::new(new_insecure_crypt());
        let store = root_volume
            .new_volume(
                "vol",
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..Default::default() },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");

        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        // Commit a transaction to ensure the store has dirty mutations and needs a flush.
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        root_directory
            .create_child_dir(&mut transaction, "dir")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("commit failed");

        hooks.set_before_commit(|| {
            if in_hook.swap(true, Ordering::Relaxed) {
                return;
            }

            // Run compaction. Since this hook runs before the next transaction acquires the
            // commit lock, this compaction will flush the mutations committed above and consume
            // one cached key.
            futures::executor::block_on(fs.journal().force_compact()).expect("compact failed");
        });

        // Start a second transaction. When we commit this transaction, the hook we set up above
        // will trigger.
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), root_directory.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        root_directory
            .create_child_file(&mut transaction, "file1")
            .await
            .expect("create_child_file failed");

        // When we commit:
        // 1. `prepare_commit` runs. It checks the key cache. If the limit is 1, it sees 1 key
        //    (which is >= the limit), so it does not top up. If the limit is 2, it sees 2 keys
        //    (which is >= the limit), so it also does not top up.
        // 2. The hook runs compaction. Compaction flushes the first transaction's mutations,
        //    consuming one cached key.
        // 3. This transaction commits. The store is marked as needing a flush. If the limit was
        //    1, the cache is now empty. If the limit was 2, the cache has 1 key left.
        transaction.commit().await.expect("commit failed");

        // Run compaction again. It will try to flush the second transaction's mutations.
        // If the limit is 1, this will fail because the cache is empty.
        // If the limit is 2, `prepare_commit` would have topped up the cache to 2 keys, so
        // compaction would have left 1 key, and this compaction will succeed.
        fs.journal().force_compact().await.expect("compaction failed");

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_unlock_pre_cache_stall_does_not_block_other_stores() {
        let device = DeviceHolder::new(FakeDevice::new(16384, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystemBuilder::new()
            .format(true)
            .journal_options(JournalOptions { reclaim_size: 32_768, ..Default::default() })
            .open(device)
            .await
            .expect("open failed");

        let crypt1 = Arc::new(StallingCrypt::new(Arc::new(new_insecure_crypt())));
        let crypt2 = Arc::new(new_insecure_crypt());

        let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");

        let store1 = root_vol
            .new_volume(
                "test1",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(crypt1.clone()),
                        ..StoreOptions::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");

        // Write some data to store1 and lock it.
        {
            let mut transaction = store1
                .new_transaction(
                    lock_keys![LockKey::object(
                        store1.store_object_id(),
                        store1.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let root_dir1 = Directory::open(&store1, store1.root_directory_object_id())
                .await
                .expect("open failed");
            root_dir1
                .create_child_file(&mut transaction, "foo")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
        }
        store1.lock().await.expect("lock failed");

        let store2 = root_vol
            .new_volume(
                "test2",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: Some(crypt2.clone()),
                        ..StoreOptions::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");

        // Now install the blocker on crypt1.
        let stalled_rx = crypt1.stall_on_create_key();

        // Start unlocking store1 in the background. It should stall on crypt1 during
        // pre_cache_keys.
        let store1_clone = store1.clone();
        let crypt1_clone = crypt1.clone();
        let unlock_join_handle = fasync::Task::spawn(async move {
            store1_clone.unlock(crypt1_clone).await.expect("unlock failed");
        });

        // Wait until store1 unlock has actually stalled.
        let continue_tx = stalled_rx.await.expect("stalled_rx failed");

        // While store1's unlock is blocked, we should still be able to write to store2
        // and trigger compactions.
        let root_dir2 =
            Directory::open(&store2, store2.root_directory_object_id()).await.expect("open failed");
        let long_name = "a".repeat(255);
        let mut i = 0;
        while store2.counters.lock().num_flushes < 3 {
            if i > 200 {
                panic!(
                    "Failed to trigger 3 compactions after 200 transactions. Flushes: {}",
                    store2.counters.lock().num_flushes
                );
            }
            let mut transaction = store2
                .new_transaction(
                    lock_keys![LockKey::object(store2.store_object_id(), root_dir2.object_id())],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            root_dir2
                .create_child_file(&mut transaction, &format!("{}-{:03}", long_name, i))
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            i += 1;
            fasync::Timer::new(std::time::Duration::from_millis(5)).await;
        }

        // Unblock crypt1.
        let _ = continue_tx.send(());
        unlock_join_handle.await;

        fs.close().await.expect("Close failed");
    }

    async fn run_unlock_stall_test(unwrap_stall_index: usize) -> bool {
        log::info!("Starting run_unlock_stall_test for index {}", unwrap_stall_index);
        let device = DeviceHolder::new(FakeDevice::new(16384, TEST_DEVICE_BLOCK_SIZE));
        let crypt1 = Arc::new(new_insecure_crypt());
        let crypt2 = Arc::new(new_insecure_crypt());
        let (store1_id, device) = {
            let fs = FxFilesystemBuilder::new()
                .format(true)
                .journal_options(JournalOptions { reclaim_size: 32_768, ..Default::default() })
                .open(device)
                .await
                .expect("open failed");

            let store1_id = {
                let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");

                let store1 = root_vol
                    .new_volume(
                        "test1",
                        NewChildStoreOptions {
                            options: StoreOptions {
                                crypt: Some(crypt1.clone()),
                                ..StoreOptions::default()
                            },
                            ..Default::default()
                        },
                    )
                    .await
                    .expect("new_volume failed");

                root_vol
                    .new_volume(
                        "test2",
                        NewChildStoreOptions {
                            options: StoreOptions {
                                crypt: Some(crypt2.clone()),
                                ..StoreOptions::default()
                            },
                            ..Default::default()
                        },
                    )
                    .await
                    .expect("new_volume failed");

                let root_dir1 = Directory::open(&store1, store1.root_directory_object_id())
                    .await
                    .expect("open failed");

                for i in 0..8 {
                    let mut transaction = store1
                        .new_transaction(
                            lock_keys![LockKey::object(
                                store1.store_object_id(),
                                root_dir1.object_id()
                            )],
                            Options::default(),
                        )
                        .await
                        .expect("new_transaction failed");
                    let name = format!("file_{i}");
                    root_dir1
                        .create_child_file(&mut transaction, &name)
                        .await
                        .expect("create_child_file failed");
                    transaction.commit().await.expect("commit failed");
                }

                // Flush store1 to roll the key.
                store1.flush().await.expect("flush failed");

                // Now write something after the flush, so it is encrypted with the new key
                // and remains in the journal.
                {
                    let mut transaction = store1
                        .new_transaction(
                            lock_keys![LockKey::object(
                                store1.store_object_id(),
                                root_dir1.object_id()
                            )],
                            Options::default(),
                        )
                        .await
                        .expect("new_transaction failed");
                    root_dir1
                        .create_child_file(&mut transaction, "file_after_flush")
                        .await
                        .expect("create_child_file failed");
                    transaction.commit().await.expect("commit failed");
                }
                store1.store_object_id()
            };

            fs.close().await.expect("Close failed");
            let device = fs.take_device().await;
            (store1_id, device)
        };
        device.reopen(false);

        log::info!("Opening filesystem");
        let fs = FxFilesystemBuilder::new()
            .journal_options(JournalOptions { reclaim_size: 32_768, ..Default::default() })
            .open(device)
            .await
            .expect("open failed");
        log::info!("Opened filesystem, getting root volume");
        let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
        log::info!("Got root volume");

        log::info!("Re-opening store2");
        // Re-open store2.
        let store2 = root_vol
            .volume("test2", StoreOptions { crypt: Some(crypt2), ..StoreOptions::default() })
            .await
            .expect("volume failed");
        log::info!("Re-opened store2");

        // Prepare stalling crypt for store1.
        let stalling_crypt1 = Arc::new(StallingCrypt::new(crypt1));
        let stalled_rx = stalling_crypt1.stall_on_unwrap(unwrap_stall_index);

        // Get store1 handle from object manager without unlocking.
        let store1 = fs.object_manager().store(store1_id).unwrap();

        log::info!("Spawning store1 unlock task");
        // Start unlocking store1 in the background.
        let store1_clone = store1.clone();
        let stalling_crypt1_clone = stalling_crypt1.clone();
        let unlock_join_handle = fasync::Task::spawn(async move {
            log::info!("store1 unlock task started");
            store1_clone.unlock(stalling_crypt1_clone).await.expect("unlock failed");
            log::info!("store1 unlock task finished");
        });

        // Wait for either the stall to occur or unlock to complete.
        let continue_tx = futures::select! {
            res = stalled_rx.fuse() => {
                log::info!("Stalled at index {}", unwrap_stall_index);
                Some(res.expect("stalled_rx failed"))
            }
            _ = unlock_join_handle.fuse() => {
                log::info!("Unlock completed without stalling at index {}", unwrap_stall_index);
                None
            }
        };

        let stalled = continue_tx.is_some();

        if let Some(continue_tx) = continue_tx {
            log::info!("Writing to store2 while stalled at index {}", unwrap_stall_index);
            // While store1's unlock is blocked, try to write to store2 and trigger compactions.
            let root_dir2 = Directory::open(&store2, store2.root_directory_object_id())
                .await
                .expect("open failed");
            let long_name = "a".repeat(255);
            let mut i = 0;
            while store2.counters.lock().num_flushes < 3 {
                if i > 200 {
                    panic!(
                        "Failed to trigger 3 compactions after 200 transactions. Flushes: {}",
                        store2.counters.lock().num_flushes
                    );
                }
                let mut transaction = store2
                    .new_transaction(
                        lock_keys![LockKey::object(
                            store2.store_object_id(),
                            root_dir2.object_id()
                        )],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                root_dir2
                    .create_child_file(&mut transaction, &format!("{}-{:03}", long_name, i))
                    .await
                    .expect("create_child_file failed");
                transaction.commit().await.expect("commit failed");
                i += 1;
                fasync::Timer::new(std::time::Duration::from_millis(5)).await;
            }

            log::info!("Unblocking store1 at index {}", unwrap_stall_index);
            // Unblock store1 unlock.
            let _ = continue_tx.send(());
        }

        fs.close().await.expect("Close failed");
        log::info!(
            "Finished run_unlock_stall_test for index {}, stalled: {}",
            unwrap_stall_index,
            stalled
        );
        stalled
    }

    #[fuchsia::test(threads = 10)]
    async fn test_unlock_replay_stall_does_not_block_other_stores() {
        let mut unwrap_stall_index = 0;
        loop {
            let stalled = run_unlock_stall_test(unwrap_stall_index).await;
            if !stalled {
                break;
            }
            unwrap_stall_index += 1;
        }
    }

    #[fuchsia::test(threads = 10)]
    async fn test_unlock_flush_race() {
        let device = DeviceHolder::new(FakeDevice::new(16384, TEST_DEVICE_BLOCK_SIZE));
        let crypt = Arc::new(new_insecure_crypt());

        // Phase 1: Format and set up store1 with some mutations.
        let (store1_id, device) = {
            let (store1_id, fs) = {
                let fs = FxFilesystemBuilder::new()
                    .format(true)
                    .open(device)
                    .await
                    .expect("open failed");
                let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
                let store1 = root_vol
                    .new_volume(
                        "test1",
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

                let root_dir = Directory::open(&store1, store1.root_directory_object_id())
                    .await
                    .expect("open failed");
                let mut transaction = store1
                    .new_transaction(
                        lock_keys![LockKey::object(store1.store_object_id(), root_dir.object_id())],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                root_dir
                    .create_child_file(&mut transaction, "file1")
                    .await
                    .expect("create_child_file failed");
                transaction.commit().await.expect("commit failed");

                (store1.store_object_id(), fs)
            };
            fs.close().await.expect("Close failed");
            let device = fs.take_device().await;
            (store1_id, device)
        };
        device.reopen(false);

        // Phase 2: Reopen and unlock with a race.
        let fs;
        let store1;
        let (mut hooks, fs_hooks) = Hooks::new();
        fs = FxFilesystemBuilder::new().hooks(fs_hooks).open(device).await.expect("open failed");

        store1 = fs.object_manager().store(store1_id).unwrap();

        // Set up the callback to trigger a flush of store1 during unlock (when flush
        // lock is dropped).
        hooks.set_unlock_resources_acquired(|| {
            futures::executor::block_on(store1.flush()).expect("flush failed");
        });

        store1.unlock(crypt).await.expect("unlock failed");

        fs.journal().force_compact().await.expect("compact failed");

        fsck(fs.clone()).await.expect("fsck failed");

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_open_object_with_illegal_key_in_root_store() {
        let fs = test_filesystem().await;
        let crypt = Arc::new(new_insecure_crypt());
        let object_id = {
            let store = fs.root_store();
            let mut transaction = store
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");

            let reserved_id = store.get_next_object_id().await.expect("get_next_object_id failed");
            let object_id = reserved_id.get();

            let (fxfs_key, unwrapped_key) =
                crypt.create_key(object_id, KeyPurpose::Data).await.expect("create_key failed");

            let handle = ObjectStore::create_object_with_id(
                &store,
                &mut transaction,
                reserved_id,
                HandleOptions::default(),
                Some(ObjectEncryptionOptions {
                    permanent: false,
                    key_id: 0,
                    key: EncryptionKey::Fxfs(fxfs_key),
                    unwrapped_key,
                }),
            )
            .expect("create_object_with_id failed");
            transaction.commit().await.expect("commit failed");

            let buf = handle.allocate_buffer(fs.block_size().get() as usize).await;
            handle.write_or_append(None, buf.as_ref()).await.expect("Write some data");

            // Manually overwrite keys to be appended with an illegal key type. It won't even
            // attempt to use the key since nothing references it, but it will still cause the
            // failure.
            let old_keys = store.get_keys(object_id).await.expect("Old keys should work fine");
            let mut new_keys =
                vec![(1, EncryptionKey::FscryptInoLblk32File { key_identifier: [0; 16] })];
            for key in old_keys.iter() {
                new_keys.push(key.clone());
            }
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::Object {
                        store_object_id: store.store_object_id,
                        object_id
                    }],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            transaction.add(
                store.store_object_id(),
                Mutation::replace_or_insert_object(
                    ObjectKey::keys(object_id),
                    ObjectValue::keys(new_keys.into()),
                ),
            );
            transaction.commit().await.expect("commit failed");

            object_id
        };

        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");

        let store = fs.root_store();
        assert!(
            ObjectStore::open_object(&store, object_id, HandleOptions::default(), Some(crypt))
                .await
                .is_err()
        );

        let res = store.get_keys(object_id).await;
        assert!(matches!(res, Err(e) if FxfsError::IntegrityError.matches(&e)));
    }

    // Tests mixing the old encryption format with the new by writing an encrypted mutations object
    // in the old format and manually attaching it to the object store.
    #[fuchsia::test]
    async fn test_encrypted_mutations_mixed_versions() {
        let fs = test_filesystem().await;
        let crypt = Arc::new(new_insecure_crypt());

        // Create a new encrypted child store "test" and flush so its root directory is in layers.
        let (store_object_id, root_dir_id, unwrapped_key) = {
            let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume(
                    "test",
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
            let store_object_id = store.store_object_id();
            let root_dir_id = store.root_directory_object_id();
            let store_info = store.store_info().unwrap();
            let key = store_info.mutations_key.as_ref().unwrap().clone();
            let unwrapped_key = crypt
                .unwrap_key(&fxfs_crypto::WrappedKey::Fxfs(key.into()), store_object_id)
                .await
                .expect("unwrap_key failed");

            // Flush the filesystem so that the store creation and root directory are saved to layers.
            fs.object_manager()
                .flush(FlushReason::Journal(ForceMajor::False))
                .await
                .expect("flush failed");

            (store_object_id, root_dir_id, unwrapped_key)
        };

        // Generate mutations for the encrypted mutations object, then serialize and encrypt them
        // as the old version (ChaCha20) encrypted mutations object.
        let store = fs.object_manager().store(store_object_id).unwrap();
        let (old_file_oid, chacha_offset) = {
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(store_object_id, root_dir_id)],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let root_directory =
                Directory::open(&store, root_dir_id).await.expect("open root dir failed");
            let old_file = root_directory
                .create_child_file(&mut transaction, "old_file")
                .await
                .expect("create_child_file failed");
            let oid = old_file.object_id();

            let mutations = transaction.take_mutations();
            let mut old_data = Vec::new();
            for item in &mutations {
                item.mutation.serialize_into(&mut old_data).expect("serialize failed");
            }

            let mut stream_cipher = StreamCipher::new(&unwrapped_key, 0);
            stream_cipher.encrypt(&mut old_data);

            let old_version = Version { major: 56, minor: 0 };
            let encrypted_mutations = EncryptedMutations {
                transactions: vec![(
                    JournalCheckpoint { file_offset: 0, checksum: 0, version: old_version },
                    mutations.len() as u64,
                )],
                data: old_data,
                mutations_key_roll: Vec::new(),
            };

            // Write `encrypted_mutations` to parent store using DirectWriter and attach to StoreInfo.
            let parent_store = store.parent_store().unwrap();
            let mut create_txn = parent_store
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let handle = ObjectStore::create_object(
                &parent_store,
                &mut create_txn,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("create_object failed");
            create_txn.commit().await.expect("commit failed");

            let mut writer = DirectWriter::new(&handle, Options::default()).await;
            let mut buffer = Vec::new();
            encrypted_mutations.serialize_with_version(&mut buffer).expect("serialize failed");
            writer.write_bytes(&buffer).await.expect("write_bytes failed");
            writer.complete().await.expect("writer complete failed");

            let mut store_info = store.load_store_info().await.unwrap();
            store_info.encrypted_mutations_object_id = handle.object_id();
            store_info.mutations_cipher_offset = 0;

            let mut update_txn = parent_store
                .new_transaction(
                    lock_keys![LockKey::object(
                        parent_store.store_object_id(),
                        store.store_info_handle_object_id().unwrap(),
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            store
                .write_store_info(&mut update_txn, &store_info)
                .await
                .expect("write_store_info failed");
            *store.store_info.lock() = Some(store_info);
            update_txn.commit().await.expect("commit failed");

            (oid, stream_cipher.offset())
        };

        // Roll the mutations key for AES picking up where ChaCha20 left off, and write a
        // transaction in the journal.
        let (new_wrapped_key, new_unwrapped_key) = crypt
            .create_key(store_object_id, KeyPurpose::Metadata)
            .await
            .expect("create_key failed");
        if let LockState::Unlocked { mutations_cipher, .. } = &mut *store.lock_state.lock() {
            *mutations_cipher = JournalCipher::new_aes256_xts(&new_unwrapped_key, chacha_offset);
        } else {
            panic!("Unexpected lock state");
        }
        store.store_info.lock().as_mut().unwrap().mutations_key = Some(new_wrapped_key);

        let new_file_oid = {
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(store_object_id, root_dir_id)],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let root_directory =
                Directory::open(&store, root_dir_id).await.expect("open root dir failed");
            let new_file = root_directory
                .create_child_file(&mut transaction, "new_file")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            new_file.object_id()
        };

        // Close and reopen the filesystem *without* flushing the child store.
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("FS open failed");

        // Mount/unlock the encrypted volume and verify that both files exist and can be opened.
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let volume = root_volume
            .volume("test", StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() })
            .await
            .expect("volume failed");

        let root_directory =
            Directory::open(&volume, root_dir_id).await.expect("open root dir failed");

        assert_eq!(
            root_directory.lookup("old_file").await.expect("lookup old_file failed"),
            Some((old_file_oid, ObjectDescriptor::File, false))
        );

        assert_eq!(
            root_directory.lookup("new_file").await.expect("lookup new_file failed"),
            Some((new_file_oid, ObjectDescriptor::File, false))
        );

        let old_file =
            ObjectStore::open_object(&volume, old_file_oid, HandleOptions::default(), None)
                .await
                .expect("open old_file failed");
        assert_eq!(old_file.object_id(), old_file_oid);

        let new_file =
            ObjectStore::open_object(&volume, new_file_oid, HandleOptions::default(), None)
                .await
                .expect("open new_file failed");
        assert_eq!(new_file.object_id(), new_file_oid);
    }

    #[test_case(false; "unencrypted")]
    #[test_case(true; "encrypted")]
    #[fuchsia::test]
    async fn test_large_mutations_in_transaction(encrypted: bool) {
        let fs = test_filesystem().await;
        let crypt = Arc::new(new_insecure_crypt());

        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_volume
            .new_volume(
                "test",
                NewChildStoreOptions {
                    options: StoreOptions {
                        crypt: if encrypted { Some(crypt.clone()) } else { None },
                        ..StoreOptions::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");
        let store_object_id = store.store_object_id();
        let root_dir_id = store.root_directory_object_id();

        let root_dir = Directory::open(&store, root_dir_id).await.expect("open root dir failed");
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store_object_id, root_dir_id)],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        let file = root_dir
            .create_child_file(&mut transaction, "test_file")
            .await
            .expect("create_child_file failed");

        // Add multiple inline extended attributes to the file, limiting each to
        // MAX_INLINE_XATTR_SIZE, until the total serialized size exceeds
        // DEFAULT_MAX_SERIALIZED_RECORD_SIZE.
        let mut total_size = 0;
        let mut i = 0;
        while total_size <= DEFAULT_MAX_SERIALIZED_RECORD_SIZE {
            let name = format!("attr{i}").into_bytes();
            let mutation = Mutation::replace_or_insert_object(
                ObjectKey::extended_attribute(file.object_id(), name),
                ObjectValue::inline_extended_attribute(vec![0u8; MAX_INLINE_XATTR_SIZE]),
            );
            let mut buf = Vec::new();
            mutation.serialize_into(&mut buf).unwrap();
            total_size += buf.len() as u64;
            transaction.add(store_object_id, mutation);
            i += 1;
        }

        transaction.commit().await.expect("commit should succeed");

        if encrypted {
            store.lock().await.expect("lock failed");
            store.unlock(crypt).await.expect("unlock failed");
        }

        for j in 0..i {
            let name = format!("attr{j}").into_bytes();
            let item = store
                .tree
                .find(&ObjectKey::extended_attribute(file.object_id(), name))
                .await
                .expect("find failed")
                .expect("attr not found");
            assert_eq!(
                item.value,
                ObjectValue::inline_extended_attribute(vec![0u8; MAX_INLINE_XATTR_SIZE])
            );
        }
    }
}
