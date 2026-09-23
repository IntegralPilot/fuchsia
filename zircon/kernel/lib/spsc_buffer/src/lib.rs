// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![no_std]

use core::sync::atomic::{AtomicU64, Ordering};
use core::{cmp, ptr, slice};
pub use kalloc::NoOpAllocator;
use kalloc::{Allocator, Box, DefaultAllocator};
use zx_status::Status;

/// A simple convenience type used to hold the read and write pointers as separate values.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct RingPointers {
    read: u32,
    write: u32,
}

impl RingPointers {
    /// Constructs a new `RingPointers` with the given read and write offsets.
    const fn new(read: u32, write: u32) -> Self {
        Self { read, write }
    }

    /// Splits combined 64-bit pointers into individual read and write pointers.
    const fn from_combined(combined: u64) -> Self {
        Self::new((combined >> 32) as u32, combined as u32)
    }

    /// Combines read and write pointers into a single 64-bit value.
    const fn as_combined(&self) -> u64 {
        ((self.read as u64) << 32) | (self.write as u64)
    }

    /// Returns the amount of data available to read in the buffer.
    const fn available_data(&self) -> u32 {
        self.write.wrapping_sub(self.read)
    }
}

/// A wrapper around a slot of memory in the ring buffer.
///
/// A Reservation has a predetermined size that is determined by the size passed into `reserve`. Any
/// attempt to write more than this amount of data into the slot is a programming error and will
/// cause an assertion failure. This class provides a formal way for writers to serialize data in
/// place in the ring buffer, thus eliminating the need for a temporary serialization buffer.
#[derive(Debug)]
pub struct Reservation<'a> {
    combined_pointers: &'a AtomicU64,
    storage_len: u32,
    initial_ring_pointers: RingPointers,
    region1: &'a mut [u8],
    region2: &'a mut [u8],
    write_offset: u32,
    committed: bool,
}

impl<'a> Drop for Reservation<'a> {
    fn drop(&mut self) {
        debug_assert!(self.committed, "Reservation dropped without being committed");
    }
}

impl<'a> Reservation<'a> {
    /// Writes the given data into this reservation.
    pub fn write(&mut self, data: &[u8]) -> Result<(), Status> {
        if self.committed {
            return Err(Status::BAD_STATE);
        }

        let mut bytes_to_copy = data.len();
        let mut region1_copy_amount = 0;
        let mut write_offset = self.write_offset as usize;

        let region1_len = self.region1.len();
        if write_offset < region1_len {
            let space_left_in_region1 = region1_len - write_offset;
            region1_copy_amount = cmp::min(bytes_to_copy, space_left_in_region1);

            self.region1[write_offset..write_offset + region1_copy_amount]
                .copy_from_slice(&data[..region1_copy_amount]);

            write_offset += region1_copy_amount;
            bytes_to_copy -= region1_copy_amount;
        }

        if bytes_to_copy > 0 {
            if write_offset < region1_len {
                return Err(Status::BAD_STATE);
            }
            let region2_len = self.region2.len();
            let region2_offset = write_offset - region1_len;
            if region2_len < region2_offset {
                return Err(Status::BAD_STATE);
            }
            if region2_len - region2_offset < bytes_to_copy {
                return Err(Status::BUFFER_TOO_SMALL);
            }

            self.region2[region2_offset..region2_offset + bytes_to_copy]
                .copy_from_slice(&data[region1_copy_amount..region1_copy_amount + bytes_to_copy]);

            write_offset += bytes_to_copy;
        }

        self.write_offset = write_offset as u32;
        Ok(())
    }

    /// Advances the write pointer of the associated spsc buffer.
    ///
    /// This makes the written data visible to the reader, and thus can only be called once all
    /// writes have been completed and the reservation is fully written.
    pub fn commit(mut self) -> Result<(), Status> {
        if self.committed {
            return Err(Status::BAD_STATE);
        }
        self.committed = true;

        let total_len =
            self.region1.len().checked_add(self.region2.len()).ok_or(Status::BAD_STATE)? as u32;
        if self.write_offset != total_len {
            return Err(Status::BAD_STATE);
        }

        advance_write_pointer(
            self.combined_pointers,
            self.storage_len,
            self.initial_ring_pointers,
            total_len,
        )
    }

    /// Cancels this reservation, allowing it to be safely dropped without advancing the write
    /// pointer.
    pub fn cancel(mut self) {
        self.committed = true;
    }
}

