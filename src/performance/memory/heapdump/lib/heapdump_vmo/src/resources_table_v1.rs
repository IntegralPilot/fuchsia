// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use memory_mapped_vmo::MemoryMappedVmo;
use std::alloc::Layout;
use std::mem::{align_of, size_of, size_of_val};
use zerocopy::FromBytes;

/// An offset within the VMO.
type Offset = u32;
const OFFSET_INVALID: Offset = Offset::MAX;

// Stack traces are stored in compressed form as an array of u8 prefixed by a u16 stating its
// length. The u16 is guaranteed to be aligned.
type StackTraceLength = u16;

// Known stack traces are indexed by a hash table, whose linked list heads (one for each bucket) are
// stored at the beginning of the VMO.
//
// Note that the presence and the format of the hash table is meant to be an internal detail of the
// current ResourcesTableWriter implementation: ResourcesTableReader does not depend on it.
const NUM_STACK_BUCKETS: usize = 1 << 13;
type StackBucketHeads = [Offset; NUM_STACK_BUCKETS];

/// A resource key is just an offset into the VMO.
#[derive(Clone, Copy, Debug, Eq, FromBytes, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct ResourceKey(Offset);

impl ResourceKey {
    /// Used by tests in this crate to construct placeholder values.
    #[cfg(test)]
    pub(crate) const fn from_raw(offset: Offset) -> ResourceKey {
        ResourceKey(offset)
    }

    pub const fn into_raw(self) -> Offset {
        self.0
    }
}

#[repr(C)]
#[derive(Debug, FromBytes)]
pub struct ThreadInfo {
    pub koid: zx::sys::zx_koid_t,
    pub name: zx::Name,
}

/// Mediates write access to a VMO containing compressed stack traces and thread info structs.
///
/// Compressed stack traces are stored as a hash table for efficient deduplication. Thread
/// information structures are not deduplicated (it is expected that deduplication happens at a
/// higher level in the stack). Apart from the hash table's bucket heads, all data is immutable:
/// neither stack traces nor thread info structs can be modified or deleted after having been
/// inserted.
///
/// Hash collisions are handled by maintaining per-bucket linked lists. Specifically, an array of
/// list heads, one for each bucket, is stored at the beginning of the VMO. The remaining part of
/// the VMO contains the linked list nodes. Since nodes are immutable, insertion always happen at
/// the head of the list.
///
/// Thanks to the fact that inserted data is immutable, other readers are allowed to read data from
/// this VMO while ResourcesTableWriter is still alive. In particular, all insertion functions
/// return a ResourceKey, which is simply the offset of the just-inserted now-immutable data, and
/// that can be used by ResourcesTableReader to read data back without even having to know about the
/// hash table.
pub struct ResourcesTableWriter {
    storage: MemoryMappedVmo,
    watermark: usize, // Offset of the first unallocated byte.
}

impl ResourcesTableWriter {
    /// Initializes a VMO as an empty table and creates an AllocationsTableWriter to write into it.
    ///
    /// # Safety
    /// The caller must guarantee that data accesses to the `vmo` do not generate data conflicts,
    /// i.e. 1) only one ResourcesTableWriter can exist for this VMO 2) as this an append-only data
    /// structure, readers should only read through the ResourceKeys we create, that are guaranteed
    /// to point to VMO sections that have already been populated.
    pub unsafe fn new(vmo: &zx::Vmo) -> Result<ResourcesTableWriter, crate::Error> {
        let storage = unsafe { MemoryMappedVmo::new_readwrite(vmo)? };
        if storage.vmo_size() < size_of::<StackBucketHeads>() {
            return Err(crate::Error::BufferTooSmall);
        } else if storage.vmo_size() - 1 > Offset::MAX as usize {
            return Err(crate::Error::BufferTooBig);
        }

        let mut result = ResourcesTableWriter { storage, watermark: size_of::<StackBucketHeads>() };

        // Clear the hash table.
        for bucket_index in 0..NUM_STACK_BUCKETS {
            *result.stack_bucket_head_at(bucket_index) = OFFSET_INVALID;
        }

        Ok(result)
    }

