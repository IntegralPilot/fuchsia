// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ops::{Deref, DerefMut};

pub mod raw_vec;

use crate::storage::StorageFamily;
use crate::vec::raw_vec::RawVec;

/// A contiguous, growable vector collection backed by a `RawVecLike` container.
///
/// # Examples
///
/// ```
/// use sapphire_collections::vec::StackVec;
///
/// let mut vec = StackVec::<i32, 4>::new();
/// vec.try_push(10).unwrap();
/// vec.try_push(20).unwrap();
/// assert_eq!(vec.len(), 2);
/// assert_eq!(vec[0], 10);
/// assert_eq!(vec.pop(), Some(20));
/// ```
pub struct Vec<T, A: StorageFamily> {
    inner: RawVec<T, A>,
    len: usize,
}

impl<T, A: StorageFamily> Default for Vec<T, A>
where
    RawVec<T, A>: Default,
{
    fn default() -> Self {
        Self { inner: Default::default(), len: 0 }
    }
}

impl<T, A: StorageFamily> Vec<T, A> {
    /// Creates a new, empty vector.
    pub fn new() -> Self
    where
        Self: Default,
    {
        Self::default()
    }

    /// Creates a new, empty vector with the given allocator.
    pub fn new_in(allocator: A::Storage<T>) -> Self {
        Self { inner: RawVec::new_in(allocator), len: 0 }
    }

