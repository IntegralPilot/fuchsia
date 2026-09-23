// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! eBPF LPM Trie map implementation. See
//! [eBPF docs](https://docs.ebpf.io/linux/map-type/BPF_MAP_TYPE_LPM_TRIE/)
//! for semantics of LPM tries.
//!
//! # Data layout
//! The following layout is used to store the data in the shared VMO:
//!
//! +--------+-------+------+
//! | HEADER | NODES | HEAP |
//! +--------+-------+------+
//!
//!  - HEADER stores the following bits of information:
//!     - 2 locks for the two following sections,
//!     - Information about free & used entries for the two following sections,
//!     - Reference to the root of the trie.
//!  - NODES section stores the nodes that form the trie.
//!  - HEAP section stores the data values. HEAP entries are referenced by the
//!    corresponding entries in the NODEs section.
//!
//! # Trie storage
//! The trie nodes are stored in the NODES section of the VMO described above,
//! which is a table of `max_entries * 2` nodes. Nodes can be identified by
//! their index. Each entry in the table stores node header followed by the
//! corresponding key. A node can be in one for the 3 states:
//!
//!  - Unused: Nodes that have never been used. This is the initial state for
//!    all nodes. All enries with the index not less than
//!    `LpmTrieStateHeader::num_used_nodes` are considered unused.
//!  - Used: These nodes are part for the trie.
//!  - Free: Nodes that were used previously used an were released. These nodes
//!    are stored in a linked list defined by the `free_nodes_head` field of the
//!    header.
//!
//! Used nodes form a tree by storing `left` and `right` fields in the node
//! header.
//!
//! # Heap
//! All map values referenced by the Trie are stored in the heap. The heap
//! is a table of `max_entries` elements, each contains a header following by a
//! map value. Heap entries are ref-counted. They can be referenced either by
//! a node in the trie or by an eBPF program while it's being executed.
//! Lifetime of the data allocated from the heap is managed in a manner similar
//! to how eBPF hash maps implementation manages data entries. See [`HashMap`]
//! documentation for details.
//!
//! # Synchronization
//! Access to an LPM tries is synchronized used two locks stored in the trie
//! header. One lock is used for the trie itself, while the second lock
//! controls access to the heap.

use super::buffer::{MapBuffer, VmoOrName};
use super::{MapError, MapImpl, MapKey, MapValueRef};
use ebpf::{EbpfBufferPtr, MapFlags, MapSchema};
use linux_uapi::{BPF_EXIST, BPF_NOEXIST};
use std::sync::Arc;
use zerocopy::{FromBytes, IntoBytes};

fn num_matching_bits(a: u32, b: u32) -> usize {
    let a = u32::from_be(a);
    let b = u32::from_be(b);
    (a ^ b).leading_zeros() as usize
}

// Trait for a buffer that represents a key in an LPM trie.
trait LpmKey {
    // Reads 32 bits of the key, starting from the first element (i.e.
    // `read_u32(0)` returns the key length).
    fn read_u32(&self, index: usize) -> u32;

    // Key length.
    fn key_len(&self) -> usize {
        self.read_u32(0) as usize
    }

    // Returns single bit at the specified `index`.
    fn get_bit(&self, index: usize) -> Option<bool> {
        if index >= self.key_len() {
            return None;
        }
        let element = u32::from_be(self.read_u32(1 + index / 32));
        Some(element & (1 << (31 - index % 32)) > 0)
    }

    // Returns number of bits that match between `self` and `key`.
    fn num_matching_bits(&self, key: impl LpmKey) -> usize {
        let mut result = 0;
        let len = std::cmp::min(self.key_len(), key.key_len());
        for i in 0..(len / 32) {
            let a = self.read_u32(i + 1);
            let b = key.read_u32(i + 1);
            if a == b {
                result += 32;
            } else {
                result += num_matching_bits(a, b);
                return result;
            }
        }
        let suffix = len % 32;
        if suffix > 0 {
            let a = self.read_u32(len / 32 + 1);
            let b = key.read_u32(len / 32 + 1);
            if a == b {
                result += suffix;
            } else {
                result += std::cmp::min(suffix, num_matching_bits(a, b));
            }
        }
        result
    }

    // Returns true if `self` is a prefix of they `key`.
    fn is_prefix_of(&self, key: impl LpmKey) -> bool {
        self.num_matching_bits(key) == self.key_len()
    }
}

impl LpmKey for &[u8] {
    fn read_u32(&self, index: usize) -> u32 {
        let pos = index * std::mem::size_of::<u32>();
        assert!(pos < self.len());
        let end = pos + std::mem::size_of::<u32>();
        if end <= self.len() {
            u32::read_from_bytes(&self[pos..end]).unwrap()
        } else {
            // `self` may not contain a whole number of `u32` values. The last
            // value is padded with 0s.
            let len = self.len() - pos;
            let mut value: u32 = 0;
            value.as_mut_bytes()[..len].copy_from_slice(&self[pos..]);
            value
        }
    }
}

impl<'a> LpmKey for EbpfBufferPtr<'a> {
    fn read_u32(&self, index: usize) -> u32 {
        self.get_ptr::<u32>(index * std::mem::size_of::<u32>()).unwrap().load_relaxed()
    }
}

mod internal {
    use super::LpmKey;
    use crate::maps::MapError;
    use crate::maps::buffer::MapBuffer;
    use crate::maps::lock::RwMapLock;
    use ebpf::{EbpfBufferPtr, MapSchema};
    use static_assertions::{const_assert, const_assert_eq};
    use std::sync::atomic::{AtomicU32, Ordering};

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

        fn is_some(&self) -> bool {
            self.get().is_some()
        }

        fn is_none(&self) -> bool {
            self.get().is_none()
        }

        fn set(&self, value: Option<u32>) {
            let v = value.map(|index| index + 1).unwrap_or(0);
            self.0.store(v, Ordering::Relaxed);
        }