    /// Allocates space in the VMO and returns the base offset of the allocated range.
    fn allocate(&mut self, layout: Layout) -> Result<Offset, crate::Error> {
        // Forbid alignment requirements greater than the page size, as they would have implications
        // on how the VMO can be mapped for reading.
        if layout.align() > zx::system_get_page_size() as usize {
            return Err(crate::Error::InvalidInput);
        }

        let result_start = (self.watermark + layout.align() - 1) & !(layout.align() - 1);
        let result_end = result_start + layout.size();

        if result_end <= self.storage.vmo_size() {
            self.watermark = result_end;
            Ok(result_start as Offset)
        } else {
            Err(crate::Error::OutOfSpace)
        }
    }

    /// This is the hash function for stack traces.
    fn compute_bucket_index(compressed_stack_trace: &[u8]) -> usize {
        let tmp = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(compressed_stack_trace);
        tmp as usize % NUM_STACK_BUCKETS
    }

    /// Returns a mutable reference to the head of a given bucket's linked list.
    fn stack_bucket_head_at(&mut self, bucket_index: usize) -> &mut Offset {
        // The bucket heads are always stored at the beginning of the VMO.
        let bucket_heads = self.storage.get_object_mut::<StackBucketHeads>(0).unwrap();
        &mut bucket_heads[bucket_index]
    }

    /// Tries to find an already-inserted stack trace in the given bucket by scanning its linked
    /// list.
    fn find_in_bucket(
        &mut self,
        bucket_index: usize,
        compressed_stack_trace: &[u8],
    ) -> Option<Offset> {
        let mut curr = *self.stack_bucket_head_at(bucket_index);
        while curr != OFFSET_INVALID {
            // Read the "next" field in the current node and its compressed stack trace, which is
            // stored immediately afterwards.
            let curr_next: Offset = *self.storage.get_object(curr as usize).unwrap();
            let payload_offset = curr as usize + size_of_val(&curr_next);
            let curr_payload = get_compressed_stack_trace(&self.storage, payload_offset).unwrap();

            // Is this stack trace the one we were looking for?
            if *curr_payload == *compressed_stack_trace {
                return Some(curr);
            }

            curr = curr_next;
        }

        // Not found.
        None
    }

    fn insert_in_bucket(
        &mut self,
        bucket_index: usize,
        compressed_stack_trace: &[u8],
    ) -> Result<Offset, crate::Error> {
        // Allocate space for:
        // - The "next" field
        // - The stack trace length
        // - The actual stack trace
        let alloc_bytes =
            size_of::<Offset>() + size_of::<StackTraceLength>() + compressed_stack_trace.len();
        let alloc_align = align_of::<Offset>();
        let new = self.allocate(Layout::from_size_align(alloc_bytes, alloc_align).unwrap())?;

        let old_head = *self.stack_bucket_head_at(bucket_index);

        // Write them.
        *self.storage.get_object_mut(new as usize).unwrap() = old_head;
        set_compressed_stack_trace(
            &mut self.storage,
            new as usize + size_of::<Offset>(),
            compressed_stack_trace,
        )
        .unwrap();

        // Update the bucket's head pointer.
        *self.stack_bucket_head_at(bucket_index) = new;

        Ok(new)
    }

    /// Appends a compressed stack trace and returns its offset into the VMO.
    ///
    /// This function also applies deduplication: if a copy of the given stack trace is already
    /// present, the offset of the existing copy is returned without modifying the VMO contents.
    pub fn intern_compressed_stack_trace(
        &mut self,
        compressed_stack_trace: &[u8],
    ) -> Result<(ResourceKey, bool), crate::Error> {
        // Verify that the length fits in StackTraceLength and return error if it does not.
        if compressed_stack_trace.len() > StackTraceLength::MAX as usize {
            return Err(crate::Error::BufferTooBig);
        }

        // Find/insert a StackNode and get its offset within the memory region.
        let bucket_index = Self::compute_bucket_index(compressed_stack_trace);
        let (offset, inserted) = match self.find_in_bucket(bucket_index, compressed_stack_trace) {
            Some(offset) => (offset, false),
            None => (self.insert_in_bucket(bucket_index, compressed_stack_trace)?, true),
        };

        // Adjust the returned offset to skip the "next" field (which is an internal
        // ResourcesTableWriter implementation detail) and point directly to the StackTraceLength
        // field (which is what ResourcesTableReader expects to receive).
        let resource_key = ResourceKey(offset + size_of::<Offset>() as Offset);
        Ok((resource_key, inserted))
    }

