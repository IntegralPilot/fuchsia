// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::{MaybeUninit, size_of};
use core::slice;
use core::sync::atomic::{AtomicU64, Ordering};
use zx_status::Status;

/// Possible error conditions when attempting to allocate a blob ID.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AllocateError {
    /// The header was invalid (purportedly either with indices extending past the blob head
    /// offset or the blob head offset extending past the length of the buffer). Suggests invalid
    /// initialization or corruption.
    InvalidHeader,
    /// The would-be bookkeeping index slot for the new allocated ID is non-empty (i.e., not in
    /// the initialized state). Suggests corruption.
    NonEmptyIndex,
    /// Out Of Memory: there is insufficient memory available for the requested allocation.
    /// (What is available can be queried with `remaining_bytes()`).
    OutOfMemory,
}

impl From<AllocateError> for Status {
    fn from(err: AllocateError) -> Self {
        match err {
            AllocateError::OutOfMemory => Status::NO_MEMORY,
            AllocateError::InvalidHeader | AllocateError::NonEmptyIndex => {
                Status::IO_DATA_INTEGRITY
            }
        }
    }
}

/// Error type for `allocate_with` combining allocator errors and copy errors.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AllocateErrorWith<E> {
    /// Allocation error within the ID allocator structure.
    Error(AllocateError),
    /// Error returned by the caller-provided copy closure.
    Copy(E),
}

impl<E> From<AllocateErrorWith<E>> for Status
where
    Status: From<E>,
{
    fn from(err: AllocateErrorWith<E>) -> Self {
        match err {
            AllocateErrorWith::Error(e) => e.into(),
            AllocateErrorWith::Copy(e) => Status::from(e),
        }
    }
}

/// Possible error conditions when retrieving a blob by ID, either via iteration or `get_blob()`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlobError {
    /// The header was invalid (purportedly either with indices extending past the blob head
    /// offset or the blob head offset extending past the length of the buffer). Suggests invalid
    /// initialization or corruption.
    InvalidHeader,
    /// The requested ID has not yet been allocated.
    UnallocatedId,
    /// Suggests a lost race in which the requested ID is valid and the header has been updated
    /// to reflect that, but the bookkeeping index has not yet been committed. Such a case
    /// unfortunately is indistinguishable from the other possibility that the index was already
    /// committed but then subsequently corrupted with zeroes. Where there is confidence in the
    /// first case, this call should be retried.
    UncommittedIndex,
    /// The corresponding bookkeeping index is invalid, with the blob purportedly not being
    /// contained within `[blob head, end of region)`. Suggests corruption.
    InvalidIndex,
}

impl From<BlobError> for Status {
    fn from(err: BlobError) -> Self {
        match err {
            BlobError::UnallocatedId => Status::NOT_FOUND,
            BlobError::UncommittedIndex => Status::SHOULD_WAIT,
            BlobError::InvalidHeader | BlobError::InvalidIndex => Status::IO_DATA_INTEGRITY,
        }
    }
}

/// Specifies whether memory should be zeroed when initializing a `BlobIdAllocator`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ZeroFill {
    No,
    Yes,
}

/// Header for the ID allocator region.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub next_id: u32,
    pub blob_head: u32,
}

const _: () = assert!(size_of::<Header>() == size_of::<u64>());

impl Header {
    pub const SIZE: usize = size_of::<Self>();

    pub fn from_u64(val: u64) -> Self {
        Self { next_id: val as u32, blob_head: (val >> 32) as u32 }
    }

    pub fn to_u64(self) -> u64 {
        (self.next_id as u64) | ((self.blob_head as u64) << 32)
    }

    /// Returns the byte offset just past the last bookkeeping index, or `None` if the computation
    /// would overflow a `u32`.
    pub fn index_end(self) -> Option<u32> {
        self.next_id.checked_mul(Index::SIZE as u32)?.checked_add(Header::SIZE as u32)
    }

    /// Returns `true` if the header reflects a valid state for a buffer of `length` bytes.
    pub fn is_valid(self, length: usize) -> bool {
        self.remaining_bytes(length).is_some()
    }

    /// Returns the remaining number of available bytes in the region, given the length of the
    /// region, or `None` in the event of an invalid header.
    pub fn remaining_bytes(self, length: usize) -> Option<usize> {
        let end = self.index_end()?;
        if end <= self.blob_head && (self.blob_head as usize) <= length {
            Some((self.blob_head - end) as usize)
        } else {
            None
        }
    }
}