        fn take(&self) -> Option<u32> {
            let v = self.0.swap(0, Ordering::Relaxed);
            (v > 0).then(|| v - 1)
        }
    }

    #[repr(C)]
    pub(super) struct LpmTrieStateHeader {
        // Index of the root node, if any.
        root: EntryIndex,

        // Number of used node entries.
        num_used_nodes: AtomicU32,

        // The head of the linked list of the node entries that are available
        // for reuse.
        free_nodes_head: EntryIndex,
    }

    // Struct used to store state of the heap.
    #[repr(C)]
    pub struct HeapHeader {
        // Number of used data entries. All entries with indices greater than
        // this value are considered unused.
        num_used_entries: AtomicU32,

        // The head of the linked list of entries that are available for reuse.
        free_list_head: EntryIndex,
    }

    // Hash table header stored in the VMO at offset 0.
    #[repr(C)]
    pub(super) struct LpmTrieHeader {
        // Lock that controls access to the stack.
        lock: AtomicU32,
        state: LpmTrieStateHeader,

        // Lock that controls access to the heap.
        heap_lock: AtomicU32,
        heap_header: HeapHeader,
    }

    // 4 bytes added at the end to ensure 8-byte alignment for the hash table.
    const TRIE_HEADER_SIZE: usize = 32;
    const_assert!(TRIE_HEADER_SIZE >= size_of::<LpmTrieHeader>());
    const_assert_eq!(TRIE_HEADER_SIZE % MapBuffer::ALIGNMENT, 0);

    // Header of a node. In memory each node entry consists of the header
    // followed by the key, both padded for 8-byte alignment.
    #[repr(C)]
    struct NodeHeader {
        left: EntryIndex,
        right: EntryIndex,
        value: EntryIndex,
    }

    const NODE_HEADER_SIZE: usize = 16;
    const_assert!(NODE_HEADER_SIZE >= size_of::<DataEntryHeader>());
    const_assert_eq!(NODE_HEADER_SIZE % MapBuffer::ALIGNMENT, 0);

    // Header of a data entry. Each data entry consists of the header followed by
    // the value, both padded for 8-byte alignment.
    #[repr(C)]
    struct DataEntryHeader {
        ref_count: AtomicU32,

        // Next element in the free list. Used only when the entry in the free
        // list.
        next_free: EntryIndex,
    }

    const DATA_ENTRY_HEADER_SIZE: usize = 8;
    const_assert_eq!(DATA_ENTRY_HEADER_SIZE, size_of::<DataEntryHeader>());
    const_assert_eq!(DATA_ENTRY_HEADER_SIZE % MapBuffer::ALIGNMENT, 0);

    // Defines layout of an LPM trie in memory.
    #[derive(Debug, Copy, Clone)]
    pub(super) struct Layout {
        pub key_size: u32,
        pub value_size: u32,
        pub max_entries: u32,
    }

    impl Layout {
        const LPM_KEY_PREFIX_SIZE: u32 = 4;

        // LPM Tries allow up to 2048 bits prefix, so total key is up to 260 bytes.
        const LPM_MAX_KEY_LENGTH_BYTES: u32 = Self::LPM_KEY_PREFIX_SIZE + 256;

        pub fn new(schema: &MapSchema) -> Result<Self, MapError> {
            // Key must be at least 5 bytes: 4 bytes for the prefix, and 1 byte for the key itself.
            if schema.key_size < (Self::LPM_KEY_PREFIX_SIZE + 1)
                || schema.key_size > Self::LPM_MAX_KEY_LENGTH_BYTES
                || schema.max_entries == 0
            {
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

        /// Returns the total size of the LPM trie in bytes, or None if it is too large.
        pub fn checked_total_size(&self) -> Option<usize> {
            TRIE_HEADER_SIZE
                .checked_add(self.padded_node_size().checked_mul(self.num_nodes())?)?
                .checked_add(self.padded_data_entry_size().checked_mul(self.max_entries as usize)?)
        }

        fn padded_key_size(&self) -> usize {
            MapBuffer::round_up_to_alignment(self.key_size as usize).unwrap()
        }

        fn padded_value_size(&self) -> usize {
            MapBuffer::round_up_to_alignment(self.value_size as usize).unwrap()
        }

        fn num_nodes(&self) -> usize {
            // Number of allocated nodes is the double of the number of entries
            // to provide space for non-leaf nodes.
            (self.max_entries as usize) * 2
        }

        fn node_size(&self) -> usize {
            NODE_HEADER_SIZE + self.key_size as usize
        }

        fn padded_node_size(&self) -> usize {
            NODE_HEADER_SIZE + self.padded_key_size()
        }

        fn node_offset(&self, index: u32) -> usize {
            TRIE_HEADER_SIZE + self.padded_node_size() * (index as usize)
        }

        fn data_entry_size(&self) -> usize {
            DATA_ENTRY_HEADER_SIZE + self.value_size as usize
        }

        fn padded_data_entry_size(&self) -> usize {
            DATA_ENTRY_HEADER_SIZE + self.padded_value_size()
        }

        fn data_entry_offset(&self, index: u32) -> usize {
            TRIE_HEADER_SIZE
                + self.padded_node_size() * self.num_nodes()
                + self.padded_data_entry_size() * (index as usize)
        }

        pub fn total_size(&self) -> usize {
            self.checked_total_size().unwrap()
        }
    }

    pub(super) struct HeapState<'a> {
        header: &'a HeapHeader,
        store: LpmTrieStore<'a>,
    }

    impl<'a> HeapState<'a> {
        fn new(store: LpmTrieStore<'a>) -> RwMapLock<'a, Self> {
            const LOCK_SIGNAL: zx::Signals = zx::Signals::USER_0;

            // SAFETY: `RwMapLock` wraps `HeapState` to ensure access to
            // the heap is synchronized.
            unsafe {
                let lpm_trie_header = store.buf.ptr().get_ptr::<LpmTrieHeader>(0).unwrap().deref();
                RwMapLock::new(
                    &lpm_trie_header.heap_lock,
                    store.buf.vmo().as_handle_ref(),
                    LOCK_SIGNAL,
                    HeapState { header: &lpm_trie_header.heap_header, store },
                )
            }
        }

        pub fn alloc(&mut self) -> Option<LpmTrieEntryRef<'a>> {
            let entry = if let Some(index) = self.header.free_list_head.get() {
                // Pop an entry from the head of the free list if it's not
                // empty.
                let entry = self.store.data_entry(index);
                let next_free = entry.header().next_free.get();
                self.header.free_list_head.set(next_free);
                entry
            } else {
                // Allocate an unused entry if any.
                let index = self.header.num_used_entries.fetch_add(1, Ordering::Relaxed);
                if index >= self.store.layout.max_entries {
                    // The heap is full.
                    self.header.num_used_entries.fetch_sub(1, Ordering::Relaxed);
                    return None;
                }

                self.store.data_entry(index)
            };

            let ref_count = entry.ref_count().fetch_add(1, Ordering::Relaxed);
            assert!(ref_count == 0);

            Some(LpmTrieEntryRef { store: self.store.clone(), index: Some(entry.index) })
        }

        fn release(&mut self, mut entry: DataEntry<'a>) {
            assert!(entry.get_ref_count() == 0);

            // Insert the entry to the head of the free list.
            entry.set_next(self.header.free_list_head.get());
            self.header.free_list_head.set(Some(entry.index).into());
        }
    }

    #[derive(Clone)]
    pub(super) struct LpmTrieStore<'a> {
        buf: &'a MapBuffer,
        layout: Layout,
    }

    impl<'a> LpmTrieStore<'a> {
        pub fn new(buf: &'a MapBuffer, layout: Layout) -> Self {
            Self { buf, layout }
        }

        pub fn trie(&self) -> RwMapLock<'a, LpmTrieState<'a>> {
            LpmTrieState::new(self.clone())
        }

        pub fn heap(&self) -> RwMapLock<'a, HeapState<'a>> {
            HeapState::new(self.clone())
        }

        fn data_entry<'b>(&self, index: u32) -> DataEntry<'b>
        where
            'a: 'b,
        {
            assert!(index < self.layout.max_entries);
            let pos = self.layout.data_entry_offset(index);
            let entry_size = self.layout.data_entry_size();
            DataEntry { buf: self.buf.ptr().slice(pos..(pos + entry_size)).unwrap(), index }
        }
    }

    // A reference to a single node of the trie.
    #[derive(Clone)]
    pub(super) struct LpmTrieNode<'a> {
        index: u32,
        header: &'a NodeHeader,
        key: EbpfBufferPtr<'a>,
    }

    impl<'a> LpmTrieNode<'a> {
        fn new(index: u32, buffer: EbpfBufferPtr<'a>) -> Self {
            // SAFETY: `NodeHeader` contains only `AtomicU32` fields, so
            // dereferencing it without synchronization is safe.
            let header = unsafe { buffer.get_ptr::<NodeHeader>(0).unwrap().deref() };
            Self { index, header, key: buffer.slice(NODE_HEADER_SIZE..).unwrap() }
        }

        pub fn index(&self) -> u32 {
            self.index
        }

        pub fn key(&self) -> EbpfBufferPtr<'a> {
            self.key.clone()
        }

        // Updates node key to the specified value. If `len` is specified then
        // the key is truncated to that value.
        pub fn set_key(&self, key: &[u8], len: Option<usize>) {
            self.key.store(key);
            if let Some(len) = len {
                self.key.get_ptr::<u32>(0).unwrap().store_relaxed(len as u32);
            }
        }

        pub fn has_value(&self) -> bool {
            self.header.value.is_some()
        }

        // Sets the node value. Old value must be released first.
        pub fn set_value(&self, value: LpmTrieEntryRef<'a>) {
            assert!(self.header.value.is_none());
            self.header.value.set(Some(value.take_index()));
        }

        // Updates child node from `prev` to `new`.
        pub fn update_child(&self, new: Option<&LpmTrieNode<'a>>, prev: Option<&LpmTrieNode<'a>>) {
            let next_bit_pos = self.key.key_len();
            let next_bit = match (new, prev) {
                (Some(new), Some(prev)) => {
                    let bit = new.key.get_bit(next_bit_pos).unwrap();
                    assert!(bit == prev.key.get_bit(next_bit_pos).unwrap());
                    bit
                }
                (Some(new), None) => new.key.get_bit(next_bit_pos).unwrap(),
                (None, Some(prev)) => prev.key.get_bit(next_bit_pos).unwrap(),
                (None, None) => unreachable!(),
            };
            let branch = match next_bit {
                false => &self.header.left,
                true => &self.header.right,
            };

            let prev_index = prev.map(|node| node.index());
            assert!(branch.get() == prev_index);

            let new_index = new.map(|node| node.index());
            branch.set(new_index);
        }

        pub fn left(&self) -> Option<u32> {
            self.header.left.get()
        }

        pub fn right(&self) -> Option<u32> {
            self.header.right.get()
        }
    }

    // Provides access to the trie nodes. Allows to allocate new nodes and
    // release unused nodes. This type is not responsible for
    // consistency of the trie.
    pub(super) struct LpmTrieState<'a> {
        header: &'a LpmTrieStateHeader,
        store: LpmTrieStore<'a>,
    }

    impl<'a> LpmTrieState<'a> {
        pub fn new(store: LpmTrieStore<'a>) -> RwMapLock<'a, Self> {
            const LOCK_SIGNAL: zx::Signals = zx::Signals::USER_0;

            // SAFETY: `RwMapLock` wraps `LpmTrieState` to ensure access to
            // the free list is synchronized.
            unsafe {
                let lpm_trie_header = store.buf.ptr().get_ptr::<LpmTrieHeader>(0).unwrap().deref();
                RwMapLock::new(
                    &lpm_trie_header.lock,
                    store.buf.vmo().as_handle_ref(),
                    LOCK_SIGNAL,
                    LpmTrieState { header: &lpm_trie_header.state, store },
                )
            }
        }

        // Returns `LpmTrieNode` for the node at the specified index.
        pub fn node(&self, index: u32) -> LpmTrieNode<'a> {
            let offset = self.store.layout.node_offset(index);
            let size = self.store.layout.node_size();
            LpmTrieNode::new(
                index.into(),
                self.store.buf.ptr().slice(offset..(offset + size)).unwrap(),
            )
        }

        // Returns root node of the trie, if any.
        pub fn root(&self) -> Option<LpmTrieNode<'_>> {
            let index = self.header.root.get()?;
            Some(self.node(index))
        }

        // Updates the root node from `prev` to `new`.
        pub fn update_root(&self, new: Option<&LpmTrieNode<'a>>, prev: Option<&LpmTrieNode<'a>>) {
            let prev_index = prev.map(|node| node.index());
            assert!(self.header.root.get() == prev_index);

            let new_index = new.map(|node| node.index());
            self.header.root.set(new_index);
        }

        pub fn value(&self, node: &LpmTrieNode<'_>) -> Option<LpmTrieEntryRef<'a>> {
            self.store
                .data_entry(node.header.value.get()?)
                .ref_count()
                .fetch_add(1, Ordering::Relaxed);
            let data_entry_index = node.header.value.get()?;
            Some(LpmTrieEntryRef { index: Some(data_entry_index), store: self.store.clone() })
        }

        // Returns the value stored in the node and resets it to None.
        pub fn take_value(&self, node: &LpmTrieNode<'_>) -> Option<LpmTrieEntryRef<'a>> {
            let index = node.header.value.get()?;
            node.header.value.set(None);

            // No need to update the ref counter.
            Some(LpmTrieEntryRef { index: Some(index), store: self.store.clone() })
        }

        // Allocates a new node.
        pub fn allocate_node(&self) -> LpmTrieNode<'a> {
            if let Some(index) = self.header.free_nodes_head.get() {
                // Pop an entry from the head of the free list if it's not empty.
                let node = self.node(index);
                self.header.free_nodes_head.set(node.header.left.take());
                node
            } else {
                // Allocate an unused node.
                let index = self.header.num_used_nodes.fetch_add(1, Ordering::Relaxed);

                // Number of used nodes is not expected to ever exceed the
                // number of nodes allocated in the VMO since it's bound by
                // the number of data entries.
                assert!((index as usize) < self.store.layout.num_nodes());

                self.node(index)
            }
        }

        pub fn release_node(&self, node: LpmTrieNode<'a>) {
            assert!(node.header.right.is_none());
            assert!(node.header.left.is_none());
            assert!(node.header.value.is_none());

            // Push the node the free nodes linked list. `header.left` is
            // reused for this linked list.
            node.header.left.set(self.header.free_nodes_head.get());
            self.header.free_nodes_head.set(Some(node.index));
        }
    }

    #[derive(Clone)]
    pub(super) struct DataEntry<'a> {
        buf: EbpfBufferPtr<'a>,
        index: u32,
    }

    impl<'a> DataEntry<'a> {
        fn header(&self) -> &DataEntryHeader {
            // SAFETY: `DataEntryHeader` contains only `AtomicU32` fields, so
            // dereferencing it without synchronization is safe.
            unsafe { &self.buf.get_ptr::<DataEntryHeader>(0).unwrap().deref() }
        }

        /// Reference counted for the entry
        fn ref_count(&self) -> &AtomicU32 {
            &self.header().ref_count
        }

        fn get_ref_count(&self) -> u32 {
            self.ref_count().load(Ordering::Relaxed)
        }

        /// Sets the `next` field that points to the next element in the free
        /// list.
        fn set_next(&mut self, next: Option<u32>) {
            self.header().next_free.set(next)
        }

        /// Returns the buffer pointing at the element value.
        pub fn value(&self) -> EbpfBufferPtr<'a> {
            self.buf.slice(DATA_ENTRY_HEADER_SIZE..self.buf.len()).unwrap()
        }
    }

    // A ref-counted reference to a data entry.
    pub struct LpmTrieEntryRef<'a> {
        store: LpmTrieStore<'a>,
        index: Option<u32>,
    }

    impl<'a> LpmTrieEntryRef<'a> {
        pub fn is_only_reference(&self) -> bool {
            let Some(index) = self.index else {
                return false;
            };
            let entry = self.store.data_entry(index);
            return entry.get_ref_count() == 1;
        }

        pub fn ptr(&self) -> EbpfBufferPtr<'a> {
            self.store.data_entry(self.index.unwrap()).value()
        }

        fn take_index(mut self) -> u32 {
            self.index.take().unwrap()
        }
    }

    impl<'a> Drop for LpmTrieEntryRef<'a> {
        fn drop(&mut self) {
            let Some(index) = self.index else {
                return;
            };
            let entry = self.store.data_entry(index);
            let ref_count = entry.ref_count().fetch_sub(1, Ordering::Relaxed);

            // Return the entry back to the free list.
            if ref_count == 1 {
                self.store.heap().write().release(entry);
            }
        }
    }
}

