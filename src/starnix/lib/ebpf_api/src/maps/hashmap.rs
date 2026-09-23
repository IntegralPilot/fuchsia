// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! eBPF hash map implementation.
//!
//! # Data layout
//! The following layout is used to store the data in the shared VMO:
//!
//! +--------+------------+------+
//! | header | hash table | data |
//! +--------+------------+------+
//!
//!  - Header stores the key used for the hash function and the free list. See
//!    `HashTableHeader` for details.
//!  - Hash table stores mapping between hash table mappings and linked lists
//!    of the items in the bucket. Format of each entry is defined by the
//!    `BucketHeader` struct. Number of buckets equals the `max_entries`
//!    specified in the schema
//!  - Data section stores the actual data. The entries are indexed from `0`
//!    to `max_entries - 1`. Each starts with `DataEntryHeader` followed by
//!    the key and the value. Both the key and the value are padded to
//!    multiple of 8 bytes.
//!
//! # Initial state
//! In the initial state, when the hash map is empty, all section of the map
//! are set to 0, except for the hash function key in the header.
//!
//! # Data entries
//! Every data entry can be in one of the following states:
//!  - Unused: Entries there were never used. These are all entries with
//!    indices greater or equal `num_used_entries` in `FreeListHeader. In the
//!    initial state all data entries are considered unused.
//!  - Free: These entries were previously used and were freed afterwards. They
//!    are stored in the free list.
//!  - Used: These entries are being used to store a key-value pair. Each of
//!    these entries are added to one linked list that corresponds to a hash
//!    table bucket that corresponds to the key.
//!  - Stale: These entries were previously used and they are still being
//!    referenced by a running eBPF program. They are added to the free list
//!    only after all references are released.
//!
//! Allowed state transitions:
//!   - Unused to Used: a new element is being added to the map, an unused
//!     entry is allocated to store the element.
//!   - Used to Free: an element is removed from the map, the corresponding
//!     entry is not referenced by a program (i.e. `ref_counter = 0`).
//!   - Free to Used: a new element is being added to the map, a free entry
//!     is allocated to store the element.
//!   - Used to Stale: an element is removed from the map, the corresponding
//!     entry is still referenced by a program (i.e. `ref_counter > 0`).
//!   - Stale to Free: a program drops the last reference to a Stale entry,
//!     i.e. reference count becomes 0.
//!
//! Note that when a new element is being added to the map it's preferable to
//! allocate a data entry from the free list instead of using an Unused entry.
//! This allows to save memory as keeping entries in Unused state for as long
//! as possible.
//!
//! # Linked lists
//! Free entries are stored in the free list. Used entries are stored in
//! the linked lists connected to hash table buckets. The lists are formed by
//! linking data entries using the `next` field in `DataEntryHeader` with the
//! list head linked either from `FreeListHeader.head` or from
//! `BucketHeader.head`. For these three fields `0` marks the end of the list,
//!  while `N + 1` references data entry at index `N`.
//!
//! # Synchronization
//! Access to each of the linked list is synchronized using read-write locks:
//! there is one lock for each hash table bucket, plus one lock for the free
//! list.
//!
//! # Data entry reference counting
//! Data entries may be referenced by a running eBPF program. If such entries
//! are removed concurrently by a different thread they should not be reused
//! while still being referenced by an eBPF program. To handle these cases
//! properly each data entry contains a reference counter. The counter is set
//! to 0 for all entries in Unused or Free states.

