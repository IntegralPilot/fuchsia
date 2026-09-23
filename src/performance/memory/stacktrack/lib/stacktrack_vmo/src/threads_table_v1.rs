// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use memory_mapped_vmo::MemoryMappedVmo;
use static_assertions::const_assert;
use std::mem::{align_of, size_of};
use std::ops::Deref;
use std::sync::atomic::{AtomicU32, Ordering};
use zerocopy::FromBytes;

pub type NodeIndex = u32;

/// The index used to represent NULL or invalid node.
/// This is also where the Header is stored.
pub const NODE_INVALID: NodeIndex = 0;

pub const MAX_FRAMES: usize = 128;

// Define AtomicNodeIndex as a newtype so we can implement traits on it.
#[repr(transparent)]
#[derive(Debug, FromBytes)]
pub struct AtomicNodeIndex(AtomicU32);

impl AtomicNodeIndex {
    pub const fn new(value: u32) -> AtomicNodeIndex {
        AtomicNodeIndex(AtomicU32::new(value))
    }
}

impl Deref for AtomicNodeIndex {
    type Target = AtomicU32;

    fn deref(&self) -> &AtomicU32 {
        &self.0
    }
}

/// VMO Header.
#[repr(C)]
#[derive(Debug, FromBytes)]
pub struct Header {
    /// Index of the first node in the list.
    head: AtomicNodeIndex,
}

/// A node in the stack track VMO.
#[repr(C)]
#[derive(Debug, FromBytes)]
pub struct Node {
    /// Index of the next node.
    next: AtomicNodeIndex,
    /// Index of the previous node.
    prev: u32,
    /// Thread KOID.
    pub koid: u64,
    /// Number of valid frames.
    pub count: u32,
    /// Stack frames.
    pub frames: [Frame; MAX_FRAMES],
}

// Ensure Header fits in Node 0.
const_assert!(size_of::<Header>() < size_of::<Node>());
const_assert!(align_of::<Header>() <= align_of::<Node>());

/// Information about a stack frame.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, FromBytes, PartialEq)]
pub struct Frame {
    // LINT.IfChange
    pub pc: u64,
    pub fp: u64,
    // LINT.ThenChange(//src/performance/memory/stacktrack/instrumentation/src/unwind.cc)
}

/// Writer for the Stacktrack VMO.
pub struct StacktrackWriter {
    memory: MemoryMappedVmo,
    capacity: u32,
    free_head: NodeIndex,
    watermark: u32,
}

impl StacktrackWriter {
    /// Initializes a VMO as an empty table and creates a StacktrackWriter to write into it.
    ///
    /// # Safety
    /// The caller must guarantee that the `vmo` is not accessed by others while the returned
    /// instance is alive. However, it always safe to take a snapshot and read that instead.
    pub unsafe fn new(vmo: &zx::Vmo) -> Result<Self, crate::Error> {
        let memory = unsafe { MemoryMappedVmo::new_readwrite(vmo)? };

        let capacity = compute_node_capacity(memory.vmo_size())?;

        // Ensure we have at least one node for the header.
        if capacity == 0 {
            return Err(crate::Error::BufferTooSmall);
        }

        Ok(Self {
            memory,
            capacity: capacity as u32,
            free_head: NODE_INVALID,
            watermark: 1, // Start allocating after the header.
        })
    }

    fn get_header_mut(&mut self) -> &mut Header {
        self.memory.get_object_mut(0).unwrap()
    }

    fn get_node_mut(&mut self, index: NodeIndex) -> &mut Node {
        assert!(index != NODE_INVALID && index < self.capacity, "index out of bounds");
        let offset = index as usize * size_of::<Node>();
        self.memory.get_object_mut(offset).unwrap()
    }