pub(super) use internal::LpmTrieEntryRef;
use internal::{Layout, LpmTrieNode, LpmTrieState, LpmTrieStore};

#[derive(Debug)]
pub struct LpmTrie {
    buffer: MapBuffer,
    layout: Layout,
}

fn get_branch_for_key<'a>(
    state: &'a LpmTrieState<'a>,
    node: LpmTrieNode<'a>,
    key: impl LpmKey,
) -> Option<LpmTrieNode<'a>> {
    let bit = key.get_bit(node.key().key_len());
    let index = match bit {
        None => None,
        Some(false) => node.left(),
        Some(true) => node.right(),
    };
    index.map(|index| state.node(index))
}

// Finds a node that matches the to specified key as much as possible.
// The following values are returned:
//   - `node` - the node with the longest key match,
//   - `path` - the list of nodes from the root to `node`, trimmed to
//     the specified `max_path`
//   - `num_matching_bits` - the number of matching bits.
// Note that `node` may be None. This indicates that the key matches all
// nodes in the `path`, and a node with the specified `key` is expected
// to be a child of the last node in the `path`, but it's not present.
// `node` and `path` may be empty if the trie is empty.
fn find_target_node<'a>(
    state: &'a LpmTrieState<'a>,
    key: impl LpmKey + Copy,
    max_path: usize,
) -> (Option<LpmTrieNode<'a>>, Vec<u32>, usize) {
    let mut path = vec![];
    let mut current_node = state.root();
    let mut num_matching_bits = 0;
    loop {
        let Some(node) = current_node else {
            // Return the longest match we've found since we can't go further.
            return (None, path, num_matching_bits);
        };

        let node_key_len = node.key().key_len();

        num_matching_bits = node.key().num_matching_bits(key);
        if num_matching_bits < node_key_len || num_matching_bits == key.key_len() {
            // We've found the deepest node that partially matches the
            // key. Return it.
            return (Some(node.clone()), path, num_matching_bits);
        }

        if path.len() >= max_path {
            path.remove(0);
        }
        path.push(node.index());

        // Proceed to one of the two branches branch of the current node.
        current_node = get_branch_for_key(state, node, key);
    }
}