use super::buffer::{MapBuffer, VmoOrName};
use super::lock::RwMapLock;
use super::{MapError, MapImpl, MapKey, MapValueRef};
use ebpf::{EbpfBufferPtr, MapSchema};
use linux_uapi::{BPF_EXIST, BPF_NOEXIST};
use siphasher::sip::SipHasher;
use std::hash::Hasher;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Low-level types used in the `HashMap` implementation.
mod internal {
    use super::{MapBuffer, MapError, RwMapLock};
    use ebpf::{EbpfBufferPtr, MapSchema};
    use static_assertions::const_assert_eq;
    use std::mem::size_of;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    // Value stored in the shared VMO to reference a data entry by index. 0 is
    // equivalent to a null reference. Reference to entry at index N is stored
    // as N + 1. This allows to keep the whole VMO initialized to zeros after
    // creation.
    #[repr(C)]
    pub(super) struct EntryIndex(AtomicU32);

    impl EntryIndex {
        fn get(&self) -> Option<u32> {
            let v = self.0.load(Ordering::Relaxed);
            (v > 0).then(|| v - 1)
        }

        fn set(&self, value: Option<u32>) {
            let v = value.map(|index| index + 1).unwrap_or(0);
            self.0.store(v, Ordering::Relaxed);
        }
    }

    impl Default for EntryIndex {
        fn default() -> Self {
            Self(AtomicU32::new(0))
        }
    }

    // Struct used to store the free list, which contains the list of free data
    // entries.
    #[repr(C)]
    pub struct FreeListHeader {
        // Number of data entries that have been used. All entries with indices
        // greater than this value are considered unused.
        num_used_entries: AtomicU32,

        // The head of the linked list of entries that are available for reuse.
        head: EntryIndex,
    }

    // Hash table header stored in the VMO at offset 0.
    #[repr(C)]
    pub(super) struct HashTableHeader {
        pub hash_key: (AtomicU64, AtomicU64),

        // Lock that controls access to the free list.
        free_list_lock: AtomicU32,
        free_list_header: FreeListHeader,
    }

    const_assert_eq!(size_of::<HashTableHeader>(), 32);

    // 4 bytes added at the end to ensure 8-byte alignment for the hash table.
    const MAP_HEADER_SIZE: usize = 32;
    const_assert_eq!(MAP_HEADER_SIZE, size_of::<HashTableHeader>());
    const_assert_eq!(MAP_HEADER_SIZE % MapBuffer::ALIGNMENT, 0);

    // Contents of one entry in the hash table.
    #[repr(C)]
    struct BucketHeader {
        lock: AtomicU32,
        head: EntryIndex,
    }

    const BUCKET_SIZE: usize = 8;
    const_assert_eq!(BUCKET_SIZE, size_of::<BucketHeader>());
    const_assert_eq!(BUCKET_SIZE % MapBuffer::ALIGNMENT, 0);

    // Header of a data entry. Each data entry consists of the header followed by
    // the key and the value, both padded for 8-byte alignment.
    #[repr(C)]
    struct DataEntryHeader {
        next: EntryIndex,
        ref_count: AtomicU32,
    }

    const DATA_ENTRY_HEADER_SIZE: usize = 8;
    const_assert_eq!(DATA_ENTRY_HEADER_SIZE, size_of::<DataEntryHeader>());
    const_assert_eq!(DATA_ENTRY_HEADER_SIZE % MapBuffer::ALIGNMENT, 0);

    // Defines hash map layout in memory.
    #[derive(Debug, Copy, Clone)]
    pub(super) struct Layout {
        pub key_size: u32,
        pub value_size: u32,
        pub max_entries: u32,
    }

    impl Layout {
        pub fn new(schema: &MapSchema) -> Result<Self, MapError> {
            if schema.key_size == 0 || schema.max_entries == 0 {
                return Err(MapError::InvalidParam);
            }

            let result = Self {
                key_size: schema.key_size,
                value_size: schema.value_size,
                max_entries: schema.max_entries,
            };

            // Verify that all sizes are valid and we won't see any overflows.
            if result.checked_total_size().is_none() {
                return Err(MapError::InvalidParam);
            }

            Ok(result)
        }