/// Represents a blob bookkeeping index slot.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Index {
    pub size: u32,
    pub offset: u32,
}

const _: () = assert!(size_of::<Index>() == size_of::<u64>());

impl Index {
    pub const SIZE: usize = size_of::<Self>();

    pub fn from_u64(val: u64) -> Self {
        Self { size: val as u32, offset: (val >> 32) as u32 }
    }

    pub fn to_u64(self) -> u64 {
        (self.size as u64) | ((self.offset as u64) << 32)
    }

    /// See `BlobError::InvalidIndex`. The current blob head offset and length of the region must
    /// be provided, and are assumed to have already been validated.
    pub fn is_valid(self, blob_head: u32, length: usize) -> bool {
        debug_assert!((blob_head as usize) <= length);
        (blob_head <= self.offset)
            && (self.offset as usize <= length)
            && (self.size as usize <= length - (self.offset as usize))
    }
}

/// Helper struct implementing the lock-free blob-id allocator over memory.
///
/// Represents a thread-safe view into an IOBuffer region of "ID allocator" discipline
/// (`ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR`), used to map sized data blobs to sequentially-allocated
/// numeric IDs.
///
/// Suppose there are N mapped blobs. The memory is laid out as follows, with copies of the blobs
/// growing down and their corresponding bookkeeping indices growing up:
/// ```text
/// --------------------------------
///   next available blob ID (4 bytes)
///   blob head offset (4 bytes)
///   ----------------------------
///   blob 0 size (4 bytes)       } <-- bookkeeping index
///   blob 0 offset (4 bytes)     }
///   ...
///   blob N-1 size (4 bytes)
///   blob N-1 offset (4 bytes)
///   ----------------------------
///   zero-initialized memory      <-- remaining bytes available
///   ---------------------------- <-- blob head offset
///   blob N-1
///   ...
///   blob 0
/// --------------------------------
/// ```
///
/// This struct takes care of the atomic nuance required of accessing and updating such a
/// structure.
#[derive(Clone, Copy, Debug)]
pub struct BlobIdAllocator<'a> {
    bytes: &'a [u8],
}

impl<'a> BlobIdAllocator<'a> {
    /// Constructs a view from a byte slice.
    ///
    /// The provided slice must be at least 8-byte-aligned and at least 8 bytes in size.
    pub fn from_slice(slice: &'a [u8]) -> Self {
        debug_assert!(slice.as_ptr().cast::<Header>().is_aligned());
        debug_assert!((Header::SIZE..=u32::MAX as usize).contains(&slice.len()));
        Self { bytes: slice }
    }

    /// Initializes the backing memory as an ID allocator region with no blobs yet mapped,
    /// returning an initialized `BlobIdAllocator`.
    ///
    /// If the region is already known to be zero-filled, `zero_fill` may be `ZeroFill::No`.
    ///
    /// The provided slice must be at least 8-byte-aligned and at least 8 bytes in size.
    pub fn init_from_slice(slice: &'a mut [u8], zero_fill: ZeroFill) -> Self {
        debug_assert!((Header::SIZE..=u32::MAX as usize).contains(&slice.len()));
        if zero_fill == ZeroFill::Yes {
            slice[Header::SIZE..].fill(0);
        }
        let allocator = Self::from_slice(slice);
        allocator.store_header(Header { next_id: 0, blob_head: allocator.bytes.len() as u32 });
        allocator
    }

    /// The next ID to be allocated.
    pub fn next_id(&self) -> u32 {
        self.load_header().next_id
    }

    /// The remaining number of available bytes in the allocator (including those that might be
    /// used for bookkeeping). `None` is returned in the case of an invalid header (see
    /// `AllocateError::InvalidHeader` for more detail).
    pub fn remaining_bytes(&self) -> Option<usize> {
        self.load_header().remaining_bytes(self.bytes.len())
    }

    /// Attempts to store the provided blob and allocate its ID.
    pub fn allocate(&self, blob: &[u8]) -> Result<u32, AllocateError> {
        self.allocate_with(blob.len(), |dest| {
            for (d, s) in dest.iter_mut().zip(blob) {
                d.write(*s);
            }
            Ok::<(), core::convert::Infallible>(())
        })
        .map_err(|e| match e {
            AllocateErrorWith::Error(err) => err,
            AllocateErrorWith::Copy(infallible) => match infallible {},
        })
    }