/// A transactional, single-producer, single-consumer ring buffer.
///
/// The caller is responsible for ensuring that there is only one reader and one writer; no internal
/// synchronization is provided to enforce this constraint.
///
/// Backing storage is allocated dynamically during the `init` method. The requested size must be a
/// power of two for correct functionality.
// TODO(https://fxbug.dev/517301686): Use bindgen or another systematic way to avoid duplicating
// this structure and causing drift.
#[repr(C, align(8))]
pub struct Buffer<A: Allocator + Default = DefaultAllocator> {
    // The read and write pointers are stored as the upper and lower halves, respectively, of a
    // single 64-bit atomic.
    combined_pointers: AtomicU64,
    // The types used for `storage` and `size` must match those of ktl::span.
    storage: *mut u8,
    // The size of the backing storage in bytes. This is enforced to be at most
    // MAX_STORAGE_SIZE (2 GiB), meaning this value will never exceed u32::MAX.
    size: usize,
    _phantom: core::marker::PhantomData<A>,
}

impl<A: Allocator + Default> Drop for Buffer<A> {
    fn drop(&mut self) {
        if !self.storage.is_null() {
            let slice_ptr = ptr::slice_from_raw_parts_mut(self.storage, self.size);
            unsafe {
                let _ = Box::from_raw_in(slice_ptr, A::default());
            }
        }
    }
}

impl<A: Allocator + Default> Buffer<A> {
    /// Maximum size of backing storage buffer (2 GiB).
    const MAX_STORAGE_SIZE: u32 = 1 << 31;

    /// Constructs a new `Buffer` with a dynamically allocated backing storage of the given size,
    /// using the given allocator.
    pub fn try_new_in(size: u32, allocator: A) -> Result<Self, Status> {
        if size > Self::MAX_STORAGE_SIZE {
            return Err(Status::INVALID_ARGS);
        }
        if !size.is_power_of_two() {
            return Err(Status::INVALID_ARGS);
        }

        let storage_box = Box::<[u8], A>::try_new_zeroed_slice_in(size as usize, allocator)
            .map_err(|_| Status::NO_MEMORY)?;
        let (storage_ptr, _) = Box::into_raw_with_allocator(storage_box);

        Ok(Self {
            combined_pointers: AtomicU64::new(0),
            storage: storage_ptr as *mut u8,
            size: size as usize,
            _phantom: core::marker::PhantomData,
        })
    }

    /// Returns true if the buffer has valid backing storage (non-null and valid power-of-two size).
    pub fn is_valid(&self) -> bool {
        !self.storage.is_null()
            && self.size > 0
            && self.size <= Self::MAX_STORAGE_SIZE as usize
            && self.size.is_power_of_two()
    }

    /// Returns the size of the backing storage.
    pub fn size(&self) -> u32 {
        self.size as u32
    }

    /// Reserves a block of the given size in the buffer.
    ///
    /// Any data written into this block will not be visible to readers until `commit` is called on
    /// the returned `Reservation`.
    pub fn reserve(&mut self, size: u32) -> Result<Reservation<'_>, Status> {
        if size == 0 || size > Self::MAX_STORAGE_SIZE {
            return Err(Status::INVALID_ARGS);
        }

        if self.storage.is_null() {
            return Err(Status::BAD_STATE);
        }

        let storage_len = self.size as u32;
        if size > storage_len {
            return Err(Status::NO_SPACE);
        }

        let initial_state = self.load_pointers();
        let available_space = self.available_space(initial_state);
        if available_space < size {
            return Err(Status::NO_SPACE);
        }

        let write_offset = self.pointer_to_offset(initial_state.write);
        let ring_break_distance = storage_len - write_offset;
        let bytes_before_break = cmp::min(size, ring_break_distance);

        // SAFETY: The creator of Buffer must ensure that `storage` points to a valid memory region
        // of `size` bytes.
        let storage_slice = unsafe { slice::from_raw_parts_mut(self.storage, self.size) };
        let (left, right) = storage_slice.split_at_mut(write_offset as usize);
        let region1 = &mut right[..bytes_before_break as usize];

        let region2 = if bytes_before_break < size {
            let region2_len = size - bytes_before_break;
            &mut left[..region2_len as usize]
        } else {
            &mut []
        };