        /// Returns the total size of the hash map in bytes, or None if it is too large.
        pub fn checked_total_size(&self) -> Option<usize> {
            MAP_HEADER_SIZE
                .checked_add(BUCKET_SIZE.checked_mul(self.num_buckets() as usize)?)?
                .checked_add(self.padded_data_entry_size().checked_mul(self.max_entries as usize)?)
        }

        pub fn num_buckets(&self) -> u32 {
            self.max_entries
        }

        fn bucket_offset(&self, index: u32) -> usize {
            // Cannot overflow because checked_total_size() didn't overflow in `new()`.
            MAP_HEADER_SIZE + BUCKET_SIZE * (index as usize)
        }

        fn padded_key_size(&self) -> usize {
            // Cannot overflow because key_size is 32-bit.
            MapBuffer::round_up_to_alignment(self.key_size as usize).unwrap()
        }

        fn padded_value_size(&self) -> usize {
            // Cannot overflow because value_size is 32-bit.
            MapBuffer::round_up_to_alignment(self.value_size as usize).unwrap()
        }

        fn data_entry_size(&self) -> usize {
            // Cannot overflow because key_size and value_size are 32-bit.
            DATA_ENTRY_HEADER_SIZE + self.padded_key_size() + self.value_size as usize
        }

        fn padded_data_entry_size(&self) -> usize {
            // Cannot overflow because key_size and value_size are 32-bit.
            DATA_ENTRY_HEADER_SIZE + self.padded_key_size() + self.padded_value_size()
        }

        fn data_entry_offset(&self, index: u32) -> usize {
            // Cannot overflow because `checked_total_size()` didn't overflow in `new()`.
            MAP_HEADER_SIZE
                + BUCKET_SIZE * (self.num_buckets() as usize)
                + self.padded_data_entry_size() * (index as usize)
        }

        pub fn total_size(&self) -> usize {
            // Cannot overflow because `checked_total_size()` didn't overflow in `new()`.
            self.checked_total_size().unwrap()
        }
    }