    /// Inserts a new node at the head of the linked-list.
    ///
    /// The index of the new node is returned on success.
    pub fn insert_at_head(
        &mut self,
        koid: u64,
        frames: &[Frame],
    ) -> Result<NodeIndex, crate::Error> {
        // Pop a free node from the free list.
        let node_idx = if self.free_head != NODE_INVALID {
            let node_idx = self.free_head;
            let node = self.get_node_mut(node_idx);
            let next_free_idx = node.next.load(Ordering::Relaxed);

            // Ensure that the node we are about to use has no links to other nodes.
            assert_eq!(node.prev, NODE_INVALID, "free list is corrupted");
            node.next.store(NODE_INVALID, Ordering::Relaxed);

            self.free_head = next_free_idx;
            node_idx
        } else {
            let idx = self.watermark;
            if idx < self.capacity {
                self.watermark += 1;
                idx
            } else {
                return Err(crate::Error::OutOfSpace);
            }
        };

        // Set the new node's contents and links (at this point, it's not reachable yet).
        let old_head_idx = self.get_header_mut().head.load(Ordering::Relaxed);
        let node = self.get_node_mut(node_idx);
        node.koid = koid;
        node.count = frames.len() as u32;
        let count = frames.len().min(MAX_FRAMES);
        node.frames[..count].copy_from_slice(&frames[..count]);
        node.prev = NODE_INVALID;
        node.next.store(old_head_idx, Ordering::Release);

        // Update the old head's link to the new node (the prev link is only used by the writer, so
        // it doesn't need to be set atomically).
        if old_head_idx != NODE_INVALID {
            let old_head = self.get_node_mut(old_head_idx);
            old_head.prev = node_idx;
        }

        // Update the head pointer. This is the operation that makes the new node reachable by the
        // reader and it must be atomic.
        self.get_header_mut().head.store(node_idx, Ordering::Release);

        Ok(node_idx)
    }

    /// Remove the node at the given index.
    pub fn remove(&mut self, node_idx: NodeIndex) {
        let node = self.get_node_mut(node_idx);
        let prev_idx = node.prev;
        let next_idx = node.next.load(Ordering::Acquire);

        // Update the next node's link to the previous node.
        if next_idx != NODE_INVALID {
            let next_node = self.get_node_mut(next_idx);
            next_node.prev = prev_idx;
        }

        // Update the previous node's link to the next node. This is what makes the node
        // unreachable.
        if prev_idx != NODE_INVALID {
            let prev = self.get_node_mut(prev_idx);
            prev.next.store(next_idx, Ordering::Release);
        } else {
            // The node was the head, so update the head pointer.
            self.get_header_mut().head.store(next_idx, Ordering::Release);
        }

        // Push the node into the free list.
        let next_free_idx = std::mem::replace(&mut self.free_head, node_idx);
        let node = self.get_node_mut(node_idx);
        node.next.store(next_free_idx, Ordering::Relaxed);
        node.prev = NODE_INVALID;
    }
}

pub struct StacktrackReader {
    memory: MemoryMappedVmo,
    capacity: u32,
}

impl StacktrackReader {
    /// # Safety
    /// The caller must guarantee that the `vmo` is not accessed by others while the returned
    /// instance is alive, usually by taking a snapshot of the VMO that StacktrackWriter
    /// operates on and then reading the snapshot instead.
    pub unsafe fn new(vmo: &zx::Vmo) -> Result<Self, crate::Error> {
        let memory = unsafe { MemoryMappedVmo::new_readonly(vmo)? };

        let capacity = compute_node_capacity(memory.vmo_size())?;

        Ok(Self { memory, capacity: capacity as u32 })
    }

    fn get_header(&self) -> &Header {
        self.memory.get_object(0).unwrap()
    }

    fn get_node(&self, index: NodeIndex) -> &Node {
        assert!(index != 0 && index < self.capacity, "index out of bounds");
        let offset = index as usize * size_of::<Node>();
        self.memory.get_object(offset).unwrap()
    }

    pub fn iter(&self) -> StacktrackIterator<'_> {
        // We need header to start iteration.
        let head_index = self.get_header().head.load(Ordering::Acquire);
        StacktrackIterator { reader: self, next_index: head_index }
    }
}

pub struct StacktrackIterator<'a> {
    reader: &'a StacktrackReader,
    next_index: NodeIndex,
}

impl<'a> Iterator for StacktrackIterator<'a> {
    type Item = &'a Node;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next_index == NODE_INVALID || self.next_index >= self.reader.capacity {
            return None;
        }

        let node = self.reader.get_node(self.next_index);
        self.next_index = node.next.load(Ordering::Acquire);
        Some(node)
    }
}

