// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr::{self, NonNull};

use crate::AllocError;
use crate::storage::StorageFamily;
use crate::vec::raw_vec::RawVec;

/// A ring-buffer queue (`Deque`) backed by a contiguous raw vector container.
///
/// Provides bounded FIFO and double-ended operations without allocating at runtime.
///
/// # Examples
///
/// ```
/// use sapphire_collections::deque::StackDeque;
///
/// let mut deque = StackDeque::<i32, 4>::new();
/// deque.push_back(10).unwrap();
/// deque.push_front(20).unwrap();
/// assert_eq!(deque.len(), 2);
/// assert_eq!(deque.pop_front(), Some(20));
/// assert_eq!(deque.pop_back(), Some(10));
/// ```
pub struct Deque<T, A: StorageFamily> {
    inner: RawVec<T, A>,
    head: usize,
    len: usize,
}

impl<T, A: StorageFamily> Default for Deque<T, A>
where
    RawVec<T, A>: Default,
{
    fn default() -> Self {
        Self { inner: Default::default(), head: Default::default(), len: Default::default() }
    }
}

impl<T, A: StorageFamily> Deque<T, A> {
    /// Creates a new, empty `Deque` using the default buffer initialization.
    pub fn new() -> Self
    where
        Self: Default,
    {
        Self::default()
    }

    /// Creates a new, empty `Deque` backed by the provided buffer container.
    pub fn new_in(allocator: A::Storage<T>) -> Self {
        Self { inner: RawVec::new_in(allocator), head: 0, len: 0 }
    }