    pub(super) struct FreeListState<'a> {
        header: &'a FreeListHeader,
        store: HashMapStore<'a>,
    }

    impl<'a> FreeListState<'a> {
        fn new(store: HashMapStore<'a>) -> RwMapLock<'a, Self> {
            const LOCK_SIGNAL: zx::Signals = zx::Signals::USER_0;

            // SAFETY: `RwMapLock` wraps `FreeListState` to ensure access to
            // the free list is synchronized.
            unsafe {
                let hash_table_header =
                    store.buf.ptr().get_ptr::<HashTableHeader>(0).unwrap().deref();
                RwMapLock::new(
                    &hash_table_header.free_list_lock,
                    store.buf.vmo().as_handle_ref(),
                    LOCK_SIGNAL,
                    FreeListState { header: &hash_table_header.free_list_header, store },
                )
            }
        }

        pub fn alloc(&mut self) -> Option<HashMapEntryRef<'a>> {
            let entry = if let Some(index) = self.header.head.get() {
                // Pop an entry from the head of the free list if it's not empty.
                //
                // SAFETY: The free list is locked, so it's safe to access the
                // free list head.
                let entry = unsafe { self.store.data_entry(index) };
                self.header.head.set(entry.next());
                entry
            } else if let num_used_entries = self.header.num_used_entries.load(Ordering::Relaxed)
                && num_used_entries < self.store.layout.max_entries
            {
                // Allocate an unused entry if any.
                let index = num_used_entries;
                self.header.num_used_entries.store(num_used_entries + 1, Ordering::Relaxed);
                // SAFETY: The entry being allocated is marked as unused, which
                // means other threads should not be using it.
                unsafe { self.store.data_entry(index) }
            } else {
                // The map is full.
                return None;
            };

            // By returning `HashMapEntryRef` we ensure that the ref-counter
            // will decremented, unless the entry is inserted to a bucket.
            #[allow(
                clippy::undocumented_unsafe_blocks,
                reason = "Force documented unsafe blocks in Starnix"
            )]
            let ref_count = unsafe { entry.ref_count().fetch_add(1, Ordering::Relaxed) };
            assert!(ref_count == 0);

            Some(HashMapEntryRef { store: self.store.clone(), index: Some(entry.index) })
        }

        fn push(&mut self, mut entry: DataEntry<'a>) {
            assert!(entry.get_ref_count() == 0);

            // Insert the entry to the head of the free list.
            //
            // SAFETY: The `ref_count` is 0, which guarantees that the entry is
            // not in a hash table bucket.
            unsafe { entry.set_next(self.header.head.get()) };
            self.header.head.set(Some(entry.index));
        }
    }

    #[derive(Clone)]
    pub(super) struct HashMapStore<'a> {
        buf: &'a MapBuffer,
        layout: Layout,
    }

    impl<'a> HashMapStore<'a> {
        pub fn new(buf: &'a MapBuffer, layout: Layout) -> Self {
            HashMapStore { buf, layout }
        }

        pub fn free_list(&self) -> RwMapLock<'a, FreeListState<'a>> {
            FreeListState::new(self.clone())
        }

        /// # Safety
        /// Caller must ensure that if the returned entry is not being used
        /// by other threads, except for the thread-safe methods: `value()`
        /// and `[get_]ref_count()`.
        unsafe fn data_entry<'b>(&self, index: u32) -> DataEntry<'b>
        where
            'a: 'b,
        {
            assert!(index < self.layout.max_entries);
            let pos = self.layout.data_entry_offset(index);
            let entry_size = self.layout.data_entry_size();
            DataEntry {
                buf: self.buf.ptr().slice(pos..(pos + entry_size)).unwrap(),
                key_size: self.layout.key_size,
                index,
            }
        }

        /// Returns the next element in the bucket after `entry`.
        pub fn next<'b>(&self, entry: &DataEntry<'b>) -> Option<DataEntry<'b>>
        where
            'a: 'b,
        {
            // SAFETY: Caller has access to the `entry`, which implies that
            // the bucket is locked and it's safe to access the next entry in
            // the list.
            unsafe { Some(self.data_entry(entry.next()?)) }
        }

        /// Returns a ref-counted reference to the `entry`.
        pub fn get_value_reference<'b>(&self, entry: &DataEntry<'b>) -> HashMapEntryRef<'a> {
            // SAFETY: ref-counter will be decremented when the returned
            // `HashMapEntryRef` is dropped.
            unsafe { entry.ref_count().fetch_add(1, Ordering::Relaxed) };
            HashMapEntryRef { index: Some(entry.index), store: self.clone() }
        }
    }

    #[derive(Clone)]
    pub(super) struct DataEntry<'a> {
        buf: EbpfBufferPtr<'a>,
        key_size: u32,
        index: u32,
    }

    impl<'a> DataEntry<'a> {
        /// Index of the next element in the linked list.
        fn next(&self) -> Option<u32> {
            // SAFETY: It's safe to read the value because the current thread is
            // holding a lock for the bucker that contains the entry.
            unsafe { self.buf.get_ptr::<DataEntryHeader>(0).unwrap().deref().next.get() }
        }

        /// Reference counted for the entry
        ///
        /// # Safety
        /// The caller must increment/decrement the counter when new references
        /// to this entry are acquired/released. Unused entry should be
        /// returned to the free list.
        unsafe fn ref_count(&self) -> &AtomicU32 {
            #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
            unsafe {
                &self.buf.get_ptr::<DataEntryHeader>(0).unwrap().deref().ref_count
            }
        }

        fn get_ref_count(&self) -> u32 {
            // SAFETY: Loading ref-count is always safe.
            unsafe { self.ref_count().load(Ordering::Relaxed) }
        }

        /// Sets the `next` field that points to the next element in the linked
        /// list.
        ///
        /// # Safety
        /// Caller must ensure linked list consistency, i.e. the entry is in
        /// only one list, there are no cycles, etc.
        unsafe fn set_next(&mut self, next: Option<u32>) {
            #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
            unsafe {
                self.buf.get_ptr::<DataEntryHeader>(0).unwrap().deref().next.set(next)
            }
        }

        pub fn key(&self) -> EbpfBufferPtr<'a> {
            let key_pos = DATA_ENTRY_HEADER_SIZE;
            self.buf.slice(key_pos..(key_pos + self.key_size as usize)).unwrap()
        }

        /// Returns the buffer pointing at the element value.
        pub fn value(&self) -> EbpfBufferPtr<'a> {
            let value_pos = DATA_ENTRY_HEADER_SIZE
                + MapBuffer::round_up_to_alignment(self.key_size as usize).unwrap();
            self.buf.slice(value_pos..self.buf.len()).unwrap()
        }
    }

    // A ref-counted reference to a data entry.
    pub struct HashMapEntryRef<'a> {
        store: HashMapStore<'a>,
        index: Option<u32>,
    }

    impl<'a> HashMapEntryRef<'a> {
        pub fn is_only_reference(&self) -> bool {
            let Some(index) = self.index else {
                return false;
            };
            // SAFETY: The entry is only used to get the ref-count.
            let entry = unsafe { self.store.data_entry(index) };
            return entry.get_ref_count() == 1;
        }

        pub fn ptr(&self) -> EbpfBufferPtr<'a> {
            // SAFETY: The value stored in the data entry is safe to access as
            // long as we have a strong reference.
            let entry = unsafe { self.store.data_entry(self.index.unwrap()) };
            entry.value()
        }
    }

    impl<'a> Drop for HashMapEntryRef<'a> {
        fn drop(&mut self) {
            let Some(index) = self.index else {
                return;
            };
            // SAFETY: Get the data entry to decrement the ref-counter. This
            // is safe to do without acquiring the lock. The entry is pushed to
            // the free list if this was the last reference.
            unsafe {
                let entry = self.store.data_entry(index);
                let ref_count = entry.ref_count().fetch_sub(1, Ordering::Relaxed);
                if ref_count == 1 {
                    self.store.free_list().write().push(entry);
                }
            }
        }
    }

    pub(super) struct BucketState<'a> {
        head: &'a EntryIndex,
        store: HashMapStore<'a>,
    }

    impl<'a> BucketState<'a> {
        pub fn new(store: HashMapStore<'a>, index: u32) -> RwMapLock<'a, Self> {
            assert!(index < store.layout.num_buckets());

            const BUCKET_LOCK_SIGNALS: [zx::Signals; 8] = [
                zx::Signals::USER_0,
                zx::Signals::USER_1,
                zx::Signals::USER_2,
                zx::Signals::USER_3,
                zx::Signals::USER_4,
                zx::Signals::USER_5,
                zx::Signals::USER_6,
                zx::Signals::USER_7,
            ];

            // SAFETY: Returned `RwMapLock` uses the lock stored in the bucket
            // header to synchronized access access to the `BucketState`.
            unsafe {
                let offset = store.layout.bucket_offset(index);
                let header = store.buf.ptr().get_ptr::<BucketHeader>(offset).unwrap().deref();
                let lock_cell = &header.lock;
                RwMapLock::new(
                    lock_cell,
                    store.buf.vmo().as_handle_ref(),
                    BUCKET_LOCK_SIGNALS[(index as usize) % BUCKET_LOCK_SIGNALS.len()],
                    BucketState { head: &header.head, store },
                )
            }
        }

        /// Index of the first element in the bucket.
        pub fn head<'b>(&'b self) -> Option<DataEntry<'b>> {
            let index = self.head.get()?;
            // SAFETY: `head()` can be called only when the bucket is locked.
            Some(unsafe { self.store.data_entry(index) })
        }

        // Finds the specified key in the bucket. If found returns pair of indices:
        //  - Index of the entry that contains key.
        //  - Index of the previous entry in the linked list, if any.
        fn find_internal(&self, key: &[u8]) -> Option<(u32, Option<u32>)> {
            let mut previous = None;
            let mut current = self.head()?;
            loop {
                if current.key().eq_slice(key) {
                    return Some((current.index, previous));
                }
                previous = Some(current.index);
                current = self.store.next(&current)?;
            }
        }

        pub fn find<'b>(&'b self, key: &[u8]) -> Option<DataEntry<'b>> {
            let (index, _) = self.find_internal(key)?;
            // SAFETY: The entry belongs to the current bucket, which is
            // currently locked and will remain locked for the lifetime of the
            // result.
            Some(unsafe { self.store.data_entry(index) })
        }

        pub fn remove(&mut self, key: &[u8]) -> Option<HashMapEntryRef<'a>> {
            let Some((found, previous)) = self.find_internal(key) else { return None };

            // Remove the found entry from the bucket by unlinking it from the
            // linked list.
            //
            // SAFETY: The bucket is locked. The removed entry is wrapped into
            // `HashMapEntryRef`, which ensures that the ref-counter will be
            // decremented and returned to the free list.
            unsafe {
                let entry = self.store.data_entry(found);
                match previous {
                    None => self.head.set(entry.next()),
                    Some(previous) => {
                        let mut prev_entry = self.store.data_entry(previous);
                        prev_entry.set_next(entry.next());
                    }
                };
                Some(HashMapEntryRef { store: self.store.clone(), index: Some(entry.index) })
            }
        }

        pub fn insert<'b>(
            &'b mut self,
            mut entry_ref: HashMapEntryRef<'a>,
            key: &[u8],
        ) -> DataEntry<'b> {
            assert!(entry_ref.is_only_reference());
            let index = entry_ref.index.take().expect("insert called for a null EntryRef");

            // SAFETY: The entry is returned below after being inserted to the
            // list below. It's lifetime is tied to `self` which guarantees
            // that the bucket lock is held when it's being used.
            let mut entry = unsafe { self.store.data_entry(index) };

            entry.key().store(key);
            // SAFETY: The entry is not in any list (we just allocated it or
            // removed it from bucket and checked it is the only reference).
            // We are inserting it to the head of the bucket.
            unsafe {
                entry.set_next(self.head.get());
            }

            self.head.set(Some(index));

            entry
        }
    }
}