    /// Returns the total capacity of the underlying buffer.
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Reserves capacity for at least `additional` more elements to be inserted in the given `Vec`.
    ///
    /// # Errors
    /// Returns `Err(AllocError)` if the buffer is full and cannot be grown.
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), crate::AllocError> {
        let needed_capacity = self.len.checked_add(additional).ok_or(crate::AllocError)?;
        self.inner.grow_at_least(needed_capacity)
    }

    /// Reserves the minimum capacity for exactly `additional` more elements to be inserted in the given `Vec`.
    ///
    /// # Errors
    /// Returns `Err(AllocError)` if the buffer is full and cannot be grown.
    pub fn try_reserve_exact(&mut self, additional: usize) -> Result<(), crate::AllocError> {
        let needed_capacity = self.len.checked_add(additional).ok_or(crate::AllocError)?;
        self.inner.grow_exactly(needed_capacity)
    }

    /// Attempts to push a value to the back of the vector.
    ///
    /// Returns `Err(value)` if the buffer is full and cannot be grown.
    pub fn try_push(&mut self, value: T) -> Result<(), T> {
        if self.len() == self.capacity() {
            if self.inner.grow().is_err() {
                return Err(value);
            }
        }
        debug_assert!(self.capacity() > self.len());
        // SAFETY: `len < capacity()`, so the index `len` is within bounds.
        unsafe {
            self.inner.buffer_mut().get_unchecked_mut(self.len).write(value);
        }
        self.len += 1;
        Ok(())
    }

    /// Removes and returns the last element of the vector, if any.
    pub fn pop(&mut self) -> Option<T> {
        if self.len() == 0 {
            None
        } else {
            self.len -= 1;
            // SAFETY: `len` was previously initialized before incrementing, so it is valid to read.
            let inner =
                unsafe { self.inner.buffer_mut().get_unchecked_mut(self.len).assume_init_read() };
            Some(inner)
        }
    }

    /// Removes and returns the element at position `index` within the vector,
    /// shifting all elements after it to the left.
    ///
    /// # Panics
    /// Panics if `index` is out of bounds.
    pub fn remove(&mut self, index: usize) -> T {
        let len = self.len();
        assert!(index < len, "Index out of bounds");
        // SAFETY: We asserted that `index < len`. Because `inner` is initialized up to `len`,
        // the index `index` represents a valid initialized element. Shifting elements to the left
        // with `ptr::copy` is safe as long as the segments are within [0..len], which they are.
        unsafe {
            let ptr = self.inner.buffer_mut().as_mut_ptr().add(index);
            let ret = ptr.read().assume_init();
            // Shift elements after index to the left
            core::ptr::copy(ptr.add(1), ptr, len - index - 1);
            self.len -= 1;
            ret
        }
    }

    /// Returns the number of elements currently in the vector.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Shortens the vector, keeping the first `len` elements and dropping the rest.
    ///
    /// If `len` is greater than or equal to the vector's current length, this has no effect.
    ///
    /// # Time Complexity
    /// - `O(1)` if `T` does not require dropping (e.g., `u8`).
    /// - `O(N)` if `T` requires dropping (where `N` is the number of elements removed).
    pub fn truncate(&mut self, new_len: usize) {
        if new_len < self.len {
            let old_len = self.len;
            self.len = new_len;
            if core::mem::needs_drop::<T>() {
                // SAFETY: The elements in `new_len..old_len` are initialized.
                // We update `self.len` to `new_len` before dropping to ensure that if a destructor
                // panics, the vector is already in a consistent state (preventing double-drops).
                unsafe {
                    self.inner.buffer_mut()[new_len..old_len].assume_init_drop();
                }
            }
        }
    }

    /// Clears the vector, removing all values.
    pub fn clear(&mut self) {
        self.truncate(0);
    }

    /// Clones and appends all elements in a slice to the vector.
    ///
    /// If the vector does not have enough capacity to hold all elements, it will attempt
    /// to grow the underlying storage.
    ///
    /// # Errors
    /// Returns `Err(AllocError)` if the buffer is full and cannot be grown.
    pub fn try_extend(&mut self, other: &[T]) -> Result<(), crate::AllocError>
    where
        T: Clone,
    {
        let needed_capacity = self.len() + other.len();
        self.inner.grow_at_least(needed_capacity).map_err(|_| crate::AllocError)?;
        for item in other {
            // SAFETY: We ensured that the capacity is at least needed_capacity, so the index
            // `self.len` is within bounds.
            // Panic Safety: If cloning panics, it occurs before writing the value and
            // incrementing `self.len`, keeping `self.len` consistent.
            unsafe {
                self.inner.buffer_mut().get_unchecked_mut(self.len).write(item.clone());
            }
            self.len += 1;
        }
        Ok(())
    }

    /// Resizes the `Vec` in-place so that `len` is equal to `new_len`.
    ///
    /// If `new_len` is greater than `len`, the `Vec` is extended by the difference, with each
    /// additional slot filled with a clone of `value`. If `new_len` is less than `len`, the `Vec`
    /// is truncated.
    ///
    /// # Errors
    /// Returns `Err(AllocError)` if the new length exceeds capacity and the buffer cannot be grown.
    pub fn try_resize(&mut self, new_len: usize, value: T) -> Result<(), crate::AllocError>
    where
        T: Clone,
    {
        if new_len > self.len() {
            let padding_len = new_len - self.len();
            self.inner.grow_at_least(new_len).map_err(|_| crate::AllocError)?;
            for _ in 0..padding_len {
                // SAFETY: We ensured that the capacity is at least new_len, so the index
                // `self.len` is within bounds.
                // Panic Safety: If cloning panics, it occurs before writing the value and
                // incrementing `self.len`, keeping `self.len` consistent.
                unsafe {
                    self.inner.buffer_mut().get_unchecked_mut(self.len).write(value.clone());
                }
                self.len += 1;
            }
        } else {
            self.truncate(new_len);
        }
        Ok(())
    }

    /// Returns a raw pointer to the vector's buffer, or a dangling raw pointer
    /// valid for zero sized reads if the vector didn't allocate.
    ///
    /// The caller must ensure that the vector outlives the pointer this
    /// function returns, or else it will end up dangling.
    /// Modifying the vector may cause its buffer to be reallocated,
    /// which would also make any pointers to it invalid.
    ///
    /// The caller must also ensure that the memory the pointer (non-transitively) points to
    /// is never written to (except inside an `UnsafeCell`) using this pointer or any pointer
    /// derived from it. If you need to mutate the contents of the slice, use [`as_mut_ptr`].
    ///
    /// [`as_mut_ptr`]: Vec::as_mut_ptr
    pub fn as_ptr(&self) -> *const T {
        self.inner.buffer().as_ptr().cast()
    }

    /// Returns a raw mutable pointer to the vector's buffer, or a dangling
    /// raw pointer valid for zero sized reads if the vector didn't allocate.
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.inner.buffer_mut().as_mut_ptr().cast()
    }

    /// Forces the length of the vector to `new_len`.
    ///
    /// This is a low-level operation that maintains none of the normal
    /// invariants of the type. Normally changing the length of a vector
    /// is done using one of the safe operations instead, such as
    /// [`truncate`], [`resize`], [`extend`], or [`clear`].
    ///
    /// [`truncate`]: Vec::truncate
    /// [`resize`]: Vec::resize
    /// [`extend`]: Extend::extend
    /// [`clear`]: Vec::clear
    ///
    /// # Safety
    ///
    /// - `new_len` must be less than or equal to [`capacity()`].
    /// - The elements at `old_len..new_len` must be initialized.
    ///
    /// [`capacity()`]: Vec::capacity
    pub unsafe fn set_len(&mut self, new_len: usize) {
        debug_assert!(new_len <= self.capacity());
        self.len = new_len;
    }
}