    /// Returns the number of elements currently stored in the queue.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns the number of elements currently stored in the queue.
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Returns `true` if the queue contains no elements.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Reserves capacity for at least `additional` more elements to be inserted in the given `Deque`.
    ///
    /// # Errors
    ///
    /// Returns `Err(AllocError)` if the buffer is full and cannot be grown.
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), crate::AllocError> {
        let needed_capacity = self.len.checked_add(additional).ok_or(crate::AllocError)?;
        self.grow_at_least(needed_capacity)
    }

    /// Grows the internal storage if len == capacity.
    ///
    /// This function **guarantees** that self.capacity() > self.len() if the
    /// return is `Ok(())`.
    fn grow_if_at_capacity(&mut self) -> Result<(), AllocError> {
        let cap = self.inner.capacity();
        let out = if self.len == cap { self.grow() } else { Ok(()) };
        if out.is_ok() {
            assert!(self.capacity() > self.len());
        }
        out
    }

    /// Pushes an element to the front of the queue.
    ///
    /// Returns `Err(value)` if the buffer is at full capacity.
    pub fn push_front(&mut self, value: T) -> Result<(), T> {
        if self.grow_if_at_capacity().is_err() {
            return Err(value);
        }
        self.head = (self.head + self.capacity() - 1) % self.capacity();
        self.inner.buffer_mut()[self.head].write(value);
        self.len += 1;
        Ok(())
    }

    /// Pushes an element to the back of the queue.
    ///
    /// Returns `Err(value)` if the buffer is at full capacity.
    pub fn push_back(&mut self, value: T) -> Result<(), T> {
        if self.grow_if_at_capacity().is_err() {
            return Err(value);
        }
        let tail = (self.head + self.len) % self.capacity();
        self.inner.buffer_mut()[tail].write(value);
        self.len += 1;
        Ok(())
    }

    /// Pushes an element to the front of the queue. If the queue is full,
    ///
    /// The rearmost element returned
    pub fn force_push_front(&mut self, value: T) -> Option<T> {
        let _ = self.grow_if_at_capacity();

        if self.capacity() == 0 {
            panic!("Can't push to deque with ungrowable capacity of 0");
        }
        let out = if self.len == self.capacity() {
            let tail = (self.head + self.len - 1) % self.capacity();
            // SAFETY: `len == cap` and `cap > 0`, so the tail index is guaranteed initialized.
            let old = unsafe { self.inner.buffer_mut()[tail].assume_init_read() };
            self.len -= 1;
            Some(old)
        } else {
            None
        };
        self.head = (self.head + self.capacity() - 1) % self.capacity();
        self.inner.buffer_mut()[self.head].write(value);
        self.len += 1;
        out
    }

    /// Pushes an element to the back of the queue. If the queue is full,
    ///
    /// The frontmost element is returned
    pub fn force_push_back(&mut self, value: T) -> Option<T> {
        let _ = self.grow_if_at_capacity();

        if self.capacity() == 0 {
            panic!("Can't push to deque with ungrowable capacity of 0");
        }

        let out = if self.len == self.capacity() {
            let head = self.head;
            self.head = (self.head + 1) % self.capacity();
            // SAFETY: `len == cap` and `cap > 0`, so the head index is guaranteed initialized.
            let old = unsafe { self.inner.buffer_mut()[head].assume_init_read() };
            self.len -= 1;
            Some(old)
        } else {
            None
        };
        let tail = (self.head + self.len) % self.capacity();
        self.inner.buffer_mut()[tail].write(value);
        self.len += 1;
        out
    }

    /// Removes and returns the element at the front of the queue, if any.
    pub fn pop_front(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let head = self.head;
        self.head = (self.head + 1) % self.inner.capacity();
        self.len -= 1;
        // SAFETY: `len > 0`, so the head index is guaranteed initialized.
        let val = unsafe { self.inner.buffer_mut()[head].assume_init_read() };
        Some(val)
    }

    /// Removes and returns the element at the back of the queue, if any.
    pub fn pop_back(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let cap = self.inner.capacity();
        let tail = (self.head + self.len - 1) % cap;
        self.len -= 1;
        // SAFETY: `len > 0`, so the tail index is guaranteed initialized.
        let val = unsafe { self.inner.buffer_mut()[tail].assume_init_read() };
        Some(val)
    }

    fn grow(&mut self) -> Result<(), crate::AllocError> {
        let old_cap = self.capacity();
        self.inner.grow()?;
        debug_assert!(
            self.inner.capacity() > old_cap,
            "Capacity should have increased since `grow` succeeded"
        );
        self.finish_grow(old_cap);
        Ok(())
    }

    /// Attempts to grow the underlying storage.
    ///
    /// If wrapped around, shifts the front segment to the new space to maintain contiguity.
    fn grow_at_least(&mut self, needed_capacity: usize) -> Result<(), crate::AllocError> {
        let old_cap = self.inner.capacity();
        self.inner.grow_at_least(needed_capacity)?;
        self.finish_grow(old_cap);
        Ok(())
    }

    fn finish_grow(&mut self, old_cap: usize) {
        // NOTE: We can't use physical_index until we correct our deque.
        if self.is_empty() {
            // Nothing to do since we have no elements to move around
            return;
        }
        let new_cap = self.inner.capacity();
        if new_cap == old_cap {
            // Nothing changed so nothing to do
            return;
        }

        // We must keep elements contiguous
        //
        // H := head
        // L := last element (`self.physical_index(self.len - 1)`)
        //
        //    H             L
        // A [o o o o o o o o ] // no wrap-around, NOP
        //    H             L
        //   [o o o o o o o o . . . . . . . . ]
        //              L H
        // B [o o o o o o o o ] // Move leading elements
        //              L                 H
        //   [o o o o o o . . . . . . . . o o ]

        if self.head <= old_cap - self.len() {
            // Case A: NOP
        } else {
            // Case B: Move [Head, old_cap) to the end
            let count = old_cap - self.head;
            let ptr = self.inner.buffer_mut().as_mut_ptr();
            // SAFETY: The destination pointer is valid since the capacity is now `new_cap`. The original moved-from
            // elements won't be used since self.head is updated to the new tail index.
            unsafe {
                // NOTE: We can't use copy_nonoverlapping since elements could end up on top of
                // one another if we didn't double the capacity.
                ptr::copy(ptr.add(self.head), ptr.add(new_cap - count), count);
            }
            self.head = new_cap - count;
        }
    }

    /// Returns a shared reference to the element at the front of the queue, if any.
    pub fn peek_front(&self) -> Option<&T> {
        self.get(0)
    }

    /// Returns a shared reference to the element at the back of the queue, if any.
    pub fn peek_back(&self) -> Option<&T> {
        self.get(self.len.checked_sub(1)?)
    }

    /// Returns a mutable reference to the element at the front of the queue, if any.
    pub fn peek_front_mut(&mut self) -> Option<&mut T> {
        self.get_mut(0)
    }

    /// Returns a mutable reference to the element at the back of the queue, if any.
    pub fn peek_back_mut(&mut self) -> Option<&mut T> {
        self.get_mut(self.len.checked_sub(1)?)
    }

    /// Returns the physical (addressable) index in the buffer for the given logical index.
    fn physical_index(&self, index: usize) -> Option<usize> {
        if index >= self.len() {
            return None;
        }
        Some((self.head + index) % self.capacity())
    }

    /// Returns a reference to the element at the logical `index` (0 is oldest).
    pub fn get(&self, index: usize) -> Option<&T> {
        let physical_idx = self.physical_index(index)?;
        // SAFETY: physical_idx is valid and initialized because index < len
        unsafe { Some(self.inner.buffer()[physical_idx].assume_init_ref()) }
    }

    /// Returns a reference to the element at the logical `index` (0 is oldest).
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        let physical_idx = self.physical_index(index)?;
        // SAFETY: physical_idx is valid and initialized because index < len
        unsafe { Some(self.inner.buffer_mut()[physical_idx].assume_init_mut()) }
    }

    /// Clears the queue, removing and dropping all elements.
    pub fn clear(&mut self) {
        while let Some(_) = self.pop_front() {}
    }

    /// Removes and returns the element at the front of the queue only if it satisfies `predicate`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sapphire_collections::deque::StackDeque;
    ///
    /// let mut deque = StackDeque::<i32, 3>::new();
    /// deque.push_back(10).unwrap();
    /// deque.push_back(20).unwrap();
    ///
    /// assert_eq!(deque.pop_front_if(|&x| x == 5), None);
    /// assert_eq!(deque.pop_front_if(|&x| x == 10), Some(10));
    /// assert_eq!(deque.peek_front(), Some(&20));
    /// ```
    pub fn pop_front_if<F>(&mut self, predicate: F) -> Option<T>
    where
        F: FnOnce(&T) -> bool,
    {
        if let Some(first) = self.peek_front() {
            if predicate(first) {
                return self.pop_front();
            }
        }
        None
    }

    /// Removes and returns the element at the back of the queue only if it satisfies `predicate`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sapphire_collections::deque::StackDeque;
    ///
    /// let mut deque = StackDeque::<i32, 3>::new();
    /// deque.push_back(10).unwrap();
    /// deque.push_back(20).unwrap();
    ///
    /// assert_eq!(deque.pop_back_if(|&x| x == 5), None);
    /// assert_eq!(deque.pop_back_if(|&x| x == 20), Some(20));
    /// assert_eq!(deque.peek_back(), Some(&10));
    /// ```
    pub fn pop_back_if<F>(&mut self, predicate: F) -> Option<T>
    where
        F: FnOnce(&T) -> bool,
    {
        if let Some(last) = self.peek_back() {
            if predicate(last) {
                return self.pop_back();
            }
        }
        None
    }

    /// Returns an iterator yielding shared references to the elements of the queue in FIFO order.
    ///
    /// # Examples
    ///
    /// ```
    /// use sapphire_collections::deque::StackDeque;
    ///
    /// let mut deque = StackDeque::<i32, 3>::new();
    /// deque.push_back(10).unwrap();
    /// deque.push_back(20).unwrap();
    ///
    /// let mut iter = deque.iter();
    /// assert_eq!(iter.next(), Some(&10));
    /// assert_eq!(iter.next(), Some(&20));
    /// assert_eq!(iter.next(), None);
    /// ```
    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            buffer: self.inner.buffer().into(),
            index: self.physical_index(0).unwrap_or(0),
            count: self.len(),
            _marker: PhantomData,
        }
    }

    /// Returns an iterator yielding mutable references to the elements of the queue in FIFO order.
    ///
    /// # Examples
    ///
    /// ```
    /// use sapphire_collections::deque::StackDeque;
    ///
    /// let mut deque = StackDeque::<i32, 3>::new();
    /// deque.push_back(10).unwrap();
    /// deque.push_back(20).unwrap();
    ///
    /// for x in deque.iter_mut() {
    ///     *x += 100;
    /// }
    ///
    /// let mut iter = deque.iter();
    /// assert_eq!(iter.next(), Some(&110));
    /// assert_eq!(iter.next(), Some(&120));
    /// ```
    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut {
            buffer: self.inner.buffer_mut().into(),
            index: self.physical_index(0).unwrap_or(0),
            count: self.len(),
            _marker: PhantomData,
        }
    }
}