pub(super) use internal::HashMapEntryRef;
use internal::{BucketState, HashMapStore, HashTableHeader, Layout};

#[derive(Debug)]
pub struct HashMap {
    buffer: MapBuffer,
    layout: Layout,
}

impl HashMap {
    pub fn new(schema: &MapSchema, vmo: impl Into<VmoOrName>) -> Result<Self, MapError> {
        let layout = Layout::new(schema)?;
        let buffer =
            MapBuffer::new(layout.total_size(), vmo.into().with_name_prefix("ebpf:hashmap"))?;

        // Generate a random key to be used for the hash function and store it
        // in the buffer.
        // SAFETY: The buffer is not shared at this point, so it's safe to
        // access it directly.
        let header = unsafe { buffer.ptr().get_ptr::<HashTableHeader>(0).unwrap().deref() };
        header.hash_key.0.store(rand::random(), Ordering::Relaxed);
        header.hash_key.1.store(rand::random(), Ordering::Relaxed);

        Ok(HashMap { buffer, layout })
    }

    fn hash_key<'a>(&'a self) -> (u64, u64) {
        // SAFETY: The hash key never changes, it's safe to access it directly.
        let header = unsafe { self.buffer.ptr().get_ptr::<HashTableHeader>(0).unwrap().deref() };
        (header.hash_key.0.load(Ordering::Relaxed), header.hash_key.1.load(Ordering::Relaxed))
    }

    fn bucket<'a>(&'a self, index: u32) -> RwMapLock<'a, BucketState<'a>> {
        BucketState::new(self.store(), index)
    }

    fn store<'a>(&'a self) -> HashMapStore<'a> {
        HashMapStore::new(&self.buffer, self.layout)
    }

    fn get_bucket_index_for_key<'a>(&'a self, key: &'_ [u8]) -> u32 {
        let (k1, k2) = self.hash_key();
        let mut hasher = SipHasher::new_with_keys(k1, k2);
        hasher.write(key);
        let hash = hasher.finish();

        // Fold the hash value into 32 bit.
        let hash32 = (hash >> 32) ^ (hash & 0xffff_ffff);

        ((hash32 * (self.layout.num_buckets() as u64)) >> 32) as u32
    }

    fn get_bucket_for_key<'a>(&'a self, key: &'_ [u8]) -> RwMapLock<'a, BucketState<'a>> {
        self.bucket(self.get_bucket_index_for_key(key))
    }
}

