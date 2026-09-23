// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::ops::{Deref, DerefMut};
use kalloc::{AllocError, Allocator, Box, DefaultAllocator};

/// Macro to construct a fallible `Vector`.
///
/// This macro is analogous to the standard `vec!` macro but returns an
/// `Option<Vector<T>>` to handle allocation failures gracefully.
///
/// # Returns
/// - `Some(Vector<T>)` on successful allocation.
/// - `None` if allocation fails.
///
/// # Examples
///
/// ```
/// use fbl::try_vec;
///
/// // Constructing a vector with a list of elements:
/// let v = try_vec![1, 2, 3].expect("Allocation failed");
/// assert_eq!(v.len(), 3);
///
/// // Constructing a vector with a repeated element:
/// let v2 = try_vec![0; 5].expect("Allocation failed");
/// assert_eq!(v2.len(), 5);
/// assert_eq!(v2[0], 0);
/// ```
#[macro_export]
macro_rules! try_vec {
    ($($x:expr),* $(,)?) => {
        {
            let mut v = $crate::Vector::new();
            let count = 0 $(+ { let _ = stringify!($x); 1 })*;
            let f = || {
                if count > 0 {
                    v.reserve(count)?;
                }
                $(
                    v.push_back($x)?;
                )*
                Ok(v)
            };
            f()
        }
    };

    ($elem:expr; $n:expr) => {
        {
            let mut v = $crate::Vector::new();
            let n = $n;
            let f = || {
                if n > 0 {
                    v.reserve(n)?;
                    for _ in 0..n {
                        v.push_back($elem.clone())?;
                    }
                }
                Ok(v)
            };
            f()
        }
    };
}

/// `Vector` is a heap-allocated dynamic array, providing a subset of the
/// functionality of `std::vec::Vec`.
///
/// Notably, `Vector` supports fallible allocation (methods return `Option`
/// on allocation failure) to handle out-of-memory conditions gracefully,
/// which is required for Zircon kernel code.
///
/// `Vector` does not implement `Clone` and cannot be copied.
pub struct Vector<T, A: Allocator = DefaultAllocator> {
    buf: Box<[core::mem::MaybeUninit<T>], A>,

    /// The number of entries in the vector.
    ///
    /// This struct maintains the invariant that the elements ..size of `buf`
    /// are initialized.
    size: usize,
}

const CAPACITY_MINIMUM: usize = 16;
const CAPACITY_GROWTH_FACTOR: usize = 2;
const CAPACITY_SHRINK_FACTOR: usize = 4;

// Size of Vector is now 24 bytes (Box (16) + size (8))
zr::static_assert!(core::mem::size_of::<Vector<u32>>() == 24);
zr::static_assert!(core::mem::align_of::<Vector<u32>>() == 8);

impl<T, A: Allocator> Vector<T, A> {
    /// Creates an empty vector with the given allocator.
    pub const fn new_in(allocator: A) -> Self {
        Vector { buf: Box::empty_slice_in(allocator), size: 0 }
    }

    /// Returns the number of elements in the vector.
    pub fn len(&self) -> usize {
        self.size
    }

    /// Returns the capacity of the vector.
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Returns true if the vector is empty.
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Reserve enough size to hold at least capacity elements.
    pub fn reserve(&mut self, new_capacity: usize) -> Result<(), AllocError> {
        if new_capacity <= self.buf.len() {
            return Ok(());
        }
        self.reallocate(new_capacity)
    }

    /// Clears the vector, dropping all elements.
    pub fn clear(&mut self) {
        self.truncate(0);
    }

    /// Swaps the contents of this vector with another.
    pub fn swap(&mut self, other: &mut Self) {
        core::mem::swap(self, other);
    }

    /// Appends an element to the back of the vector.
    pub fn push_back(&mut self, value: T) -> Result<(), AllocError> {
        self.grow_for_new_element()?;
        self.buf[self.size].write(value);
        self.size += 1;
        Ok(())
    }