impl<T, A: StorageFamily> Drop for Deque<T, A> {
    fn drop(&mut self) {
        while let Some(val) = self.pop_front() {
            drop(val);
        }
    }
}

/// An iterator yielding shared references to the elements of a [`Deque`] in FIFO order.
pub struct Iter<'a, T> {
    buffer: NonNull<[MaybeUninit<T>]>,
    index: usize,
    count: usize,
    _marker: PhantomData<&'a [MaybeUninit<T>]>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.count == 0 {
            return None;
        }
        let element = unsafe { self.buffer.cast::<MaybeUninit<T>>().add(self.index) };
        self.count -= 1;
        self.index = (self.index + 1) % self.buffer.len();
        // SAFETY: The raw pointer is valid because the lifetime `'a` of the mutable borrow is tied to `_marker`.
        // The elements yielded are distinct on every call because `index` is incremented.
        // Additionally, the element is initialized because it's guaranteed that `index` and `count` will be initialized correctly
        // on the Deque::iter_mut
        Some(unsafe { element.as_ref().assume_init_ref() })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // SAFETY: The raw pointer is valid.
        (self.count, Some(self.count))
    }
}

impl<'a, T> ExactSizeIterator for Iter<'a, T> {}

/// An owning iterator yielding elements of a [`Deque`] in FIFO order by value.
pub struct IntoIter<T, A: StorageFamily> {
    deque: Deque<T, A>,
}

