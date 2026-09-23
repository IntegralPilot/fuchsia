// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::mem::MaybeUninit;

/// `RingBuffer` is a statically-allocated, typed ring buffer container.
/// This container is not thread safe.
#[repr(C)]
pub struct RingBuffer<T, const N: usize> {
    data: [MaybeUninit<T>; N],

    /// The index offset to the oldest element in the buffer.
    head: u32,

    /// The index offset to the empty slot where the next element should be inserted.
    tail: u32,

    /// The number of elements currently stored in the buffer.
    size: u32,
}

// Compile-time tests to ensure layout compatibility for a specific instance
zr::static_assert!(core::mem::size_of::<RingBuffer<u8, 10>>() == 24);
zr::static_assert!(core::mem::align_of::<RingBuffer<u8, 10>>() == 4);

impl<T, const N: usize> RingBuffer<T, N> {
    const ASSERT_N_POSITIVE: () = assert!(N > 0);
    const ASSERT_N_FITS_U32: () = assert!(N <= u32::MAX as usize);

    /// Create a new, empty RingBuffer.
    pub const fn new() -> Self {
        let _ = Self::ASSERT_N_POSITIVE;
        let _ = Self::ASSERT_N_FITS_U32;
        RingBuffer { data: [const { MaybeUninit::uninit() }; N], head: 0, tail: 0, size: 0 }
    }

    /// Returns the number of elements in the buffer.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Returns the capacity of the buffer.
    pub const fn capacity() -> usize {
        N
    }

    /// Returns true if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Returns true if the buffer is full.
    pub fn is_full(&self) -> bool {
        self.size == N as u32
    }

    /// Returns a reference to the oldest element in the buffer.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is empty.
    pub fn front(&self) -> &T {
        assert!(!self.is_empty(), "Calling front on an empty RingBuffer");
        // SAFETY: The buffer is not empty, so the element at `head` is initialized.
        unsafe { self.data[self.head as usize].assume_init_ref() }
    }

    /// Returns a mutable reference to the oldest element in the buffer.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is empty.
    pub fn front_mut(&mut self) -> &mut T {
        assert!(!self.is_empty(), "Calling front_mut on an empty RingBuffer");
        // SAFETY: The buffer is not empty, so the element at `head` is initialized.
        unsafe { self.data[self.head as usize].assume_init_mut() }
    }

    /// Returns a reference to the newest element in the buffer.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is empty.
    pub fn back(&self) -> &T {
        assert!(!self.is_empty(), "Calling back on an empty RingBuffer");
        let index = self.previous(self.tail);
        // SAFETY: The buffer is not empty, so the element at `previous(tail)` is initialized.
        unsafe { self.data[index as usize].assume_init_ref() }
    }

    /// Returns a mutable reference to the newest element in the buffer.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is empty.
    pub fn back_mut(&mut self) -> &mut T {
        assert!(!self.is_empty(), "Calling back_mut on an empty RingBuffer");
        let index = self.previous(self.tail);
        // SAFETY: The buffer is not empty, so the element at `previous(tail)` is initialized.
        unsafe { self.data[index as usize].assume_init_mut() }
    }

    /// Removes the oldest element from the buffer.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is empty.
    pub fn pop(&mut self) {
        assert!(!self.is_empty(), "Calling pop on an empty RingBuffer");
        // SAFETY: The buffer is not empty, so the element at `head` is initialized.
        unsafe { self.data[self.head as usize].assume_init_drop() };
        self.head = self.next(self.head);
        self.size -= 1;
    }

    /// Pushes a new element into the buffer.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is full.
    pub fn push(&mut self, obj: T) {
        assert!(!self.is_full(), "Calling push on a full RingBuffer");
        self.data[self.tail as usize].write(obj);
        self.tail = self.next(self.tail);
        self.size += 1;
    }

    /// Removes all elements from the buffer.
    pub fn clear(&mut self) {
        while !self.is_empty() {
            self.pop();
        }
        self.head = 0;
        self.tail = 0;
        self.size = 0;
    }

    fn next(&self, index: u32) -> u32 {
        if index == (N as u32 - 1) { 0 } else { index + 1 }
    }

    fn previous(&self, index: u32) -> u32 {
        if index == 0 { N as u32 - 1 } else { index - 1 }
    }
}

impl<T, const N: usize> Drop for RingBuffer<T, N> {
    fn drop(&mut self) {
        self.clear();
    }
}

impl<T, const N: usize> Default for RingBuffer<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn pod_push() {
        const BUFF_SIZE: usize = 10;
        let mut buffer = RingBuffer::<u8, BUFF_SIZE>::new();
        assert_eq!(buffer.size(), 0);
        assert!(buffer.is_empty());

        // Fill the buffer to capacity.
        for i in 0..BUFF_SIZE {
            buffer.push(i as u8);
            assert_eq!(*buffer.front(), 0);
            assert_eq!(*buffer.back(), i as u8);
        }