    /// Removes the last element from the vector and returns it, or None if it is empty.
    pub fn pop_back(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }
        self.size -= 1;
        // SAFETY: We checked that the vector is not empty, and we decremented
        // size. So `self.size` is a valid index containing an initialized element.
        let val = unsafe { self.buf[self.size].assume_init_read() };
        self.consider_shrinking();
        Some(val)
    }

    /// Inserts an element at position index, shifting all elements after it to the right.
    pub fn insert(&mut self, index: usize, value: T) -> Result<(), AllocError> {
        assert!(index <= self.size);
        self.push_back(value)?;
        let size = self.size;
        self[index..size].rotate_right(1);
        Ok(())
    }

    /// Removes the element at position index, shifting all elements after it to the left.
    pub fn erase(&mut self, index: usize) -> T {
        assert!(index < self.size);
        let size = self.size;
        self[index..size].rotate_left(1);
        self.pop_back().unwrap()
    }

    /// Shortens the vector, keeping the first `new_len` elements and dropping the rest.
    /// If `new_len` is greater than or equal to the current size, this has no effect.
    pub fn truncate(&mut self, new_len: usize) {
        if new_len >= self.size {
            return;
        }
        let old_size = self.size;
        self.size = new_len;
        // SAFETY: Elements from new_len to old_size are initialized.
        unsafe {
            core::ptr::drop_in_place(self.buf[new_len..old_size].assume_init_mut());
        }
        self.consider_shrinking();
    }

    /// Resizes the vector to the specified size.
    /// If new_size is smaller, elements are truncated.
    /// If new_size is larger, new elements are initialized with `Default::default()`.
    /// Returns None if allocation fails.
    pub fn resize_with_default(&mut self, new_size: usize) -> Result<(), AllocError>
    where
        T: Default,
    {
        self.resize_with(new_size, T::default)
    }

    /// Resizes the vector to the specified size.
    /// If new_size is smaller, elements are truncated.
    /// If new_size is larger, new elements are cloned from `value`.
    /// Returns None if allocation fails.
    pub fn resize(&mut self, new_size: usize, value: T) -> Result<(), AllocError>
    where
        T: Clone,
    {
        self.resize_with(new_size, || value.clone())
    }

    /// Resizes the vector to the specified size.
    /// If new_size is smaller, elements are truncated.
    /// If new_size is larger, new elements are created by calling the closure.
    /// Returns None if allocation fails.
    pub fn resize_with<F>(&mut self, new_size: usize, mut f: F) -> Result<(), AllocError>
    where
        F: FnMut() -> T,
    {
        if new_size <= self.size {
            self.truncate(new_size);
        } else {
            self.reserve(new_size)?;
            while self.size < new_size {
                self.push_back(f())?;
            }
        }
        Ok(())
    }

    // Internal helper to reallocate storage.
    fn reallocate(&mut self, new_capacity: usize) -> Result<(), AllocError> {
        assert!(new_capacity > 0);
        assert!(new_capacity >= self.size);

        if new_capacity > self.buf.len() {
            Box::try_grow(&mut self.buf, new_capacity)?;
        } else if new_capacity < self.buf.len() {
            // SAFETY: We ensure in Vector that elements above `new_capacity`
            // are uninitialized or already dropped.
            unsafe {
                Box::try_shrink(&mut self.buf, new_capacity)?;
            }
        }
        Ok(())
    }

    // Internal helper to grow capacity if needed for a new element.

    fn grow_for_new_element(&mut self) -> Result<(), AllocError> {
        if self.size == self.buf.len() {
            let new_capacity = if self.buf.len() == 0 {
                CAPACITY_MINIMUM
            } else {
                self.buf.len() * CAPACITY_GROWTH_FACTOR
            };
            self.reallocate(new_capacity)?;
        }
        Ok(())
    }

    // Internal helper to shrink capacity if it's too large.
    fn consider_shrinking(&mut self) {
        if self.size * CAPACITY_SHRINK_FACTOR < self.buf.len() && self.buf.len() > CAPACITY_MINIMUM
        {
            let new_capacity = self.buf.len() / CAPACITY_SHRINK_FACTOR;
            // If reallocation fails, we just keep the old capacity.
            let _ = self.reallocate(new_capacity);
        }
    }

    /// Creates a vector from an iterator.
    /// Returns None if allocation fails.
    ///
    /// Creates a vector from an iterator with the given allocator.
    pub fn try_from_iter_in<I: IntoIterator<Item = T>>(
        iter: I,
        allocator: A,
    ) -> Result<Self, AllocError> {
        let mut v = Vector::new_in(allocator);
        let iter = iter.into_iter();

        let (lower, _) = iter.size_hint();
        if lower > 0 {
            v.reserve(lower)?;
        }

        for item in iter {
            v.push_back(item)?;
        }
        Ok(v)
    }
}