impl<T, A: StorageFamily> Iterator for IntoIter<T, A> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.deque.pop_front()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.deque.len(), Some(self.deque.len()))
    }
}

impl<T, A: StorageFamily> ExactSizeIterator for IntoIter<T, A> {}

impl<T, A: StorageFamily> IntoIterator for Deque<T, A> {
    type Item = T;
    type IntoIter = IntoIter<T, A>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter { deque: self }
    }
}

impl<'a, T, A: StorageFamily> IntoIterator for &'a Deque<T, A> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// An iterator yielding mutable references to the elements of a [`Deque`] in FIFO order.
pub struct IterMut<'a, T> {
    buffer: NonNull<[MaybeUninit<T>]>,
    index: usize,
    count: usize,
    _marker: PhantomData<&'a mut [MaybeUninit<T>]>,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.count == 0 {
            return None;
        }
        let mut element = unsafe { self.buffer.cast::<MaybeUninit<T>>().add(self.index) };
        self.count -= 1;
        self.index = (self.index + 1) % self.buffer.len();
        // SAFETY: The raw pointer is valid because the lifetime `'a` of the mutable borrow is tied to `_marker`.
        // The elements yielded are distinct on every call because `index` is incremented.
        // Additionally, the element is initialized because it's guaranteed that `index` and `count` will be initialized correctly
        // on the Deque::iter_mut
        Some(unsafe { element.as_mut().assume_init_mut() })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // SAFETY: The raw pointer is valid.
        (self.count, Some(self.count))
    }
}

impl<'a, T> ExactSizeIterator for IterMut<'a, T> {}

impl<'a, T, A: StorageFamily> IntoIterator for &'a mut Deque<T, A> {
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

use crate::storage::ArrayStorage;

/// A Deque backed by a stack-allocated fixed-size raw array.
pub type StackDeque<T, const SIZE: usize> = Deque<T, ArrayStorage<SIZE>>;

/// A Deque backed by a standard growable heap-allocated raw buffer.
#[cfg(feature = "std")]
pub type StdDeque<T> = Deque<T, crate::storage::Global>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn test_deque_iterator() {
        let mut deque = StackDeque::<i32, 4>::new();
        deque.push_back(10).unwrap();
        deque.push_back(20).unwrap();
        deque.push_back(30).unwrap();

        // Test Iter by reference
        let mut iter = deque.iter();
        assert_eq!(iter.len(), 3);
        assert_eq!(iter.next(), Some(&10));
        assert_eq!(iter.next(), Some(&20));
        assert_eq!(iter.next(), Some(&30));
        assert_eq!(iter.next(), None);

        // Test IterMut
        for x in deque.iter_mut() {
            *x += 100;
        }
        assert_eq!(*deque.get(0).unwrap(), 110);

        // Test IntoIterator for &mut Deque
        for x in &mut deque {
            *x += 1000;
        }
        assert_eq!(*deque.get(0).unwrap(), 1110);

        // Test IntoIterator for &Deque
        let mut vals = Vec::new();
        for &x in &deque {
            vals.push(x);
        }
        assert_eq!(vals, vec![1110, 1120, 1130]);

        // Test IntoIterator for Deque (consuming)
        let mut vals = Vec::new();
        for x in deque {
            vals.push(x);
        }
        assert_eq!(vals, vec![1110, 1120, 1130]);
    }