        assert!(buffer.is_full());
        assert_eq!(*buffer.front(), 0);

        for i in 0..BUFF_SIZE {
            assert_eq!(*buffer.front(), i as u8);
            assert_eq!(*buffer.back(), (BUFF_SIZE - 1) as u8);
            buffer.pop();
        }

        assert!(buffer.is_empty());

        // Wrap around test.
        buffer.push(11);
        assert_eq!(*buffer.front(), 11);
    }

    #[test]
    fn default_trait() {
        let buffer = RingBuffer::<u8, 10>::default();
        assert_eq!(buffer.size(), 0);
        assert!(buffer.is_empty());
    }

    #[test]
    #[should_panic(expected = "Calling pop on an empty RingBuffer")]
    fn empty_pop_assert() {
        let mut buffer = RingBuffer::<u8, 10>::new();
        buffer.pop();
    }

    #[test]
    #[should_panic(expected = "Calling front on an empty RingBuffer")]
    fn empty_front_assert() {
        let buffer = RingBuffer::<u8, 10>::new();
        let _ = buffer.front();
    }

    #[test]
    #[should_panic(expected = "Calling back on an empty RingBuffer")]
    fn empty_back_assert() {
        let buffer = RingBuffer::<u8, 10>::new();
        let _ = buffer.back();
    }

    #[test]
    #[should_panic(expected = "Calling push on a full RingBuffer")]
    fn full_push_assert() {
        let mut buffer = RingBuffer::<u8, 2>::new();
        buffer.push(1);
        buffer.push(2);
        buffer.push(3);
    }

    #[test]
    fn construct_destruct_match() {
        static CONSTRUCT_COUNT: AtomicU32 = AtomicU32::new(0);
        static DESTRUCT_COUNT: AtomicU32 = AtomicU32::new(0);

        // Reset counts for the test
        CONSTRUCT_COUNT.store(0, Ordering::Relaxed);
        DESTRUCT_COUNT.store(0, Ordering::Relaxed);

        struct TestObj;

        impl TestObj {
            fn new() -> Self {
                CONSTRUCT_COUNT.fetch_add(1, Ordering::Relaxed);
                TestObj
            }
        }

        impl Drop for TestObj {
            fn drop(&mut self) {
                DESTRUCT_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }

        {
            let mut buffer = RingBuffer::<TestObj, 10>::new();

            buffer.push(TestObj::new());
            assert_eq!(CONSTRUCT_COUNT.load(Ordering::Relaxed), 1);
            assert_eq!(DESTRUCT_COUNT.load(Ordering::Relaxed), 0);

            buffer.pop();
            assert_eq!(CONSTRUCT_COUNT.load(Ordering::Relaxed), 1);
            assert_eq!(DESTRUCT_COUNT.load(Ordering::Relaxed), 1);

            buffer.push(TestObj::new());
            buffer.push(TestObj::new());
            assert_eq!(CONSTRUCT_COUNT.load(Ordering::Relaxed), 3);
            assert_eq!(DESTRUCT_COUNT.load(Ordering::Relaxed), 1);

            buffer.clear();
            assert_eq!(CONSTRUCT_COUNT.load(Ordering::Relaxed), 3);
            assert_eq!(DESTRUCT_COUNT.load(Ordering::Relaxed), 3);

            buffer.push(TestObj::new());
            buffer.push(TestObj::new());
            assert_eq!(CONSTRUCT_COUNT.load(Ordering::Relaxed), 5);
            assert_eq!(DESTRUCT_COUNT.load(Ordering::Relaxed), 3);
        }

        // Out of scope.
        assert_eq!(CONSTRUCT_COUNT.load(Ordering::Relaxed), 5);
        assert_eq!(DESTRUCT_COUNT.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn test_ring_buffer_capacity() {
        assert_eq!(RingBuffer::<u8, 5>::capacity(), 5);
    }

    #[test]
    fn test_ring_buffer_mut_accessors() {
        let mut buffer = RingBuffer::<u8, 5>::new();
        buffer.push(10);
        buffer.push(20);
        *buffer.front_mut() = 15;
        *buffer.back_mut() = 25;
        assert_eq!(*buffer.front(), 15);
        assert_eq!(*buffer.back(), 25);
    }

    #[test]
    #[should_panic(expected = "Calling front_mut on an empty RingBuffer")]
    fn empty_front_mut_assert() {
        let mut buffer = RingBuffer::<u8, 10>::new();
        let _ = buffer.front_mut();
    }

    #[test]
    #[should_panic(expected = "Calling back_mut on an empty RingBuffer")]
    fn empty_back_mut_assert() {
        let mut buffer = RingBuffer::<u8, 10>::new();
        let _ = buffer.back_mut();
    }
}