impl<T, A: Allocator> Deref for Vector<T, A> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        // SAFETY: Vector maintains the invariant that elements from 0 to self.size are initialized.
        unsafe { self.buf[0..self.size].assume_init_ref() }
    }
}

impl<T, A: Allocator> DerefMut for Vector<T, A> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Vector maintains the invariant that elements from 0 to self.size are initialized.
        unsafe { self.buf[0..self.size].assume_init_mut() }
    }
}

impl<T, A: Allocator> Drop for Vector<T, A> {
    fn drop(&mut self) {
        self.clear();
    }
}

impl<T> Vector<T, DefaultAllocator> {
    /// Creates an empty vector using the default allocator.
    pub const fn new() -> Self {
        Vector { buf: Box::empty_slice(), size: 0 }
    }

    /// Creates a vector from an iterator using the default allocator.
    pub fn try_from_iter<I: IntoIterator<Item = T>>(iter: I) -> Result<Self, AllocError> {
        let mut v = Vector::new();
        let iter = iter.into_iter();

        let (lower, _) = iter.size_hint();
        if lower > 0 {
            v.reserve(lower)?;
        }

        for item in iter {
            v.push_back(item)?;
        }
        Ok(v)
    }
}

impl<T> Default for Vector<T, DefaultAllocator> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    use core::cell::Cell;
    use core::ptr::NonNull;

    #[derive(Debug, PartialEq, Eq)]
    struct TestState {
        live_obj_count: Cell<usize>,
        ctor_count: Cell<usize>,
        dtor_count: Cell<usize>,
        alloc_count: Cell<usize>,
        fail_threshold: Cell<usize>,
    }

    impl Default for TestState {
        fn default() -> Self {
            Self {
                live_obj_count: Cell::new(0),
                ctor_count: Cell::new(0),
                dtor_count: Cell::new(0),
                alloc_count: Cell::new(0),
                fail_threshold: Cell::new(usize::MAX),
            }
        }
    }

    #[derive(Clone)]
    struct TestAllocator<'a> {
        state: &'a TestState,
    }

    impl<'a> Allocator for TestAllocator<'a> {
        fn allocate(&self, layout: core::alloc::Layout) -> Result<NonNull<[u8]>, AllocError> {
            let current = self.state.alloc_count.get();
            self.state.alloc_count.set(current + 1);
            if current >= self.state.fail_threshold.get() {
                return Err(AllocError);
            }
            DefaultAllocator::default().allocate(layout)
        }

        unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: core::alloc::Layout) {
            unsafe { DefaultAllocator::default().deallocate(ptr, layout) }
        }

        unsafe fn grow(
            &self,
            ptr: NonNull<u8>,
            old_layout: core::alloc::Layout,
            new_layout: core::alloc::Layout,
        ) -> Result<NonNull<[u8]>, AllocError> {
            let current = self.state.alloc_count.get();
            self.state.alloc_count.set(current + 1);
            if current >= self.state.fail_threshold.get() {
                return Err(AllocError);
            }
            unsafe { DefaultAllocator::default().grow(ptr, old_layout, new_layout) }
        }

        unsafe fn shrink(
            &self,
            ptr: NonNull<u8>,
            old_layout: core::alloc::Layout,
            new_layout: core::alloc::Layout,
        ) -> Result<NonNull<[u8]>, AllocError> {
            let current = self.state.alloc_count.get();
            self.state.alloc_count.set(current + 1);
            if current >= self.state.fail_threshold.get() {
                return Err(AllocError);
            }
            unsafe { DefaultAllocator::default().shrink(ptr, old_layout, new_layout) }
        }

        fn allocate_zeroed(
            &self,
            layout: core::alloc::Layout,
        ) -> Result<NonNull<[u8]>, AllocError> {
            let current = self.state.alloc_count.get();
            self.state.alloc_count.set(current + 1);
            if current >= self.state.fail_threshold.get() {
                return Err(AllocError);
            }
            DefaultAllocator::default().allocate_zeroed(layout)
        }
    }

    #[derive(Debug, Eq, PartialEq)]
    struct TestObject<'a> {
        val: usize,
        alive: bool,
        state: &'a TestState,
    }

    impl<'a> TestObject<'a> {
        fn new(val: usize, state: &'a TestState) -> Self {
            state.live_obj_count.set(state.live_obj_count.get() + 1);
            state.ctor_count.set(state.ctor_count.get() + 1);
            TestObject { val, alive: true, state }
        }
    }

    impl<'a> Drop for TestObject<'a> {
        fn drop(&mut self) {
            if self.alive {
                self.state.live_obj_count.set(self.state.live_obj_count.get() - 1);
                self.state.dtor_count.set(self.state.dtor_count.get() + 1);
            }
        }
    }

    #[test]
    fn test_empty() {
        let v: Vector<u32> = Vector::new();
        assert_eq!(v.len(), 0);
        assert_eq!(v.capacity(), 0);
        assert!(v.is_empty());
    }

    #[test]
    fn test_push_pop() {
        let mut v: Vector<u32> = Vector::new();
        v.push_back(1).unwrap();
        v.push_back(2).unwrap();
        v.push_back(3).unwrap();

        assert_eq!(v.len(), 3);
        assert_eq!(v[0], 1);
        assert_eq!(v[1], 2);
        assert_eq!(v[2], 3);

        assert_eq!(v.pop_back(), Some(3));
        assert_eq!(v.pop_back(), Some(2));
        assert_eq!(v.pop_back(), Some(1));
        assert_eq!(v.pop_back(), None);
    }

    #[test]
    fn test_insert_erase() {
        let mut v: Vector<u32> = Vector::new();
        v.push_back(1).unwrap();
        v.push_back(3).unwrap();

        v.insert(1, 2).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], 1);
        assert_eq!(v[1], 2);
        assert_eq!(v[2], 3);

        assert_eq!(v.erase(1), 2);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], 1);
        assert_eq!(v[1], 3);
    }

    #[test]
    fn test_resize() {
        let mut v: Vector<u32> = Vector::new();
        v.resize_with_default(5).unwrap();
        assert_eq!(v.len(), 5);
        for i in 0..5 {
            assert_eq!(v[i], 0);
        }

        v.resize_with_default(2).unwrap();
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn test_drop_behavior() {
        let state = TestState::default();
        {
            let mut v: Vector<TestObject<'_>, TestAllocator<'_>> =
                Vector::new_in(TestAllocator { state: &state });
            v.push_back(TestObject::new(1, &state)).unwrap();
            v.push_back(TestObject::new(2, &state)).unwrap();
            assert_eq!(state.live_obj_count.get(), 2);
        }
        assert_eq!(state.live_obj_count.get(), 0);
        assert_eq!(state.ctor_count.get(), 2);
        assert_eq!(state.dtor_count.get(), 2);
    }

    #[test]
    fn test_counting_allocator() {
        let state = TestState::default();
        {
            let mut v: Vector<u32, TestAllocator<'_>> =
                Vector::new_in(TestAllocator { state: &state });
            v.push_back(1).unwrap(); // Causes allocation
            assert_eq!(state.alloc_count.get(), 1);
        }
    }

    #[test]
    fn test_failing_allocator() {
        let state = TestState::default();
        state.fail_threshold.set(1); // Fail after 1st allocation

        let mut v: Vector<u32, TestAllocator<'_>> = Vector::new_in(TestAllocator { state: &state });
        v.push_back(1).unwrap(); // 1st alloc succeeds

        // Fill up to capacity to trigger grow
        for i in 2..=16 {
            v.push_back(i).unwrap();
        }
        assert_eq!(v.len(), 16);
        assert_eq!(v.capacity(), 16);

        // Next push_back will try to grow and call alloc.
        // Since threshold is 1, and we already did 1 alloc (at first push),
        // next alloc will fail!
        assert_eq!(v.push_back(17), Err(AllocError));
        assert_eq!(v.len(), 16); // Size unchanged

        // Verify elements are still valid
        for i in 0..16 {
            assert_eq!(v[i], (i + 1) as u32);
        }
    }

    #[test]
    fn test_truncate() {
        let mut v: Vector<u32> = Vector::new();
        v.push_back(1).unwrap();
        v.push_back(2).unwrap();
        v.push_back(3).unwrap();

        v.truncate(2);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], 1);
        assert_eq!(v[1], 2);

        v.truncate(5); // No effect
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn test_truncate_drops_elements() {
        let state = TestState::default();
        {
            let mut v: Vector<TestObject<'_>, TestAllocator<'_>> =
                Vector::new_in(TestAllocator { state: &state });
            v.push_back(TestObject::new(1, &state)).unwrap();
            v.push_back(TestObject::new(2, &state)).unwrap();
            v.push_back(TestObject::new(3, &state)).unwrap();

            assert_eq!(state.live_obj_count.get(), 3);

            v.truncate(1);
            assert_eq!(v.len(), 1);
            assert_eq!(state.live_obj_count.get(), 1);
            assert_eq!(state.dtor_count.get(), 2);
        }
        assert_eq!(state.live_obj_count.get(), 0);
    }

    #[test]
    fn test_iterator() {
        let mut v: Vector<u32> = Vector::new();
        v.push_back(1).unwrap();
        v.push_back(2).unwrap();

        let mut it = v.iter();
        assert_eq!(it.next(), Some(&1));
        assert_eq!(it.next(), Some(&2));
        assert_eq!(it.next(), None);

        for x in v.iter_mut() {
            *x += 10;
        }

        assert_eq!(v[0], 11);
        assert_eq!(v[1], 12);
    }

    #[test]
    fn test_box() {
        let state = TestState::default();
        {
            let mut v: Vector<Box<TestObject<'_>, TestAllocator<'_>>, TestAllocator<'_>> =
                Vector::new_in(TestAllocator { state: &state });
            v.push_back(
                Box::try_new_in(TestObject::new(1, &state), TestAllocator { state: &state })
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(v.len(), 1);
            assert_eq!(v[0].val, 1);
        }
    }

    #[test]
    fn test_try_from_iter() {
        let items = [1, 2, 3, 4, 5];
        let v: Vector<u32> = Vector::try_from_iter(items.iter().copied()).unwrap();
        assert_eq!(v.len(), 5);
        assert_eq!(v[0], 1);
        assert_eq!(v[4], 5);
    }

    #[test]
    fn test_try_from_iter_failing() {
        let state = TestState::default();
        state.fail_threshold.set(0); // Fail immediately

        let items = [1, 2, 3, 4, 5];
        let v: Result<Vector<u32, TestAllocator<'_>>, AllocError> =
            Vector::try_from_iter_in(items.iter().copied(), TestAllocator { state: &state });
        assert!(v.is_err());
    }

    #[test]
    fn test_try_vec_macro() {
        let v: Result<Vector<u32>, AllocError> = try_vec![1, 2, 3];
        let v = v.unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], 1);
        assert_eq!(v[2], 3);

        let v2: Result<Vector<u32>, AllocError> = try_vec![0; 5];
        let v2 = v2.unwrap();
        assert_eq!(v2.len(), 5);
        for i in 0..5 {
            assert_eq!(v2[i], 0);
        }
    }

    #[test]
    fn test_try_vec_macro_failing() {
        let state = TestState::default();
        state.fail_threshold.set(0); // Fail immediately

        let mut v: Vector<u32, TestAllocator<'_>> = Vector::new_in(TestAllocator { state: &state });
        assert!(v.push_back(1).is_err());
    }

    #[test]
    fn test_swap() {
        let v1: Result<Vector<u32>, AllocError> = try_vec![1, 2, 3];
        let mut v1 = v1.unwrap();
        let v2: Result<Vector<u32>, AllocError> = try_vec![4, 5];
        let mut v2 = v2.unwrap();

        v1.swap(&mut v2);

        assert_eq!(v1.len(), 2);
        assert_eq!(v1[0], 4);
        assert_eq!(v1[1], 5);

        assert_eq!(v2.len(), 3);
        assert_eq!(v2[0], 1);
        assert_eq!(v2[1], 2);
        assert_eq!(v2[2], 3);
    }

    #[test]
    fn test_resize_with_value() {
        let mut v: Vector<u32> = Vector::new();
        v.resize(3, 42).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], 42);
        assert_eq!(v[1], 42);
        assert_eq!(v[2], 42);

        v.resize(1, 10).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0], 42); // Original element preserved
    }

    #[test]
    fn test_resize_with() {
        let mut v: Vector<u32> = Vector::new();
        let mut c = 0;
        v.resize_with(3, || {
            c += 1;
            c
        })
        .unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], 1);
        assert_eq!(v[1], 2);
        assert_eq!(v[2], 3);
    }
}