        Ok(Reservation {
            combined_pointers: &self.combined_pointers,
            storage_len,
            initial_ring_pointers: initial_state,
            region1,
            region2,
            write_offset: 0,
            committed: false,
        })
    }

    /// Copies `len` bytes out of the buffer using the provided `copy_fn`.
    ///
    /// The copy function has the signature `copy_fn(offset: u32, src: &[u8]) -> Result<(), Status>`
    /// and may be invoked multiple times.
    ///
    /// Returns the number of bytes read on success. If `copy_fn` returns an error, that error is
    /// propagated.
    ///
    /// Importantly, even if an error is returned, `copy_fn` might have already processed a partial
    /// amount of data (between 0 and `len` bytes). However, these partially processed bytes are
    /// considered *not read*. Consequently, the internal read pointer of the ring buffer will *not*
    /// be advanced for these unread bytes, meaning that these same bytes will remain available for
    /// reading in subsequent calls to `read`.
    pub fn read<F>(&self, mut copy_fn: F, len: u32) -> Result<u32, Status>
    where
        F: FnMut(u32, &[u8]) -> Result<(), Status>,
    {
        if len > Self::MAX_STORAGE_SIZE {
            return Err(Status::INVALID_ARGS);
        }
        if len == 0 {
            return Ok(0);
        }

        if self.storage.is_null() {
            return Err(Status::BAD_STATE);
        }

        let initial_state = self.load_pointers();
        let available_data = initial_state.available_data();
        if available_data == 0 {
            return Ok(0);
        }

        let amount_to_copy = cmp::min(available_data, len) as usize;
        let read_offset = self.pointer_to_offset(initial_state.read) as usize;
        let ring_break_distance = self.size - read_offset;
        let bytes_before_break = cmp::min(amount_to_copy, ring_break_distance);

        // SAFETY: The creator of Buffer must ensure that `storage` points to a valid memory region
        // of `size` bytes.
        let storage_slice = unsafe { slice::from_raw_parts(self.storage, self.size) };
        let slice1 = &storage_slice[read_offset..read_offset + bytes_before_break];
        copy_fn(0, slice1)?;

        if bytes_before_break < amount_to_copy {
            let bytes_after_break = amount_to_copy - bytes_before_break;
            let slice2 = &storage_slice[..bytes_after_break];
            copy_fn(bytes_before_break as u32, slice2)?;
        }

        self.advance_read_pointer(initial_state, amount_to_copy as u32)?;
        Ok(amount_to_copy as u32)
    }

    /// Empties the contents of the buffer.
    ///
    /// This is logically a read operation, so a `read` and `drain` cannot be called concurrently.
    /// Additionally, the behavior of this method is non-deterministic if a write is in-progress.
    pub fn drain(&self) -> Result<(), Status> {
        let initial_state = self.load_pointers();
        let available_data = initial_state.available_data();
        if available_data == 0 {
            return Ok(());
        }
        self.advance_read_pointer(initial_state, available_data)
    }

    // Helper function that converts read and write pointers into ring buffer offsets.
    //
    // This is logically performing pointer % storage.len(), but because storage.len() is guaranteed
    // to be a power of two it is equivalent to this logical AND.
    fn pointer_to_offset(&self, pointer: u32) -> u32 {
        let storage_len = self.size as u32;
        pointer & (storage_len - 1)
    }

    /// Returns the remaining available space in the buffer.
    fn available_space(&self, pointers: RingPointers) -> u32 {
        let storage_len = self.size as u32;
        storage_len.wrapping_sub(pointers.available_data())
    }

    /// Loads the current values of the read and write pointers.
    fn load_pointers(&self) -> RingPointers {
        let combined = self.combined_pointers.load(Ordering::Acquire);
        RingPointers::from_combined(combined)
    }

    // Adds the given delta to the read half of the combined_pointers.
    //
    // Because we store the pointers in a single combined atomic variable, we must update the entire
    // combined pointer. We perform this update using a compare and exchange to ensure that
    // concurrent operations to the write half of the combined pointers are preserved. We also check
    // to ensure reads do not encounter concurrent reads.
    //
    // This is a store-release operation that synchronizes with the load-acquire in load_pointers.
    // By using release semantics, we ensure that if the updated value is seen in load_pointers, all
    // memory operations that occurred prior to this update are observable.
    fn advance_read_pointer(&self, initial: RingPointers, delta: u32) -> Result<(), Status> {
        if delta > initial.available_data() {
            return Err(Status::INVALID_ARGS);
        }

        let target_read = initial.read.wrapping_add(delta);
        let mut starting_pointers = initial.as_combined();
        let mut target_pointers = RingPointers::new(target_read, initial.write).as_combined();

        loop {
            match self.combined_pointers.compare_exchange_weak(
                starting_pointers,
                target_pointers,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed_combined) => {
                    starting_pointers = observed_combined;
                    let observed = RingPointers::from_combined(observed_combined);
                    debug_assert_eq!(
                        observed.read, initial.read,
                        "potential concurrent read detected; expected read pointer {}, got {}",
                        initial.read, observed.read
                    );
                    target_pointers = RingPointers::new(target_read, observed.write).as_combined();
                }
            }
        }
        Ok(())
    }
}