    #[test]
    fn test_deque_pop_if() {
        let mut deque = StackDeque::<i32, 4>::new();
        deque.push_back(10).unwrap();
        deque.push_back(20).unwrap();
        deque.push_back(30).unwrap();

        // Pop front if
        assert_eq!(deque.pop_front_if(|&x| x == 5), None);
        assert_eq!(deque.pop_front_if(|&x| x == 10), Some(10));
        assert_eq!(deque.peek_front(), Some(&20));

        // Pop back if
        assert_eq!(deque.pop_back_if(|&x| x == 5), None);
        assert_eq!(deque.pop_back_if(|&x| x == 30), Some(30));
        assert_eq!(deque.peek_back(), Some(&20));
    }

    #[test]
    fn test_deque_basic() {
        let mut deque = StackDeque::<i32, 4>::new();
        assert_eq!(deque.len(), 0);
        assert_eq!(deque.capacity(), 0);

        deque.push_back(10).unwrap();
        assert_eq!(deque.capacity(), 4);
        deque.push_front(20).unwrap();
        // queue: [20, 10]
        assert_eq!(deque.len(), 2);
        assert_eq!(*deque.get(0).unwrap(), 20);
        assert_eq!(*deque.get(1).unwrap(), 10);

        assert_eq!(deque.pop_back(), Some(10));
        assert_eq!(deque.pop_front(), Some(20));
        assert_eq!(deque.len(), 0);
    }

    #[test]
    fn test_deque_wrap_around_and_get() {
        let mut deque = StackDeque::<i32, 3>::new();
        deque.push_back(1).unwrap();
        deque.push_back(2).unwrap();
        deque.push_back(3).unwrap(); // full: [1, 2, 3] (head=0)

        deque.pop_front(); // head becomes 1, len 2: [_, 2, 3]
        deque.push_back(4).unwrap(); // wrapped: [4, _, 3] (head=1, len=3, elements at physical 1, 2, 0)

        assert_eq!(deque.len(), 3);
        assert_eq!(*deque.get(0).unwrap(), 2); // logical 0 is physical 1 (2)
        assert_eq!(*deque.get(1).unwrap(), 3); // logical 1 is physical 2 (3)
        assert_eq!(*deque.get(2).unwrap(), 4); // logical 2 is physical 0 (4)
        assert_eq!(deque.get(3), None);
    }

    #[cfg(feature = "std")]
    #[test]
    fn test_deque_grow_not_wrapped() {
        let mut deque = StdDeque::<i32>::new();
        assert_eq!(deque.capacity(), 0);

        deque.push_back(1).unwrap(); // grows to 1, len 1
        assert_eq!(deque.capacity(), 1);
        assert_eq!(*deque.get(0).unwrap(), 1);

        deque.push_back(2).unwrap(); // grows to 2, len 2
        assert_eq!(deque.capacity(), 2);
        assert_eq!(*deque.get(0).unwrap(), 1);
        assert_eq!(*deque.get(1).unwrap(), 2);

        deque.push_back(3).unwrap(); // grows to 4, len 3
        assert_eq!(deque.capacity(), 4);
        assert_eq!(*deque.get(0).unwrap(), 1);
        assert_eq!(*deque.get(1).unwrap(), 2);
        assert_eq!(*deque.get(2).unwrap(), 3);
    }

    #[cfg(feature = "std")]
    #[test]
    fn test_deque_grow_wrapped() {
        let mut deque = StdDeque::<i32>::new();

        // We want to manually construct a wrapped full deque.
        // Standard StdRawVec grows by doubling: 0 -> 1 -> 2 -> 4.
        deque.push_back(1).unwrap();
        deque.push_back(2).unwrap(); // full at cap 2: [1, 2] (head=0)

        deque.pop_front(); // len 1: [_, 2] (head=1)
        deque.push_back(3).unwrap(); // wrapped full at cap 2: [3, 2] (head=1, len=2, physical 1, 0)

        // Now we push_back(4) which must grow to cap 4 and shift!
        deque.push_back(4).unwrap();

        assert_eq!(deque.capacity(), 4);
        assert_eq!(deque.len(), 3);

        assert_eq!(*deque.get(0).unwrap(), 2);
        assert_eq!(*deque.get(1).unwrap(), 3);
        assert_eq!(*deque.get(2).unwrap(), 4);
    }