    /// A variation of the allocation routine that abstracts the representation of the supplied
    /// blob and the manner in which it is copied. This is of particular value to the use of this
    /// library in kernel, which requires care in dealing with user-supplied memory.
    ///
    /// `copy`, which performs the copy of blob to a specified destination, is a callable of input
    /// signature `(dest: &mut [MaybeUninit<u8>]) -> Result<(), E>`.
    pub fn allocate_with<E>(
        &self,
        blob_size: usize,
        copy: impl FnOnce(&mut [MaybeUninit<u8>]) -> Result<(), E>,
    ) -> Result<u32, AllocateErrorWith<E>> {
        let header_atomic = self.header_atomic();
        let mut raw_hdr = header_atomic.load(Ordering::Acquire);
        let (id, offset) = loop {
            let hdr = Header::from_u64(raw_hdr);
            let remaining = hdr
                .remaining_bytes(self.bytes.len())
                .ok_or(AllocateErrorWith::Error(AllocateError::InvalidHeader))?;
            if remaining < Index::SIZE || remaining - Index::SIZE < blob_size {
                return Err(AllocateErrorWith::Error(AllocateError::OutOfMemory));
            }
            let offset = hdr.blob_head - (blob_size as u32);
            let updated = Header { next_id: hdr.next_id + 1, blob_head: offset };
            match header_atomic.compare_exchange_weak(
                raw_hdr,
                updated.to_u64(),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break (hdr.next_id, offset),
                Err(actual) => raw_hdr = actual,
            }
        };

        // We store the blob and then store the index with release semantics to ensure the
        // following:
        // (1) The write of the index stays ordered after the previous store of the blob.
        // (2) The write of the index stays ordered before subsequent reads with acquire semantics.
        //
        // SAFETY: The space check and atomic CAS ensure `[offset, offset + blob_size)` is
        // in-bounds, disjoint from the index table, and claimed exclusively.
        let dest_slice = unsafe {
            let dest_ptr = self.bytes.as_ptr().add(offset as usize).cast_mut().cast();
            slice::from_raw_parts_mut(dest_ptr, blob_size)
        };
        copy(dest_slice).map_err(AllocateErrorWith::Copy)?;

        // Before overwriting, to be safe, check that the index is in the initial state (empty).
        let new_index = Index { size: blob_size as u32, offset };
        // SAFETY: `remaining_bytes` ensured `offset >= index_end`, so `id`'s index slot is
        // in-bounds and disjoint from `dest_slice`.
        unsafe { self.index_atomic(id) }
            .compare_exchange(0, new_index.to_u64(), Ordering::Release, Ordering::Relaxed)
            .map_err(|_| AllocateErrorWith::Error(AllocateError::NonEmptyIndex))?;
        Ok(id)
    }

    /// Returns the blob corresponding to a given ID.
    pub fn get_blob(&self, id: u32) -> Result<&'a [u8], BlobError> {
        let hdr = self.load_header();
        if !hdr.is_valid(self.bytes.len()) {
            return Err(BlobError::InvalidHeader);
        }
        if id >= hdr.next_id {
            return Err(BlobError::UnallocatedId);
        }
        // We load the index with acquire semantics as this ensures the following:
        // (1) The read of the index stays ordered before the subsequent load of the blob.
        // (2) The read of the index stays ordered after previous updates, which were written with
        // release semantics.
        //
        // SAFETY: `hdr.is_valid` and `id < hdr.next_id` ensure `id`'s index slot is in-bounds and
        // disjoint from payload data.
        let index_raw = unsafe { self.index_atomic(id) }.load(Ordering::Acquire);
        if index_raw == 0 {
            return Err(BlobError::UncommittedIndex);
        }
        let index = Index::from_u64(index_raw);
        if !index.is_valid(hdr.blob_head, self.bytes.len()) {
            return Err(BlobError::InvalidIndex);
        }
        let offset = index.offset as usize;
        let size = index.size as usize;
        Ok(&self.bytes[offset..offset + size])
    }

    /// Provides an iterator through all allocated blobs and IDs.
    pub fn iter(&self) -> Iter<'a> {
        Iter { allocator: *self, next_id: 0 }
    }

    fn header_atomic(&self) -> &AtomicU64 {
        // SAFETY:
        // (1) `self.bytes` is at least `Header::SIZE` (8 bytes) and 8-byte aligned per the
        //     preconditions of `from_slice` and `init_from_slice`.
        // (2) The header is exclusively accessed through atomic operations, so casting to a
        //     shared `&AtomicU64` reference is sound.
        unsafe { &*self.bytes.as_ptr().cast::<AtomicU64>() }
    }

    /// Returns a reference to the `AtomicU64` index slot for `id`.
    ///
    /// # Safety
    ///
    /// The caller must ensure `id` corresponds to an in-bounds index slot that does not overlap
    /// with any active mutable borrows.
    unsafe fn index_atomic(&self, id: u32) -> &AtomicU64 {
        let index_offset = (id as usize + 1) * Index::SIZE;
        // SAFETY:
        // (1) The caller guarantees `index_offset` is in-bounds and disjoint from mutable borrows.
        // (2) `self.bytes` is 8-byte aligned and `index_offset` is a multiple of `Index::SIZE`
        //     (8 bytes), so the pointer is properly aligned for `AtomicU64`.
        // (3) The index slot is exclusively accessed through atomic operations.
        unsafe { &*self.bytes.as_ptr().add(index_offset).cast::<AtomicU64>() }
    }

    fn load_header(&self) -> Header {
        Header::from_u64(self.header_atomic().load(Ordering::Relaxed))
    }

    fn store_header(&self, header: Header) {
        self.header_atomic().store(header.to_u64(), Ordering::Release);
    }
}