    /// Appends a thread information entry and returns its offset into the VMO.
    pub fn insert_thread_info(
        &mut self,
        koid: zx::sys::zx_koid_t,
        name: &zx::Name,
    ) -> Result<ResourceKey, crate::Error> {
        let offset = self.allocate(Layout::new::<ThreadInfo>())?;
        *self.storage.get_object_mut(offset as usize).unwrap() = ThreadInfo { koid, name: *name };
        Ok(ResourceKey(offset))
    }
}

/// Mediates read access to a VMO written by ResourcesTableWriter.
pub struct ResourcesTableReader {
    storage: MemoryMappedVmo,
}

impl ResourcesTableReader {
    /// # Safety
    /// The caller must guarantee that data in the `vmo` is not concurrently written by others.
    /// Only using ResourceKeys received from the corresponding ResourcesTableWriter satisfies
    /// this requirement.
    pub unsafe fn new(vmo: &zx::Vmo) -> Result<ResourcesTableReader, crate::Error> {
        let storage = unsafe { MemoryMappedVmo::new_readonly(vmo)? };
        Ok(ResourcesTableReader { storage })
    }

    /// Gets the compressed stack trace identified by `resource_key`.
    pub fn get_compressed_stack_trace(
        &self,
        resource_key: ResourceKey,
    ) -> Result<&[u8], crate::Error> {
        let ResourceKey(offset) = resource_key;
        get_compressed_stack_trace(&self.storage, offset as usize)
    }

    /// Gets the thread info entry identified by `resource_key`.
    pub fn get_thread_info(&self, resource_key: ResourceKey) -> Result<&ThreadInfo, crate::Error> {
        let ResourceKey(offset) = resource_key;
        Ok(self.storage.get_object(offset as usize)?)
    }
}

fn get_compressed_stack_trace(
    storage: &MemoryMappedVmo,
    byte_offset: usize,
) -> Result<&[u8], crate::Error> {
    // Read the length.
    let header: StackTraceLength = *storage.get_object(byte_offset)?;

    // Get actual data, which is stored immediately after the length, as a slice.
    Ok(storage.get_slice(byte_offset + size_of_val(&header), header as usize)?)
}