impl MapImpl for HashMap {
    fn lookup<'a>(&'a self, key: &[u8]) -> Option<MapValueRef<'a>> {
        let bucket = self.get_bucket_for_key(&key).read();
        let entry = bucket.find(&key)?;
        let value_ref = self.store().get_value_reference(&entry);
        Some(MapValueRef::new_from_hashmap(value_ref))
    }

    fn update(&self, key: &[u8], value: EbpfBufferPtr<'_>, flags: u64) -> Result<(), MapError> {
        let mut bucket = self.get_bucket_for_key(key).write();

        let mut entry_to_reuse = None;

        if flags & (BPF_NOEXIST as u64) != 0 {
            if bucket.find(key).is_some() {
                return Err(MapError::EntryExists);
            }
        } else {
            // `update()` must be atomic. The old entry is removed from the
            // bucket and replaced with a new one. The old entry is reused if
            // possible, i.e. if it's not being used by other threads.
            match bucket.remove(key) {
                Some(entry_ref) => {
                    if entry_ref.is_only_reference() {
                        entry_to_reuse = Some(entry_ref);
                    }
                }
                None => {
                    if flags & (BPF_EXIST as u64) != 0 {
                        return Err(MapError::InvalidKey);
                    }
                }
            }
        };

        // Allocate a new entry if we can't reuse the old one.
        let free_entry = match entry_to_reuse {
            Some(e) => e,
            None => self.store().free_list().write().alloc().ok_or(MapError::SizeLimit)?,
        };

        let entry = bucket.insert(free_entry, key);
        entry.value().copy(&value);

        Ok(())
    }

    fn delete(&self, key: &[u8]) -> Result<(), MapError> {
        let mut bucket = self.get_bucket_for_key(&key).write();
        match bucket.remove(&key) {
            Some(_) => Ok(()),
            None => Err(MapError::InvalidKey),
        }
    }

    fn get_next_key(&self, key: Option<&[u8]>) -> Result<MapKey, MapError> {
        let next_bucket = match key {
            Some(key) => {
                // First check if we have another item in the same bucket.
                let bucket_index = self.get_bucket_index_for_key(&key);
                let bucket = self.bucket(bucket_index).read();
                let entry = bucket.find(&key).ok_or(MapError::InvalidKey)?;
                if let Some(next_entry) = self.store().next(&entry) {
                    return Ok(next_entry.key().load::<16>());
                }
                bucket_index + 1
            }
            None => 0,
        };

        // Iterate through the remaining buckets to find the next non-empty
        // bucket.
        for bucket_index in next_bucket..self.layout.num_buckets() {
            let bucket = self.bucket(bucket_index).read();
            if let Some(entry) = bucket.head() {
                return Ok(entry.key().load::<16>());
            }
        }

        Err(MapError::InvalidKey)
    }

    fn vmo(&self) -> &Arc<zx::Vmo> {
        self.buffer.vmo()
    }
}