fn update_root_or_child(
    state: &LpmTrieState<'_>,
    maybe_parent: Option<&LpmTrieNode<'_>>,
    new: Option<&LpmTrieNode<'_>>,
    old: Option<&LpmTrieNode<'_>>,
) {
    match maybe_parent {
        None => state.update_root(new, old),
        Some(parent) => parent.update_child(new, old),
    }
}

fn trim_node(
    state: &LpmTrieState<'_>,
    maybe_parent: Option<&LpmTrieNode<'_>>,
    node: LpmTrieNode<'_>,
) -> bool {
    // Nodes with a value cannot be trimmed.
    if node.has_value() {
        return false;
    }

    // The node can be trimmed if it has fewer than 2 children.
    let child = match (node.left(), node.right()) {
        (None, None) => None,
        (Some(left), None) => Some(state.node(left)),
        (None, Some(right)) => Some(state.node(right)),
        (Some(_), Some(_)) => return false,
    };

    update_root_or_child(&state, maybe_parent, child.as_ref(), Some(&node));
    if let Some(child) = child {
        node.update_child(None, Some(&child));
    }
    state.release_node(node);

    true
}

impl LpmTrie {
    pub fn new(schema: &MapSchema, vmo: impl Into<VmoOrName>) -> Result<Self, MapError> {
        // `BPF_F_NO_PREALLOC` is required for LPM Trie.
        if !schema.flags.contains(MapFlags::NoPrealloc) {
            return Err(MapError::InvalidParam);
        }

        let layout = Layout::new(schema)?;
        let buffer =
            MapBuffer::new(layout.total_size(), vmo.into().with_name_prefix("ebpf:lpm_trie"))?;

        Ok(Self { buffer, layout })
    }