impl<T, A: StorageFamily> Deref for Vec<T, A> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        // SAFETY: All elements up to `len` are guaranteed initialized by `try_push`.
        unsafe { self.inner.buffer()[..self.len()].assume_init_ref() }
    }
}

impl<T, A: StorageFamily> DerefMut for Vec<T, A> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let len = self.len();
        // SAFETY: All elements up to `len` are guaranteed initialized by `try_push`.
        unsafe { self.inner.buffer_mut()[..len].assume_init_mut() }
    }
}

impl<T, A: StorageFamily> Drop for Vec<T, A> {
    fn drop(&mut self) {
        for element in self.iter_mut() {
            // SAFETY: All of these elements are initialized, and won't be accessed after dropping
            // them
            unsafe {
                core::ptr::drop_in_place(element);
            }
        }
    }
}

use crate::storage::ArrayStorage;

/// A vector collection backed by a stack-allocated fixed-size raw array.
pub type StackVec<T, const SIZE: usize> = Vec<T, ArrayStorage<SIZE>>;

/// A vector collection backed by a standard growable heap-allocated raw buffer.
#[cfg(feature = "std")]
pub type StdVec<T> = Vec<T, crate::storage::Global>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn test_vec_basic() {
        let mut vec = StackVec::<i32, 4>::new();
        assert_eq!(vec.len(), 0);
        assert_eq!(vec.capacity(), 0);

        vec.try_push(10).unwrap();
        assert_eq!(vec.capacity(), 4);
        vec.try_push(20).unwrap();
        assert_eq!(vec.len(), 2);
        assert_eq!(vec[0], 10);
        assert_eq!(vec[1], 20);

        assert_eq!(vec.pop(), Some(20));
        assert_eq!(vec.len(), 1);
        assert_eq!(vec.pop(), Some(10));
        assert_eq!(vec.len(), 0);
        assert_eq!(vec.pop(), None);
    }

    #[test]
    fn test_vec_remove() {
        let mut vec = StackVec::<i32, 4>::new();
        vec.try_push(10).unwrap();
        vec.try_push(20).unwrap();
        vec.try_push(30).unwrap();

        // Remove middle
        assert_eq!(vec.remove(1), 20);
        assert_eq!(vec.len(), 2);
        assert_eq!(vec[0], 10);
        assert_eq!(vec[1], 30);

        // Remove front
        assert_eq!(vec.remove(0), 10);
        assert_eq!(vec.len(), 1);
        assert_eq!(vec[0], 30);

        // Remove back
        assert_eq!(vec.remove(0), 30);
        assert_eq!(vec.len(), 0);
    }

    #[test]
    #[should_panic(expected = "Index out of bounds")]
    fn test_vec_remove_out_of_bounds() {
        let mut vec = StackVec::<i32, 4>::new();
        vec.try_push(10).unwrap();
        vec.remove(1);
    }

    #[test]
    fn test_vec_drop() {
        let counter = Rc::new(Cell::new(0));
        #[derive(Debug)]
        struct DropItem(Rc<Cell<i32>>);
        impl Drop for DropItem {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        {
            let mut vec = StackVec::<DropItem, 4>::new();
            vec.try_push(DropItem(counter.clone())).unwrap();
            vec.try_push(DropItem(counter.clone())).unwrap();
            // pop one
            vec.pop();
            assert_eq!(counter.get(), 1); // popped one should be dropped
        }
        assert_eq!(counter.get(), 2); // remaining one should be dropped when vec goes out of scope
    }

    #[test]
    fn test_vec_truncate_and_clear() {
        let counter = Rc::new(Cell::new(0));
        #[derive(Debug)]
        struct DropItem(Rc<Cell<i32>>);
        impl Drop for DropItem {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let mut vec = StackVec::<DropItem, 4>::new();
        vec.try_push(DropItem(counter.clone())).unwrap();
        vec.try_push(DropItem(counter.clone())).unwrap();
        vec.try_push(DropItem(counter.clone())).unwrap();

        assert_eq!(vec.len(), 3);
        assert_eq!(counter.get(), 0);

        // Truncate keeping first 2
        vec.truncate(2);
        assert_eq!(vec.len(), 2);
        assert_eq!(counter.get(), 1); // 1 item dropped

        // Clear the remaining
        vec.clear();
        assert_eq!(vec.len(), 0);
        assert_eq!(counter.get(), 3); // remaining 2 dropped
    }

    #[test]
    fn test_vec_extend_and_resize() {
        let mut vec = StackVec::<i32, 6>::new();
        vec.try_extend(&[1, 2, 3]).unwrap();
        assert_eq!(&vec[..], &[1, 2, 3]);

        // Fail to extend past capacity (capacity is 6, len is 3, trying to add 4 elements)
        assert!(vec.try_extend(&[4, 5, 6, 7]).is_err());
        assert_eq!(&vec[..], &[1, 2, 3]); // Stays unmodified on failure

        // Success extend up to capacity
        vec.try_extend(&[4, 5, 6]).unwrap();
        assert_eq!(&vec[..], &[1, 2, 3, 4, 5, 6]);

        // Resize down (truncation)
        vec.try_resize(3, 0).unwrap();
        assert_eq!(&vec[..], &[1, 2, 3]);

        // Resize up with padding
        vec.try_resize(5, 9).unwrap();
        assert_eq!(&vec[..], &[1, 2, 3, 9, 9]);

        // Fail to resize past capacity
        assert!(vec.try_resize(7, 0).is_err());
        assert_eq!(&vec[..], &[1, 2, 3, 9, 9]);
    }

    #[test]
    fn test_vec_resize_non_power_of_two() {
        let mut vec = StackVec::<i32, 3>::new();
        assert_eq!(vec.capacity(), 0);

        // Resizing directly to the maximum non-power-of-two capacity (3) triggers growth
        // where the next power-of-two (4) exceeds storage capacity, falling back to exact capacity.
        vec.try_resize(3, 42).unwrap();
        assert_eq!(vec.len(), 3);
        assert_eq!(vec.capacity(), 3);
        assert_eq!(&vec[..], &[42, 42, 42]);
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        #[derive(Debug, Clone)]
        enum VecOp<T> {
            Push(T),
            Pop,
            Remove(usize),
        }

        proptest! {
            #[test]
            fn test_vec_differential(ops in prop::collection::vec(
                prop_oneof![
                    any::<i32>().prop_map(VecOp::<i32>::Push),
                    Just(VecOp::Pop),
                    any::<usize>().prop_map(VecOp::<i32>::Remove),
                ],
                0..100
            )) {
                let mut custom_vec = StdVec::<i32>::new();
                let mut std_vec = std::vec::Vec::<i32>::new();

                for op in ops {
                    match op {
                        VecOp::Push(val) => {
                            custom_vec.try_push(val).unwrap();
                            std_vec.push(val);
                        }
                        VecOp::Pop => {
                            assert_eq!(custom_vec.pop(), std_vec.pop());
                        }
                        VecOp::Remove(idx) => {
                            let len = std_vec.len();
                            if len > 0 {
                                let target_idx = idx % len;
                                assert_eq!(custom_vec.remove(target_idx), std_vec.remove(target_idx));
                            }
                        }
                    }
                    assert_eq!(custom_vec.len(), std_vec.len());
                    assert_eq!(&custom_vec[..], &std_vec[..]);
                }
            }
        }
        proptest! {
            #[test]
            fn test_vec_zst(ops in prop::collection::vec(
                prop_oneof![
                    Just(VecOp::<()>::Push(())),
                    Just(VecOp::<()>::Pop),
                    any::<usize>().prop_map(VecOp::<()>::Remove),
                ],
                0..100
            )) {
                let mut custom_vec = StdVec::<()>::new();
                let mut std_vec = std::vec::Vec::<()>::new();

                for op in ops {
                    match op {
                        VecOp::Push(()) => {
                            custom_vec.try_push(()).unwrap();
                            std_vec.push(());
                        }
                        VecOp::Pop => {
                            custom_vec.pop();
                            std_vec.pop();
                        }
                        VecOp::Remove(idx) => {
                            let len = std_vec.len();
                            if len > 0 {
                                let target_idx = idx % len;
                                custom_vec.remove(target_idx);
                                std_vec.remove(target_idx);
                            }
                        }
                    }
                    assert_eq!(custom_vec.len(), std_vec.len());
                    assert_eq!(&custom_vec[..], &std_vec[..]);
                }
            }
        }
    }
}