fn set_compressed_stack_trace(
    storage: &mut MemoryMappedVmo,
    byte_offset: usize,
    compressed_stack_trace: &[u8],
) -> Result<(), crate::Error> {
    let header: StackTraceLength =
        compressed_stack_trace.len().try_into().map_err(|_| crate::Error::BufferTooBig)?;

    // Write the length.
    *storage.get_object_mut(byte_offset)? = header;

    // Write actual data immediately after the length.
    storage
        .get_slice_mut(byte_offset + size_of_val(&header), compressed_stack_trace.len())?
        .copy_from_slice(compressed_stack_trace);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    // Some tests below use this constant to create a VMO with a known size.
    const VMO_SIZE: usize = 4 * 1024 * 1024; // 4 MiB

    struct TestStorage {
        vmo: zx::Vmo,
    }

    impl TestStorage {
        pub fn new(vmo_size: usize) -> (TestStorage, ResourcesTableWriter) {
            let vmo = zx::Vmo::create(vmo_size as u64).unwrap();

            // SAFETY: only one ResourcesTableWriter can ever be created.
            let writer = unsafe { ResourcesTableWriter::new(&vmo) }.unwrap();

            (TestStorage { vmo }, writer)
        }

        fn create_reader(&self) -> ResourcesTableReader {
            // SAFETY: Tests only use ResourceKeys created by the writer to access it.
            unsafe { ResourcesTableReader::new(&self.vmo) }.unwrap()
        }
    }

    #[test]
    fn test_stack_trace_deduplication() {
        let (storage, mut writer) = TestStorage::new(VMO_SIZE);

        // Insert different distinct stack traces and store the corresponding resource keys.
        // The number of stack traces is chosen so that at least one bucket contains at least three
        // stack traces.
        const COUNT: usize = 2 * NUM_STACK_BUCKETS + 1;
        let mut pairs = Vec::new();
        for i in 0..COUNT {
            // Generate a unique array of bytes and pretend it is a compressed stack trace.
            let stack_trace = i.to_ne_bytes();

            let (resource_key, inserted) =
                writer.intern_compressed_stack_trace(&stack_trace).unwrap();
            assert!(inserted, "expected true because the stack trace was not present");

            pairs.push((stack_trace, resource_key));
        }

        // Verify that trying to insert them again returns the same resource keys.
        for (stack_trace, expected_resource_key) in &pairs {
            let (actual_resource_key, inserted) =
                writer.intern_compressed_stack_trace(stack_trace).unwrap();
            assert!(!inserted, "expected false because the stack trace is already present");
            assert_eq!(actual_resource_key, *expected_resource_key);
        }

        // Verify that they can be read back.
        let reader = storage.create_reader();
        for (expected_stack_trace, resource_key) in &pairs {
            let actual_stack_trace = reader.get_compressed_stack_trace(*resource_key).unwrap();
            assert_eq!(actual_stack_trace, *expected_stack_trace);
        }
    }

    #[test]
    fn test_empty_stack_trace() {
        let (storage, mut writer) = TestStorage::new(VMO_SIZE);

        // It must be possible to insert the empty stack trace.
        let (resource_key, inserted) = writer.intern_compressed_stack_trace(&[]).unwrap();
        assert!(inserted);

        // Verify that is can be read back correctly.
        let reader = storage.create_reader();
        let read_result = reader.get_compressed_stack_trace(resource_key).unwrap();
        assert_eq!(read_result, []);
    }

    #[test]
    fn test_long_stack_traces() {
        let (storage, mut writer) = TestStorage::new(VMO_SIZE);

        // Inserting a stack trace whose length cannot be represented in the length field (u16).
        // should fail.
        let stack_trace_too_long = vec![0xAA; u16::MAX as usize + 1];
        let result = writer.intern_compressed_stack_trace(&stack_trace_too_long);
        assert_matches!(result, Err(crate::Error::BufferTooBig));

        // Inserting a stack trace with the maximum representable length should succeed.
        let stack_trace_max_len = vec![0x55; u16::MAX as usize];
        let (resource_key, _) = writer.intern_compressed_stack_trace(&stack_trace_max_len).unwrap();

        // And it must be possible to read it back.
        let reader = storage.create_reader();
        let read_result = reader.get_compressed_stack_trace(resource_key).unwrap();
        assert_eq!(read_result, stack_trace_max_len);
    }

    #[test]
    fn test_write_until_out_of_space() {
        let (_storage, mut writer) = TestStorage::new(VMO_SIZE);

        // Insert many distinct stack traces and verify that, at some point, we get an OutOfSpace
        // error. Instead of estimating exactly how many stack traces can fit, we just use VMO_SIZE
        // as an upper bound before declaring failure (each distinct stack trace obviously requires
        // at least one byte of storage).
        for i in 0..=VMO_SIZE {
            // Generate a unique array of bytes and pretend it is a compressed stack trace.
            let stack_trace = i.to_ne_bytes();

            if let Err(crate::Error::OutOfSpace) =
                writer.intern_compressed_stack_trace(&stack_trace)
            {
                return; // Test passed
            }
        }

        unreachable!("Inserted more than {} distinct stack traces", VMO_SIZE);
    }

    #[test]
    fn test_thread_info() {
        let (storage, mut writer) = TestStorage::new(VMO_SIZE);

        // Insert a thread info struct with placeholder values (the name must be padded to the
        // expected length).
        const FAKE_KOID: zx::sys::zx_koid_t = 1234;
        const FAKE_NAME: zx::Name = zx::Name::new_lossy("fake-name");
        let resource_key = writer.insert_thread_info(FAKE_KOID, &FAKE_NAME).unwrap();

        // Verify that it can be read back correctly.
        let reader = storage.create_reader();
        let thread_info = reader.get_thread_info(resource_key).unwrap();
        assert_eq!(thread_info.koid, FAKE_KOID);
        assert_eq!(thread_info.name, FAKE_NAME);
    }
}