// Adds the given delta to the write half of the combined_pointers.
//
// Because we store the pointers in a single combined atomic variable, we must update the entire
// combined pointer. We perform this update using a compare and exchange to ensure that concurrent
// operations to the read half of the combined pointers are preserved. We also check to ensure
// writes do not encounter concurrent writes.
//
// This is a store-release operation that synchronizes with the load-acquire in load_pointers. By
// using release semantics, we ensure that if the updated value is seen in load_pointers, all memory
// operations that occurred prior to this update are observable.
fn advance_write_pointer(
    combined_pointers: &AtomicU64,
    storage_len: u32,
    initial: RingPointers,
    delta: u32,
) -> Result<(), Status> {
    let available_data = initial.available_data();
    if delta > storage_len.checked_sub(available_data).ok_or(Status::INVALID_ARGS)? {
        return Err(Status::INVALID_ARGS);
    }

    let target_write = initial.write.wrapping_add(delta);
    let mut starting_pointers = initial.as_combined();
    let mut target_pointers = RingPointers::new(initial.read, target_write).as_combined();

    loop {
        match combined_pointers.compare_exchange_weak(
            starting_pointers,
            target_pointers,
            Ordering::Release,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed_combined) => {
                starting_pointers = observed_combined;
                let observed = RingPointers::from_combined(observed_combined);
                debug_assert_eq!(
                    observed.write, initial.write,
                    "potential concurrent write detected; expected write pointer {}, got {}",
                    initial.write, observed.write
                );
                target_pointers = RingPointers::new(observed.read, target_write).as_combined();
            }
        }
    }
    Ok(())
}

impl Buffer<DefaultAllocator> {
    /// Constructs a new `Buffer` with a dynamically allocated backing storage of the given size,
    /// using the default allocator.
    pub fn try_new(size: u32) -> Result<Self, Status> {
        Self::try_new_in(size, DefaultAllocator)
    }
}

impl Buffer<NoOpAllocator> {
    /// Constructs a `Buffer` from raw pointers using a no-op allocator.
    ///
    /// The returned buffer does not own the memory and will not deallocate it when dropped.
    ///
    /// # Safety
    ///
    /// - `storage` must be non-null and point to a valid, initialized slice of bytes whose length
    ///   is a power of two and does not exceed `MAX_STORAGE_SIZE`.
    pub unsafe fn from_raw_parts(storage: *mut u8, size: usize) -> Self {
        assert!(!storage.is_null());
        assert!(size <= Self::MAX_STORAGE_SIZE as usize);
        assert!(size.is_power_of_two());
        Self {
            combined_pointers: AtomicU64::new(0),
            storage,
            size,
            _phantom: core::marker::PhantomData,
        }
    }