    #[test]
    fn test_deque_drop() {
        let counter = Rc::new(Cell::new(0));
        #[derive(Debug)]
        struct DropItem(Rc<Cell<i32>>);
        impl Drop for DropItem {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        {
            let mut deque = StackDeque::<DropItem, 4>::new();
            deque.push_back(DropItem(counter.clone())).unwrap();
            deque.push_back(DropItem(counter.clone())).unwrap();
            deque.pop_front();
            assert_eq!(counter.get(), 1); // popped one dropped
        }
        assert_eq!(counter.get(), 2); // remaining one dropped
    }

    #[test]
    fn test_deque_force_push() {
        // Use a fixed-size StackDeque so it doesn't grow and we can test the "full" case.
        let mut deque = StackDeque::<i32, 3>::new();

        // 1. Test force_push_back on non-full deque (writing to uninitialized slot)
        assert!(deque.force_push_back(1).is_none());
        assert!(deque.force_push_back(2).is_none());
        assert_eq!(deque.len(), 2);
        assert_eq!(deque.get(0).unwrap(), &1);
        assert_eq!(deque.get(1).unwrap(), &2);

        // 2. Test force_push_front on non-full deque (writing to uninitialized slot)
        assert!(deque.force_push_front(3).is_none());
        assert_eq!(deque.len(), 3);
        assert_eq!(deque.get(0).unwrap(), &3);
        assert_eq!(deque.get(1).unwrap(), &1);
        assert_eq!(deque.get(2).unwrap(), &2);

        // Deque is now full: [3, 1, 2] (logical indices: 0->3, 1->1, 2->2)

        // 3. Test force_push_back on full deque (should overwrite and drop the frontmost element, which is 3)
        assert_eq!(deque.force_push_back(4), Some(3));
        assert_eq!(deque.len(), 3);
        assert_eq!(deque.get(0).unwrap(), &1);
        assert_eq!(deque.get(1).unwrap(), &2);
        assert_eq!(deque.get(2).unwrap(), &4);

        // Deque is: [1, 2, 4]

        // 4. Test force_push_front on full deque (should overwrite and drop the rearmost element, which is 4)
        assert_eq!(deque.force_push_front(5), Some(4));
        assert_eq!(deque.len(), 3);
        assert_eq!(deque.get(0).unwrap(), &5);
        assert_eq!(deque.get(1).unwrap(), &1);
        assert_eq!(deque.get(2).unwrap(), &2);
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;
        use std::collections::VecDeque as StdVecDeque;

        #[derive(Debug, Clone)]
        enum DequeOp {
            PushBack(i32),
            PushFront(i32),
            PopBack,
            PopFront,
            Get(usize),
        }

        proptest! {
            #[test]
            fn test_deque_differential(ops in prop::collection::vec(
                prop_oneof![
                    any::<i32>().prop_map(DequeOp::PushBack),
                    any::<i32>().prop_map(DequeOp::PushFront),
                    Just(DequeOp::PopBack),
                    Just(DequeOp::PopFront),
                    any::<usize>().prop_map(DequeOp::Get),
                ],
                0..100
            )) {
                let mut custom_deque = StdDeque::<i32>::new();
                let mut std_deque = StdVecDeque::<i32>::new();

                for op in ops {
                    match op {
                        DequeOp::PushBack(val) => {
                            custom_deque.push_back(val).unwrap();
                            std_deque.push_back(val);
                        }
                        DequeOp::PushFront(val) => {
                            custom_deque.push_front(val).unwrap();
                            std_deque.push_front(val);
                        }
                        DequeOp::PopBack => {
                            assert_eq!(custom_deque.pop_back(), std_deque.pop_back());
                        }
                        DequeOp::PopFront => {
                            assert_eq!(custom_deque.pop_front(), std_deque.pop_front());
                        }
                        DequeOp::Get(idx) => {
                            let len = std_deque.len();
                            if len > 0 {
                                let target_idx = idx % len;
                                assert_eq!(custom_deque.get(target_idx), std_deque.get(target_idx));
                            } else {
                                assert_eq!(custom_deque.get(idx), None);
                            }
                        }
                    }
                    assert_eq!(custom_deque.len(), std_deque.len());

                    for i in 0..std_deque.len() {
                        assert_eq!(custom_deque.get(i), std_deque.get(i));
                    }
                }
            }
        }
    }
}