    fn store<'a>(&'a self) -> LpmTrieStore<'a> {
        LpmTrieStore::new(&self.buffer, self.layout)
    }

    // All callers pass keys of length layout.key_size (>= LPM_KEY_PREFIX_SIZE + 1)
    fn is_valid_key(&self, key: &[u8]) -> bool {
        key.key_len() <= 8 * (self.layout.key_size as usize - 4)
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        let state = self.store().trie().read();
        state.root().is_none()
    }
}

impl MapImpl for LpmTrie {
    fn lookup<'a>(&'a self, key: &[u8]) -> Option<MapValueRef<'a>> {
        if !self.is_valid_key(key) {
            return None;
        }
        let state = self.store().trie().read();
        let mut longest_match = None;
        let mut current_node = state.root();
        loop {
            let Some(node) = current_node else {
                // Return the longest match we've found if we can't go further.
                break;
            };

            // If the current node is not the prefix of the target key then
            // we don't need to search further.
            if !node.key().is_prefix_of(key) {
                break;
            }

            if node.has_value() {
                longest_match = Some(node.clone());
            }

            // Proceed to one of the two branches branch of the current node.
            current_node = get_branch_for_key(&state, node, key);
        }

        longest_match.map(|node| MapValueRef::new_from_lpm_trie(state.value(&node).unwrap()))
    }

    fn update(&self, key: &[u8], value: EbpfBufferPtr<'_>, flags: u64) -> Result<(), MapError> {
        let state = self.store().trie().write();

        assert!(key.len() == self.layout.key_size as usize);
        if !self.is_valid_key(key) {
            return Err(MapError::InvalidKey);
        }

        let (node, mut path, bits_match) = find_target_node(&state, key, 1);
        let parent = path.pop().map(|index| state.node(index));

        if let Some(node) = &node {
            if bits_match == key.key_len() && bits_match == node.key().key_len() {
                if flags & (BPF_NOEXIST as u64) != 0 {
                    return Err(MapError::EntryExists);
                }

                // We have a node that matches the `key` exactly. Just store
                // the new value in that node. The old value is dropped first
                // (if any) to free space in the heap in case it's full.
                std::mem::drop(state.take_value(&node));
                let data_entry = self.store().heap().write().alloc().ok_or(MapError::SizeLimit)?;
                data_entry.ptr().copy(&value);
                node.set_value(data_entry);

                return Ok(());
            }
        }

        if flags & (BPF_EXIST as u64) != 0 {
            return Err(MapError::InvalidKey);
        }

        // Allocate a data entry in the heap. This has to be done first to
        // ensure we have space for the new element. As long as this operation
        // succeeds it's safe to assume that we will be able to allocate 1 or
        // 2 new nodes below, so the rest of the operation can't fail.
        let data_entry = self.store().heap().write().alloc().ok_or(MapError::SizeLimit)?;
        data_entry.ptr().copy(&value);

        let new_node = state.allocate_node();
        new_node.set_key(key, None);
        new_node.set_value(data_entry);

        match node {
            None => {
                // There is no node with partial key match. Add a new leaf
                // node. This also includes the case when the tree is empty -
                // in that case the `parent` is none and the new node becomes
                // the root.
                update_root_or_child(&state, parent.as_ref(), Some(&new_node), None);
            }
            Some(node) if bits_match == key.key_len() => {
                assert!(bits_match < node.key().key_len());

                // The new node is a prefix of the `node`. Insert a new node
                // between `parent` and `node`.
                update_root_or_child(&state, parent.as_ref(), Some(&new_node), Some(&node));
                new_node.update_child(Some(&node), None);
            }
            Some(node) => {
                // `key` differs from the `node`'s key. In this case `node`
                // should become a sibling of the `new_node`, which requires
                // a new parent.
                let new_parent = state.allocate_node();
                new_parent.set_key(key, Some(bits_match));
                update_root_or_child(&state, parent.as_ref(), Some(&new_parent), Some(&node));
                new_parent.update_child(Some(&node), None);
                new_parent.update_child(Some(&new_node), None);
            }
        }

        Ok(())
    }

    fn delete(&self, key: &[u8]) -> Result<(), MapError> {
        if !self.is_valid_key(key) {
            return Err(MapError::InvalidKey);
        }
        let state = self.store().trie().write();
        let (node, mut path, bits_match) = find_target_node(&state, key, 2);

        let Some(node) = node else {
            return Err(MapError::InvalidKey);
        };

        if bits_match < key.key_len() || bits_match < node.key().key_len() {
            return Err(MapError::InvalidKey);
        }

        // We've found the target node, try removing the value.
        let value = state.take_value(&node);
        if value.is_none() {
            return Err(MapError::InvalidKey);
        }
        std::mem::drop(value);

        // Check if the current node can be removed now. If it can, then try to
        // trim the parent as well.
        let parent = path.pop().map(|index| state.node(index));
        if trim_node(&state, parent.as_ref(), node) {
            let grandparent = path.pop().map(|index| state.node(index));
            if let Some(parent) = parent {
                trim_node(&state, grandparent.as_ref(), parent);
            }
        }

        Ok(())
    }

    fn get_next_key(&self, key: Option<&[u8]>) -> Result<MapKey, MapError> {
        if let Some(key) = key {
            if !self.is_valid_key(key) {
                return Err(MapError::InvalidKey);
            }
        }
        let state = self.store().trie().read();

        fn get_leftmost_node<'a>(
            state: &LpmTrieState<'a>,
            mut node: LpmTrieNode<'a>,
        ) -> Result<MapKey, MapError> {
            loop {
                node = match node.left() {
                    None => {
                        assert!(node.has_value());
                        return Ok(node.key().load());
                    }
                    Some(index) => state.node(index),
                };
            }
        }

        let get_first = || {
            let root = state.root().ok_or(MapError::InvalidKey)?;
            get_leftmost_node(&state, root)
        };

        let Some(key) = key else {
            return get_first();
        };

        let (node, mut path, bits_match) = find_target_node(&state, key, usize::MAX);

        let Some(mut node) = node else {
            return get_first();
        };

        let is_exact_match = bits_match == key.key_len() && bits_match == node.key().key_len();
        if !is_exact_match || !node.has_value() {
            return get_first();
        }

        // If we have a right branch then proceed there.
        if let Some(right) = node.right() {
            return get_leftmost_node(&state, state.node(right));
        }

        // Otherwise ascend up the tree.
        loop {
            // If the `path` is empty it means that `node` is the root.
            let parent = state.node(path.pop().ok_or(MapError::InvalidKey)?);

            // If we are ascending from the left branch then first return the
            // parent then proceed to the right branch.
            if parent.left() == Some(node.index()) {
                if parent.has_value() {
                    return Ok(parent.key().load());
                }

                if let Some(right) = parent.right() {
                    return get_leftmost_node(&state, state.node(right));
                }
            }

            node = parent;
        }
    }

    fn vmo(&self) -> &Arc<zx::Vmo> {
        self.buffer.vmo()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use rand::seq::SliceRandom as _;
    use test_case::test_case;
    use zerocopy::{FromBytes, Immutable, IntoBytes};

    const TEST_KEY_MAX_BYTES: usize = 20;

    #[repr(C)]
    #[derive(IntoBytes, Immutable, FromBytes)]
    struct Key {
        len: u32,
        data: [u8; TEST_KEY_MAX_BYTES],
    }

    impl Key {
        fn new(len: u32, v: &[u8]) -> Self {
            let mut data = [0; TEST_KEY_MAX_BYTES];
            data[..v.len()].copy_from_slice(v);
            Self { len, data }
        }
    }

    fn serialize_key(len: u32, v: &[u8]) -> Vec<u8> {
        let key = Key::new(len, v);
        key.as_bytes().to_owned()
    }

    #[fuchsia::test]
    fn test_lpm_key() {
        let key1 = serialize_key(20, &[0b01010001, 0, 0b01010000]);
        assert_eq!(key1.as_slice().get_bit(0), Some(false));
        assert_eq!(key1.as_slice().get_bit(1), Some(true));
        assert_eq!(key1.as_slice().get_bit(6), Some(false));
        assert_eq!(key1.as_slice().get_bit(7), Some(true));

        assert_eq!(key1.as_slice().get_bit(18), Some(false));
        assert_eq!(key1.as_slice().get_bit(19), Some(true));
        assert_eq!(key1.as_slice().get_bit(20), None);

        let key2 = serialize_key(20, &[0b01010001, 0, 0b01110000]);
        assert_eq!(key1.as_slice().num_matching_bits(key2.as_slice()), 18);

        let key2 = serialize_key(8, &[0b01010001]);
        assert_eq!(key1.as_slice().num_matching_bits(key2.as_slice()), 8);

        let key2 = serialize_key(24, &[0b01010001, 0, 0b01010101]);
        assert_eq!(key1.as_slice().num_matching_bits(key2.as_slice()), 20);

        let key2 = serialize_key(8, &[0b01010001]);
        assert!(!key1.as_slice().is_prefix_of(key2.as_slice()));
        assert!(key2.as_slice().is_prefix_of(key1.as_slice()));

        // Try `num_matching_bits() with a key longer than 32 bits
        let long_key1 = serialize_key(80, &[0, 0, 0, 0, 0b10000000]);
        let long_key2 = serialize_key(88, &[0, 0, 0, 0, 0b10000100]);
        assert_eq!(long_key1.as_slice().num_matching_bits(long_key2.as_slice()), 37);

        let long_key1 = serialize_key(160, &[0, 1]);
        let long_key2 = serialize_key(160, &[0, 0]);
        assert_eq!(long_key1.as_slice().num_matching_bits(long_key2.as_slice()), 15);
    }

    #[fuchsia::test]
    fn test_update_and_lookup() {
        let trie = LpmTrie::new(
            &MapSchema {
                map_type: linux_uapi::bpf_map_type_BPF_MAP_TYPE_LPM_TRIE,
                key_size: std::mem::size_of::<Key>() as u32,
                value_size: 4,
                max_entries: 10,
                flags: MapFlags::NoPrealloc,
            },
            "test",
        )
        .unwrap();

        let lookup =
            |trie: &LpmTrie, key: &[u8]| trie.lookup(&key).unwrap().ptr().load::<16>().to_vec();

        let key1 = serialize_key(8, &[0b01010001]);
        assert!(&trie.lookup(&key1).is_none());
        trie.update(&key1, (&mut [1, 2, 3, 4]).into(), 0).unwrap();
        assert_eq!(lookup(&trie, &key1), vec![1, 2, 3, 4]);

        let key2 = serialize_key(16, &[0b01010001, 0b10101010]);
        assert_eq!(lookup(&trie, &key2), vec![1, 2, 3, 4]);
        trie.update(&key2, (&mut [5, 6, 7, 8]).into(), 0).unwrap();
        assert_eq!(lookup(&trie, &key1), vec![1, 2, 3, 4]);
        assert_eq!(lookup(&trie, &key2), vec![5, 6, 7, 8]);

        // Test lookup for a key that is a prefix of an existing key.
        let key3 = serialize_key(12, &[0b01010001, 0b10100000]);
        assert_eq!(lookup(&trie, &key3), vec![1, 2, 3, 4]);

        // Test lookup for a key that is not a prefix of an existing key.
        let key4 = serialize_key(12, &[0b01010001, 0b00000000]);
        assert_eq!(lookup(&trie, &key4), vec![1, 2, 3, 4]);

        // Test update for an existing key.
        trie.update(&key1, (&mut [9, 10, 11, 12]).into(), 0).unwrap();
        assert_eq!(lookup(&trie, &key1), vec![9, 10, 11, 12]);

        let missing_key = serialize_key(6, &[0b01011000]);
        assert!(&trie.lookup(&missing_key).is_none());
    }

    #[fuchsia::test]
    fn test_max_entries() {
        let trie = LpmTrie::new(
            &MapSchema {
                map_type: linux_uapi::bpf_map_type_BPF_MAP_TYPE_LPM_TRIE,
                key_size: std::mem::size_of::<Key>() as u32,
                value_size: 4,
                max_entries: 10,
                flags: MapFlags::NoPrealloc,
            },
            "test",
        )
        .unwrap();
        for i in 0..10 {
            let key = serialize_key(8, &[i]);
            trie.update(&key, (&mut [i, 1, 2, 3]).into(), 0).expect("Failed to update map entry");
        }

        // Shouldn't be able to insert another entry since the map is full.
        let key10 = serialize_key(8, &[10]);
        assert_eq!(trie.update(&key10, (&mut [10, 1, 2, 3]).into(), 0), Err(MapError::SizeLimit));

        // It's still possible to update an existing entry.
        let key2 = serialize_key(8, &[2]);
        trie.update(&key2, (&mut [5, 1, 2, 3]).into(), 0).expect("Failed to replace map entry");

        // Once a value is removed it should be possible to insert another one.
        trie.delete(key2.as_slice()).expect("Failed to delete map entry");
        trie.update(&key10, (&mut [10, 1, 2, 3]).into(), 0).expect("Failed to insert map entry");
    }

    #[fuchsia::test]
    fn test_invalid_prefix_len_lookup() {
        let trie = LpmTrie::new(
            &MapSchema {
                map_type: linux_uapi::bpf_map_type_BPF_MAP_TYPE_LPM_TRIE,
                key_size: std::mem::size_of::<Key>() as u32,
                value_size: 4,
                max_entries: 10,
                flags: MapFlags::NoPrealloc,
            },
            "test",
        )
        .unwrap();

        let valid_key = serialize_key(32, &[192, 168, 1, 0]);
        trie.update(&valid_key, (&mut [1, 2, 3, 4]).into(), 0).unwrap();

        // Lookup with out of range prefix_len must not panic
        let bogus_key = serialize_key(u32::MAX, &[192, 168, 1, 0]);
        assert!(trie.lookup(&bogus_key).is_none());
    }

    #[fuchsia::test]
    fn test_invalid_prefix_len_delete() {
        let trie = LpmTrie::new(
            &MapSchema {
                map_type: linux_uapi::bpf_map_type_BPF_MAP_TYPE_LPM_TRIE,
                key_size: std::mem::size_of::<Key>() as u32,
                value_size: 4,
                max_entries: 10,
                flags: MapFlags::NoPrealloc,
            },
            "test",
        )
        .unwrap();

        let valid_key = serialize_key(32, &[192, 168, 1, 0]);
        trie.update(&valid_key, (&mut [1, 2, 3, 4]).into(), 0).unwrap();

        // Delete with out of range prefix_len must not panic
        let bogus_key = serialize_key(u32::MAX, &[192, 168, 1, 0]);
        assert_eq!(trie.delete(&bogus_key), Err(MapError::InvalidKey));
    }

    #[fuchsia::test]
    fn test_invalid_prefix_len_get_next_key() {
        let trie = LpmTrie::new(
            &MapSchema {
                map_type: linux_uapi::bpf_map_type_BPF_MAP_TYPE_LPM_TRIE,
                key_size: std::mem::size_of::<Key>() as u32,
                value_size: 4,
                max_entries: 10,
                flags: MapFlags::NoPrealloc,
            },
            "test",
        )
        .unwrap();

        let valid_key = serialize_key(32, &[192, 168, 1, 0]);
        trie.update(&valid_key, (&mut [1, 2, 3, 4]).into(), 0).unwrap();

        // get_next_key with out of range prefix_len must not panic
        let bogus_key = serialize_key(u32::MAX, &[192, 168, 1, 0]);
        assert_eq!(trie.get_next_key(Some(&bogus_key)), Err(MapError::InvalidKey));
    }

    fn get_short_key(id: usize) -> Vec<u8> {
        let mut res = 0u16;
        for bit in 0..8 {
            let v = if id & (1 << bit) != 0 { 0b10 } else { 0b01 };
            res |= v << (bit * 2);
        }

        serialize_key(16, res.as_bytes())
    }

    fn get_long_key(id: usize) -> Vec<u8> {
        let mut bytes = [0; 20];
        for bit in 0..8 {
            bytes[bit] = if id & (1 << bit) != 0 { 0b00011000 } else { 0b00000110 };
        }

        serialize_key(64, &bytes)
    }

    fn get_variable_len_tail_key(id: usize) -> Vec<u8> {
        let mut bytes = [0; 20];
        let len = 2 + id / 2;
        let last_byte = (len - 1) / 8;
        let last_byte_bits = (len - 1) % 8 + 1;

        // All bytes of the key are the same except for the last byte, i.e.
        // all entropy is in the last byte.
        for i in 0..(last_byte as u8) {
            bytes[i as usize] = (i * 3).reverse_bits();
        }

        bytes[last_byte] =
            ((id as u8 ^ 0b01010101) & ((1u16 << last_byte_bits) - 1) as u8).reverse_bits();

        serialize_key(len as u32, &bytes)
    }

    fn get_variable_len_head_key(id: usize) -> Vec<u8> {
        let mut bytes = [0; 20];
        let len = 2 + id / 2;

        // All bytes of the key are the same except for the first byte.
        bytes[0] = (id as u8 ^ 0b01010101).reverse_bits();
        for i in 1u8..20 {
            bytes[i as usize] = (i * 3).reverse_bits();
        }

        serialize_key(len as u32, &bytes)
    }

    #[test_case(get_short_key; "short keys")]
    #[test_case(get_long_key; "long keys")]
    #[test_case(get_variable_len_tail_key; "variable len, tail entropy")]
    #[test_case(get_variable_len_head_key; "variable len, head entropy")]
    #[fuchsia::test]
    fn test_random_insert_order(get_key: impl Fn(usize) -> Vec<u8>) {
        const NUM_ENTRIES: u8 = 200;

        let trie = LpmTrie::new(
            &MapSchema {
                map_type: linux_uapi::bpf_map_type_BPF_MAP_TYPE_LPM_TRIE,
                key_size: std::mem::size_of::<Key>() as u32,
                value_size: 4,
                max_entries: NUM_ENTRIES.into(),
                flags: MapFlags::NoPrealloc,
            },
            "test",
        )
        .unwrap();
        let mut ids: Vec<u8> = (0..NUM_ENTRIES).collect();
        ids.shuffle(&mut rand::rng());

        for id in ids.iter() {
            let id = *id;
            let key = get_key(id as usize);
            trie.update(&key, (&mut [id, 1, 2, 3]).into(), 0).expect("Failed to update map entry");
        }

        // Try looking up all entries.
        for id in 0..NUM_ENTRIES {
            let key = get_key(id as usize);
            let value = trie.lookup(&key).unwrap().ptr().load::<16>();
            assert_eq!(&value[..], &[id, 1, 2, 3]);

            // Try looking up the same key with an extra bit at the end. This
            // should yield the same result or another key that's longer than the current one.
            let mut key2 = Key::read_from_bytes(&key).unwrap();
            key2.len += 1;
            let value = trie.lookup(&key2.as_bytes()).unwrap().ptr().load::<16>();
            if &value[..] != &[id, 1, 2, 3] {
                let found_id = value[0] as usize;
                assert!(get_key(found_id).as_slice().key_len() > key.as_slice().key_len());
            }
        }

        // Iterate all entries with `get_next_key()`.
        let mut key: Option<MapKey> = None;
        let mut num_keys = 0;
        loop {
            key = match trie.get_next_key(key.as_ref().map(|k| &k[..])) {
                Ok(k) => {
                    num_keys += 1;
                    Some(k)
                }
                Err(MapError::InvalidKey) => break,
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }
        assert_eq!(num_keys, NUM_ENTRIES);

        // Remove all entries in random order.
        ids.shuffle(&mut rand::rng());
        for id in ids.iter() {
            let id = *id;
            let key = get_key(id as usize);
            trie.delete(key.as_slice()).unwrap();
        }

        // Insert the same entries in a different order.
        ids.shuffle(&mut rand::rng());
        for id in ids.iter() {
            let id = *id;
            let key = get_key(id as usize);
            trie.update(&key, (&mut [id, 1, 2, 3]).into(), 0).expect("Failed to update map entry");
        }

        // Try looking up all entries.
        for id in 0..NUM_ENTRIES {
            let key = get_key(id as usize);
            let value = trie.lookup(&key).unwrap().ptr().load::<16>();
            assert_eq!(&value[..], &[id as u8, 1, 2, 3]);
        }

        // Remove all entries in random order.
        ids.shuffle(&mut rand::rng());
        for id in ids.iter() {
            let id = *id;
            let key = get_key(id as usize);
            trie.delete(key.as_slice()).unwrap();
        }

        assert!(trie.is_empty());
    }
}