    /// Constructs an empty `Buffer` with null storage and zero size (for testing).
    pub const fn empty() -> Self {
        Self {
            combined_pointers: AtomicU64::new(0),
            storage: core::ptr::null_mut(),
            size: 0,
            _phantom: core::marker::PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_pointers_from_combined() {
        struct TestCase {
            combined_pointers: u64,
            expected: RingPointers,
        }
        let test_cases = [
            TestCase {
                combined_pointers: 0x01320087_01005382,
                expected: RingPointers::new(0x1320087, 0x1005382),
            },
            TestCase { combined_pointers: 0, expected: RingPointers::new(0, 0) },
            TestCase {
                combined_pointers: 0xFFFFFFFF_00004587,
                expected: RingPointers::new(0xFFFFFFFF, 0x4587),
            },
            TestCase {
                combined_pointers: 0x00004587_FFFFFFFF,
                expected: RingPointers::new(0x4587, 0xFFFFFFFF),
            },
        ];

        for tc in &test_cases {
            let actual = RingPointers::from_combined(tc.combined_pointers);
            assert_eq!(tc.expected, actual);
        }
    }

    #[test]
    fn test_ring_pointers_as_combined() {
        struct TestCase {
            pointers: RingPointers,
            combined: u64,
        }
        let test_cases = [
            TestCase {
                pointers: RingPointers::new(0x1320087, 0x1005382),
                combined: 0x01320087_01005382,
            },
            TestCase { pointers: RingPointers::new(0, 0), combined: 0 },
            TestCase {
                pointers: RingPointers::new(0xFFFFFFFF, 0x123),
                combined: 0xFFFFFFFF_00000123,
            },
            TestCase {
                pointers: RingPointers::new(0x123, 0xFFFFFFFF),
                combined: 0x00000123_FFFFFFFF,
            },
        ];

        for tc in &test_cases {
            let actual = tc.pointers.as_combined();
            assert_eq!(tc.combined, actual);
        }
    }

    #[test]
    fn test_available_space() {
        struct TestCase {
            pointers: RingPointers,
            buffer_size: u32,
            expected: u32,
        }
        let test_cases = [
            TestCase { pointers: RingPointers::new(0, 0), buffer_size: 16, expected: 16 },
            TestCase { pointers: RingPointers::new(3, 3), buffer_size: 16, expected: 16 },
            TestCase { pointers: RingPointers::new(3, 7), buffer_size: 16, expected: 12 },
            TestCase { pointers: RingPointers::new(0xFFFFFFFC, 3), buffer_size: 16, expected: 9 },
            TestCase { pointers: RingPointers::new(0, 16), buffer_size: 16, expected: 0 },
        ];

        for tc in &test_cases {
            let buffer = Buffer::try_new(tc.buffer_size).unwrap();
            let actual = buffer.available_space(tc.pointers);
            assert_eq!(tc.expected, actual);
        }
    }

    #[test]
    fn test_ring_pointers_available_data() {
        struct TestCase {
            pointers: RingPointers,
            expected: u32,
        }
        let test_cases = [
            TestCase { pointers: RingPointers::new(0, 0), expected: 0 },
            TestCase { pointers: RingPointers::new(3, 3), expected: 0 },
            TestCase { pointers: RingPointers::new(3, 7), expected: 4 },
            TestCase { pointers: RingPointers::new(0, 16), expected: 16 },
            TestCase { pointers: RingPointers::new(0xFFFFFFFC, 3), expected: 7 },
        ];

        for tc in &test_cases {
            let actual = tc.pointers.available_data();
            assert_eq!(tc.expected, actual);
        }
    }

    #[test]
    fn test_advance_read_pointer() {
        struct TestCase {
            initial_pointers: RingPointers,
            buffer_size: u32,
            delta: u32,
            expected: u64,
        }
        let test_cases = [
            TestCase {
                initial_pointers: RingPointers::new(1, 6),
                buffer_size: 16,
                delta: 4,
                expected: 0x5_00000006,
            },
            TestCase {
                initial_pointers: RingPointers::new(0, 16),
                buffer_size: 16,
                delta: 16,
                expected: 0x10_00000010,
            },
            TestCase {
                initial_pointers: RingPointers::new(0xFFFFFFFC, 12),
                buffer_size: 16,
                delta: 5,
                expected: 0x1_0000000C,
            },
        ];

        for tc in &test_cases {
            let buffer = Buffer::try_new(tc.buffer_size).unwrap();

            let initial = tc.initial_pointers.as_combined();
            buffer.combined_pointers.store(initial, Ordering::Release);

            buffer.advance_read_pointer(tc.initial_pointers, tc.delta).unwrap();
            let actual = buffer.combined_pointers.load(Ordering::Acquire);
            assert_eq!(tc.expected, actual);
        }
    }

    #[test]
    fn test_advance_write_pointer() {
        struct TestCase {
            initial_pointers: RingPointers,
            buffer_size: u32,
            delta: u32,
            expected: u64,
        }
        let test_cases = [
            TestCase {
                initial_pointers: RingPointers::new(7, 9),
                buffer_size: 16,
                delta: 4,
                expected: 0x7_0000000D,
            },
            TestCase {
                initial_pointers: RingPointers::new(0, 0),
                buffer_size: 16,
                delta: 16,
                expected: 0x10,
            },
            TestCase {
                initial_pointers: RingPointers::new(0xFFFFFFF1, 0xFFFFFFFC),
                buffer_size: 16,
                delta: 5,
                expected: 0xFFFFFFF100000001,
            },
        ];

        for tc in &test_cases {
            let buffer = Buffer::try_new(tc.buffer_size).unwrap();

            let initial = tc.initial_pointers.as_combined();
            buffer.combined_pointers.store(initial, Ordering::Release);

            advance_write_pointer(
                &buffer.combined_pointers,
                buffer.size as u32,
                tc.initial_pointers,
                tc.delta,
            )
            .unwrap();
            let actual = buffer.combined_pointers.load(Ordering::Acquire);
            assert_eq!(tc.expected, actual);
        }
    }

    #[test]
    fn test_try_new() {
        // Happy case
        {
            assert!(Buffer::try_new(256).is_ok());
        }

        // Calling try_new with too big of a size should fail
        {
            assert_eq!(Buffer::try_new(u32::MAX).err().unwrap(), Status::INVALID_ARGS);
        }

        // Calling try_new with a size that is not a power of two should fail
        {
            assert_eq!(Buffer::try_new(100).err().unwrap(), Status::INVALID_ARGS);
        }

        // try_new should propagate allocation failures
        {
            assert_eq!(Buffer::try_new_in(256, NoOpAllocator).err().unwrap(), Status::NO_MEMORY);
        }
    }

    #[test]
    fn test_read_write_single_threaded() {
        const STORAGE_SIZE: usize = 256;
        let mut src = [0u8; STORAGE_SIZE];
        for i in 0..STORAGE_SIZE {
            src[i] = (i * 17 + 5) as u8;
        }

        struct TestCase {
            write_size: u32,
            read_size: u32,
            expected_read_size: u32,
            expected_reserve_status: Result<(), Status>,
            expected_read_status: Result<(), Status>,
            initial_pointers: RingPointers,
            use_copy_out_err_fn: bool,
        }

        let test_cases = [
            TestCase {
                write_size: (STORAGE_SIZE / 2) as u32,
                read_size: (STORAGE_SIZE / 2) as u32,
                expected_read_size: (STORAGE_SIZE / 2) as u32,
                expected_reserve_status: Ok(()),
                expected_read_status: Ok(()),
                initial_pointers: RingPointers::new(0, 0),
                use_copy_out_err_fn: false,
            },
            TestCase {
                write_size: (STORAGE_SIZE / 2) as u32,
                read_size: (STORAGE_SIZE / 4) as u32,
                expected_read_size: (STORAGE_SIZE / 4) as u32,
                expected_reserve_status: Ok(()),
                expected_read_status: Ok(()),
                initial_pointers: RingPointers::new(0, 0),
                use_copy_out_err_fn: false,
            },
            TestCase {
                write_size: STORAGE_SIZE as u32,
                read_size: STORAGE_SIZE as u32,
                expected_read_size: STORAGE_SIZE as u32,
                expected_reserve_status: Ok(()),
                expected_read_status: Ok(()),
                initial_pointers: RingPointers::new(0, 0),
                use_copy_out_err_fn: false,
            },
            TestCase {
                write_size: (STORAGE_SIZE / 4) as u32,
                read_size: (STORAGE_SIZE / 2) as u32,
                expected_read_size: (STORAGE_SIZE / 4) as u32,
                expected_reserve_status: Ok(()),
                expected_read_status: Ok(()),
                initial_pointers: RingPointers::new(0, 0),
                use_copy_out_err_fn: false,
            },
            TestCase {
                write_size: STORAGE_SIZE as u32,
                read_size: STORAGE_SIZE as u32,
                expected_read_size: STORAGE_SIZE as u32,
                expected_reserve_status: Ok(()),
                expected_read_status: Ok(()),
                initial_pointers: RingPointers::new(
                    (STORAGE_SIZE / 2) as u32,
                    (STORAGE_SIZE / 2) as u32,
                ),
                use_copy_out_err_fn: false,
            },
            TestCase {
                write_size: STORAGE_SIZE as u32,
                read_size: STORAGE_SIZE as u32,
                expected_read_size: STORAGE_SIZE as u32,
                expected_reserve_status: Ok(()),
                expected_read_status: Ok(()),
                initial_pointers: RingPointers::new(0xFFFFFFFA, 0xFFFFFFFA),
                use_copy_out_err_fn: false,
            },
            TestCase {
                write_size: 64,
                read_size: 0,
                expected_read_size: 0,
                expected_reserve_status: Err(Status::NO_SPACE),
                expected_read_status: Ok(()),
                initial_pointers: RingPointers::new(0, (STORAGE_SIZE - 48) as u32),
                use_copy_out_err_fn: false,
            },
            TestCase {
                write_size: STORAGE_SIZE as u32,
                read_size: (STORAGE_SIZE / 2) as u32,
                expected_read_size: 0,
                expected_reserve_status: Ok(()),
                expected_read_status: Err(Status::BAD_STATE),
                initial_pointers: RingPointers::new(0, 0),
                use_copy_out_err_fn: true,
            },
        ];

        for tc in &test_cases {
            let mut dst = [0u8; STORAGE_SIZE];

            let mut spsc = Buffer::try_new(STORAGE_SIZE as u32).unwrap();

            let starting_pointers = tc.initial_pointers.as_combined();
            spsc.combined_pointers.store(starting_pointers, Ordering::Release);

            let reservation = spsc.reserve(tc.write_size);
            if let Err(e) = &tc.expected_reserve_status {
                match reservation {
                    Err(actual_err) => assert_eq!(actual_err, *e),
                    Ok(_) => panic!("expected reserve to fail with {:?}, but it succeeded", e),
                }
                continue;
            }
            let mut reservation = reservation.unwrap();

            reservation.write(&src[..tc.write_size as usize]).unwrap();
            reservation.commit().unwrap();

            let copy_out_fn = |offset: u32, src_slice: &[u8]| -> Result<(), Status> {
                let offset = offset as usize;
                assert!(offset + src_slice.len() <= dst.len());
                dst[offset..offset + src_slice.len()].copy_from_slice(src_slice);
                Ok(())
            };

            let copy_out_err_fn =
                |_offset: u32, _src_slice: &[u8]| -> Result<(), Status> { Err(Status::BAD_STATE) };

            let read_result = if tc.use_copy_out_err_fn {
                spsc.read(copy_out_err_fn, tc.read_size)
            } else {
                spsc.read(copy_out_fn, tc.read_size)
            };

            if let Err(e) = &tc.expected_read_status {
                assert_eq!(read_result.unwrap_err(), *e);
                assert_eq!(spsc.load_pointers().available_data(), tc.write_size);
                continue;
            }

            let read_bytes = read_result.unwrap();
            assert_eq!(read_bytes, tc.expected_read_size);
            assert_eq!(
                &dst[..tc.expected_read_size as usize],
                &src[..tc.expected_read_size as usize]
            );
        }
    }

    #[test]
    fn test_drain() {
        const STORAGE_SIZE: u32 = 256;
        let mut spsc = Buffer::try_new(STORAGE_SIZE).unwrap();

        let mut reservation = spsc.reserve(STORAGE_SIZE / 2).unwrap();
        let write_data = [b'f'; (STORAGE_SIZE / 2) as usize];
        reservation.write(&write_data).unwrap();
        reservation.commit().unwrap();

        assert_eq!(spsc.load_pointers().available_data(), STORAGE_SIZE / 2);

        spsc.drain().unwrap();
        assert_eq!(spsc.load_pointers().available_data(), 0);
    }

    #[test]
    fn test_commit_error() {
        const STORAGE_SIZE: u32 = 256;
        let mut spsc = Buffer::try_new(STORAGE_SIZE).unwrap();

        let mut reservation = spsc.reserve(STORAGE_SIZE / 2).unwrap();
        // Write fewer bytes than reserved.
        let write_data = [b'f'; (STORAGE_SIZE / 2) as usize - 1];
        reservation.write(&write_data).unwrap();
        assert_eq!(reservation.commit(), Err(Status::BAD_STATE));
    }

    #[test]
    fn test_write_error() {
        const STORAGE_SIZE: u32 = 256;
        let mut spsc = Buffer::try_new(STORAGE_SIZE).unwrap();

        let mut reservation = spsc.reserve(STORAGE_SIZE / 2).unwrap();
        // Write more bytes than reserved.
        let write_data = [b'f'; (STORAGE_SIZE / 2) as usize + 1];
        assert_eq!(reservation.write(&write_data), Err(Status::BUFFER_TOO_SMALL));
        reservation.committed = true;
    }

    #[test]
    fn test_from_raw_parts() {
        let mut mock_storage = [0u8; 256];

        // Safety: Pointers are valid.
        let mut spsc =
            unsafe { Buffer::from_raw_parts(mock_storage.as_mut_ptr(), mock_storage.len()) };

        // Verify reserve, write, commit works
        let mut reservation = spsc.reserve(100).unwrap();
        let write_data = [b'x'; 100];
        reservation.write(&write_data).unwrap();
        reservation.commit().unwrap();

        assert_eq!(spsc.load_pointers().available_data(), 100);
        assert_eq!(spsc.combined_pointers.load(Ordering::Relaxed) & 0xffffffff, 100); // write pointer is 100
        assert_eq!(&mock_storage[..100], &write_data[..]);
    }

    #[test]
    fn test_reserve_zero() {
        const STORAGE_SIZE: u32 = 256;
        let mut spsc = Buffer::try_new(STORAGE_SIZE).unwrap();
        match spsc.reserve(0) {
            Err(e) => assert_eq!(e, Status::INVALID_ARGS),
            Ok(_) => panic!("reserve(0) should fail with INVALID_ARGS"),
        }
    }

    #[test]
    fn test_reserve_too_large() {
        const STORAGE_SIZE: u32 = 256;
        let mut spsc = Buffer::try_new(STORAGE_SIZE).unwrap();
        match spsc.reserve(u32::MAX) {
            Err(e) => assert_eq!(e, Status::INVALID_ARGS),
            Ok(_) => panic!("reserve(u32::MAX) should fail with INVALID_ARGS"),
        }
    }

    #[test]
    fn test_reserve_and_read_null_storage() {
        let mut spsc = Buffer::<NoOpAllocator>::empty();

        assert!(!spsc.is_valid());
        assert_eq!(spsc.reserve(16).err(), Some(Status::BAD_STATE));
        let read_result = spsc.read(|_, _| Ok(()), 16);
        assert_eq!(read_result.err(), Some(Status::BAD_STATE));
    }

    #[test]
    fn test_reserve_at_break() {
        const STORAGE_SIZE: u32 = 16;
        // read raw = 3, write raw = 15
        // write_offset = 15 & 15 = 15 (distance to end is exactly 1 byte)
        // available data = 15 - 3 = 12 bytes
        // available space = 16 - 12 = 4 bytes
        let mut mock_storage = [0u8; STORAGE_SIZE as usize];

        // Safety: Pointers are valid.
        let mut spsc =
            unsafe { Buffer::from_raw_parts(mock_storage.as_mut_ptr(), mock_storage.len()) };
        spsc.combined_pointers.store((3u64 << 32) | 15u64, Ordering::Release);

        let mut reservation = spsc.reserve(4).unwrap();
        assert_eq!(reservation.region1.len(), 1);
        assert_eq!(reservation.region2.len(), 3);

        let data = [1, 2, 3, 4];
        reservation.write(&data).unwrap();
        reservation.commit().unwrap();

        assert_eq!(mock_storage[15], 1);
        assert_eq!(mock_storage[0], 2);
        assert_eq!(mock_storage[1], 3);
        assert_eq!(mock_storage[2], 4);
    }

    #[test]
    fn test_cpp_rust_integration() {
        #[link(name = "c++")]
        unsafe extern "C" {
            fn cpp_spsc_allocate(size: u32) -> *mut Buffer<NoOpAllocator>;
            fn cpp_spsc_free(spsc: *mut Buffer<NoOpAllocator>);
            fn cpp_spsc_write(spsc: *mut Buffer<NoOpAllocator>, data: *const u8, len: u32) -> i32;
            fn cpp_spsc_read(spsc: *mut Buffer<NoOpAllocator>, dst: *mut u8, len: u32) -> i32;
        }

        // 1. Allocate on the C++ side.
        let spsc_ptr = unsafe { cpp_spsc_allocate(256) };
        assert!(!spsc_ptr.is_null());

        // Convert the raw pointer to a Rust reference to interact with it in-place.
        let spsc = unsafe { &mut *spsc_ptr };

        // 2. C++ writes, Rust reads.
        let write_data = b"Hello from C++!";
        let cpp_write_status =
            unsafe { cpp_spsc_write(spsc_ptr, write_data.as_ptr(), write_data.len() as u32) };
        assert_eq!(cpp_write_status, 0); // ZX_OK

        // Rust reads and verifies.
        let mut read_buf = [0u8; 100];
        let bytes_read = spsc
            .read(
                |_, src| {
                    read_buf[..src.len()].copy_from_slice(src);
                    Ok(())
                },
                write_data.len() as u32,
            )
            .unwrap();
        assert_eq!(bytes_read, write_data.len() as u32);
        assert_eq!(&read_buf[..bytes_read as usize], write_data);

        // 3. Rust writes, C++ reads.
        let rust_write_data = b"Hello from Rust!";
        let mut reservation = spsc.reserve(rust_write_data.len() as u32).unwrap();
        reservation.write(rust_write_data).unwrap();
        reservation.commit().unwrap();

        // C++ reads and verifies.
        let mut cpp_read_buf = [0u8; 100];
        let cpp_read_bytes = unsafe {
            cpp_spsc_read(spsc_ptr, cpp_read_buf.as_mut_ptr(), rust_write_data.len() as u32)
        };
        assert_eq!(cpp_read_bytes, rust_write_data.len() as i32);
        assert_eq!(&cpp_read_buf[..cpp_read_bytes as usize], rust_write_data);

        // 4. Free on the C++ side.
        unsafe {
            cpp_spsc_free(spsc_ptr);
        }
    }
}