fn compute_node_capacity(num_bytes: usize) -> Result<usize, crate::Error> {
    if num_bytes < size_of::<Node>() {
        return Err(crate::Error::BufferTooSmall);
    }
    Ok(num_bytes / size_of::<Node>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use itertools::Itertools;

    const NUM_ITERATIONS: usize = 100;
    const NUM_NODES: usize = NUM_ITERATIONS + 10;

    struct TestStorage {
        vmo: zx::Vmo,
    }

    impl TestStorage {
        pub fn new(num_nodes: usize) -> (TestStorage, StacktrackWriter) {
            let vmo_size = num_nodes * size_of::<Node>();
            let vmo = zx::Vmo::create(vmo_size as u64).unwrap();
            let writer = unsafe { StacktrackWriter::new(&vmo).unwrap() };
            (TestStorage { vmo }, writer)
        }

        fn create_reader(&self) -> StacktrackReader {
            let snapshot = self
                .vmo
                .create_child(
                    zx::VmoChildOptions::SNAPSHOT | zx::VmoChildOptions::NO_WRITE,
                    0,
                    self.vmo.get_size().unwrap(),
                )
                .unwrap();
            unsafe { StacktrackReader::new(&snapshot).unwrap() }
        }
    }

    #[test]
    fn test_read_empty() {
        let (storage, _writer) = TestStorage::new(NUM_NODES);
        let reader = storage.create_reader();
        assert_eq!(reader.iter().count(), 0);
    }

    #[test]
    fn test_allocation_exhaustion() {
        let (_storage, mut writer) = TestStorage::new(10);
        let frames = [Frame::default()];

        // Allocate all the available nodes. Indices will be 1-9.
        for i in 1..=9 {
            assert_eq!(writer.insert_at_head(i as u64, &frames), Ok(i));
        }

        // The next allocation should fail.
        assert_eq!(writer.insert_at_head(10, &frames), Err(crate::Error::OutOfSpace));

        // Free one.
        writer.remove(9);

        // Insertion should succeed now.
        assert_eq!(writer.insert_at_head(11, &frames), Ok(9));
    }

    #[test]
    fn test_lifecycle() {
        let (storage, mut writer) = TestStorage::new(NUM_NODES);
        let frames = [Frame { pc: 0x123, fp: 0x456 }];

        // Insert one node (takes index 1).
        assert_eq!(writer.insert_at_head(101, &frames), Ok(1));

        // Read back its contents.
        {
            let reader = storage.create_reader();
            let nodes: Vec<_> = reader.iter().collect();
            assert_eq!(nodes.len(), 1);
            assert_eq!(nodes[0].koid, 101);
            assert_eq!(nodes[0].count, 1);
            assert_eq!(nodes[0].frames[0].pc, 0x123);
            assert_eq!(nodes[0].frames[0].fp, 0x456);
        }

        // Insert another node (takes index 2)
        assert_eq!(writer.insert_at_head(102, &frames), Ok(2));

        // Remove the previous node (index 1).
        writer.remove(1);

        // Verify read (should see 102, which is at index 2)
        {
            let reader = storage.create_reader();
            let nodes: Vec<_> = reader.iter().collect();
            assert_eq!(nodes.len(), 1);
            assert_eq!(nodes[0].koid, 102);
        }

        // Remove the second node too (index 2).
        writer.remove(2);

        // Verify that the table is now empty.
        {
            let reader = storage.create_reader();
            assert_eq!(reader.iter().count(), 0);
        }
    }

    #[test]
    fn test_multiple_nodes() {
        let (storage, mut writer) = TestStorage::new(NUM_NODES);
        let frames = [Frame::default()];

        assert_eq!(writer.insert_at_head(101, &frames), Ok(1));
        assert_eq!(writer.insert_at_head(102, &frames), Ok(2));

        let reader = storage.create_reader();
        let nodes = reader.iter().map(|node| node.koid).sorted().collect_vec();
        assert_eq!(nodes, [101, 102]);
    }
}