/// Iterator over blobs in a `BlobIdAllocator`.
pub struct Iter<'a> {
    allocator: BlobIdAllocator<'a>,
    next_id: u32,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Result<(u32, &'a [u8]), BlobError>;

    fn next(&mut self) -> Option<Self::Item> {
        let max_id = self.allocator.next_id();
        if self.next_id >= max_id {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        Some(self.allocator.get_blob(id).map(|blob| (id, blob)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ptr;
    use std::vec::Vec;
    use std::{thread, vec};

    #[test]
    fn test_header_and_index() {
        let hdr = Header { next_id: 42, blob_head: 4096 };
        let raw = hdr.to_u64();
        let decoded = Header::from_u64(raw);
        assert_eq!(hdr, decoded);

        assert_eq!(Header { next_id: 0, blob_head: 4096 }.index_end(), Some(8));
        assert_eq!(Header { next_id: 1, blob_head: 4096 }.index_end(), Some(16));
        assert_eq!(Header { next_id: 5, blob_head: 4096 }.index_end(), Some(48));

        const MAX_NEXT_ID: u32 = (u32::MAX - 8) / 8;
        assert_eq!(
            Header { next_id: MAX_NEXT_ID, blob_head: 4096 }.index_end(),
            Some(8 + MAX_NEXT_ID * 8)
        );
        assert_eq!(Header { next_id: MAX_NEXT_ID + 1, blob_head: 4096 }.index_end(), None);

        assert_eq!(Header { next_id: 0, blob_head: 1024 }.remaining_bytes(1024), Some(1024 - 8));
        assert_eq!(Header { next_id: 1, blob_head: 1000 }.remaining_bytes(1024), Some(1000 - 16));
        assert_eq!(Header { next_id: 0, blob_head: 8 }.remaining_bytes(8), Some(0));
        assert_eq!(Header { next_id: 1, blob_head: 15 }.remaining_bytes(1024), None);
        assert_eq!(Header { next_id: 0, blob_head: 2000 }.remaining_bytes(1024), None);

        let idx = Index { size: 100, offset: 500 };
        let raw_idx = idx.to_u64();
        assert_eq!(raw_idx, 100 | (500u64 << 32));
        assert_eq!(Index::from_u64(raw_idx), idx);
        assert!(idx.is_valid(400, 1024));
        assert!(!idx.is_valid(600, 1024));
        assert!(!idx.is_valid(400, 550));
    }

    #[test]
    fn test_single_threaded() {
        let blob_a = [b'a'; 51];
        let blob_b = [b'b'; 17];
        let blob_c = [b'c'; 1];

        let mut buffer = [0u64; 100 / 8 + 1];
        let slice = unsafe { slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), 100) };
        let allocator = BlobIdAllocator::init_from_slice(slice, ZeroFill::Yes);

        assert_eq!(allocator.remaining_bytes(), Some(92)); // 100 - 8 (header)

        let id_a = allocator.allocate(&blob_a).expect("allocate A");
        assert_eq!(id_a, 0);
        assert_eq!(allocator.remaining_bytes(), Some(33)); // 92 - 8 (index) - 51 (blob)

        let res_a = allocator.get_blob(0).expect("get A");
        assert_eq!(res_a, &blob_a[..]);

        let id_b = allocator.allocate(&blob_b).expect("allocate B");
        assert_eq!(id_b, 1);
        assert_eq!(allocator.remaining_bytes(), Some(8)); // 33 - 8 (index) - 17 (blob)

        let res_b = allocator.get_blob(1).expect("get B");
        assert_eq!(res_b, &blob_b[..]);

        // Allocating blob C requires 8 (index) + 1 (data) = 9 bytes, but only 8 remain.
        assert_eq!(allocator.allocate(&blob_c), Err(AllocateError::OutOfMemory));

        let items: Vec<(u32, &[u8])> = allocator.iter().map(Result::unwrap).collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], (0, &blob_a[..]));
        assert_eq!(items[1], (1, &blob_b[..]));
    }

    #[test]
    fn test_multi_threaded() {
        const NUM_THREADS: usize = 100;
        const BUF_SIZE: usize = 8 + NUM_THREADS * 8 + NUM_THREADS * 1;
        let mut buffer = vec![0u64; BUF_SIZE / 8 + 1];
        let slice =
            unsafe { slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), BUF_SIZE) };
        let allocator = BlobIdAllocator::init_from_slice(slice, ZeroFill::Yes);

        let mut ids: Vec<u32> = thread::scope(|s| {
            let mut handles = Vec::with_capacity(NUM_THREADS);
            for i in 0..NUM_THREADS {
                handles.push(s.spawn(move || {
                    let byte = [i as u8];
                    allocator.allocate(&byte).expect("allocate in thread")
                }));
            }
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        ids.sort();
        for (expected, actual) in ids.iter().enumerate() {
            assert_eq!(expected as u32, *actual);
        }

        let mut blob_values: Vec<u8> = allocator
            .iter()
            .map(|res| {
                let (_id, blob) = res.unwrap();
                assert_eq!(blob.len(), 1);
                blob[0]
            })
            .collect();
        assert_eq!(blob_values.len(), NUM_THREADS);
        blob_values.sort();
        for (i, val) in blob_values.iter().enumerate() {
            assert_eq!(i as u8, *val);
        }
    }

    #[test]
    fn test_corruption_detection() {
        let mut buffer = [0u64; 16];
        let slice = unsafe { slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), 128) };
        let allocator = BlobIdAllocator::init_from_slice(slice, ZeroFill::No);

        // Corrupt header by setting blob_head < index_end
        let corrupted_hdr = Header { next_id: 1, blob_head: 8 };
        unsafe {
            ptr::write_volatile(buffer.as_mut_ptr(), corrupted_hdr.to_u64());
        }

        assert_eq!(allocator.allocate(&[0u8; 4]), Err(AllocateError::InvalidHeader));
        assert_eq!(allocator.get_blob(0), Err(BlobError::InvalidHeader));

        // Re-initialize and corrupt index slot to cause CAS failure
        let slice = unsafe { slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), 128) };
        let allocator = BlobIdAllocator::init_from_slice(slice, ZeroFill::No);
        unsafe {
            // Slot for index 0 is at offset 8 (index 1 in u64 array). Write non-zero value to it.
            ptr::write_volatile(buffer.as_mut_ptr().add(1), 0x1234);
        }
        assert_eq!(allocator.allocate(&[0u8; 4]), Err(AllocateError::NonEmptyIndex));
    }

    #[test]
    fn test_overflowing_next_id_is_invalid() {
        let mut buffer = [0u64; 512];
        let slice = unsafe { slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), 4096) };
        let allocator = BlobIdAllocator::init_from_slice(slice, ZeroFill::Yes);

        // Corrupt next_id to the overflow-triggering value.
        // next_id = 0x20000000 causes: (uint32_t)(8 + 0x20000000*8) = 8 (wraps!)
        unsafe {
            let raw = buffer.as_mut_ptr() as *mut u32;
            ptr::write_volatile(raw, 0x2000_0000);
        }

        assert_eq!(allocator.remaining_bytes(), None);
        assert_eq!(allocator.allocate(&[0u8; 8]), Err(AllocateError::InvalidHeader));
        assert_eq!(allocator.get_blob(0), Err(BlobError::InvalidHeader));
    }
}
