// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::alloc::Layout;
use core::cell::UnsafeCell;
use core::mem::{MaybeUninit, align_of};
use core::ptr::NonNull;

use crate::AllocError;
use crate::storage::{Storage, StorageFamily};

/// Storage family type that provides inline storage capacity for `N` elements
pub struct ArrayStorage<const N: usize> {}

impl<const N: usize> StorageFamily for ArrayStorage<N> {
    type Storage<T> = InlineStorage<[T; N]>;
}

/// Storage for a single inline element `T`
#[derive(Debug)]
pub struct InlineStorage<T> {
    store: UnsafeCell<MaybeUninit<T>>,
    allocated: bool,
}

impl<T> Default for InlineStorage<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> InlineStorage<T> {
    /// Constructs an unallocated inline storage
    pub const fn new() -> Self {
        Self { store: UnsafeCell::new(MaybeUninit::uninit()), allocated: false }
    }

    /// Returns an [`InlineStorage`] for `N` elements of type `T`
    pub const fn array<const N: usize>() -> InlineStorage<[T; N]> {
        InlineStorage::new()
    }

    fn verify_layout_fits(&self, layout: Layout) -> Result<(), AllocError> {
        if layout.size() > size_of::<T>() || layout.align() > align_of::<T>() {
            return Err(AllocError);
        }
        Ok(())
    }
}

#[non_exhaustive]
#[derive(Debug, Copy, Clone)]
pub struct InlineStorageHandle;

// SAFETY: `resolve` is guaranteed to return the valid allocation pointer associated with the handle
unsafe impl<T> Storage for InlineStorage<T> {
    type Handle = InlineStorageHandle;

    unsafe fn resolve(&self, _handle: Self::Handle) -> NonNull<()> {
        // SAFETY: handle guarantees allocation.
        unsafe { NonNull::new_unchecked(self.store.get()).cast() }
    }

    fn allocate(&mut self, layout: Layout) -> Result<(Self::Handle, usize), AllocError> {
        if self.allocated {
            return Err(AllocError);
        }
        self.verify_layout_fits(layout)?;
        self.allocated = true;
        Ok((InlineStorageHandle, size_of::<T>()))
    }

    unsafe fn deallocate(&mut self, _layout: Layout, _handle: Self::Handle) {
        self.allocated = false;
    }

    unsafe fn grow(
        &mut self,
        _old_layout: Layout,
        new_layout: Layout,
        _handle: Self::Handle,
    ) -> Result<(Self::Handle, usize), AllocError> {
        debug_assert!(self.allocated);
        self.verify_layout_fits(new_layout)?;
        Ok((InlineStorageHandle, size_of::<T>()))
    }

    unsafe fn shrink(
        &mut self,
        _old_layout: Layout,
        new_layout: Layout,
        _handle: Self::Handle,
    ) -> Result<(Self::Handle, usize), AllocError> {
        self.verify_layout_fits(new_layout)?;
        Ok((InlineStorageHandle, size_of::<T>()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vec::Vec;

    #[test]
    fn test_inline_storage_vec() {
        let mut vec = Vec::<i32, InlineStorage<[i32; 4]>>::new();
        assert_eq!(vec.capacity(), 0);

        vec.try_push(10).unwrap();
        assert_eq!(vec.capacity(), 4);
        assert_eq!(vec[0], 10);

        vec.try_push(20).unwrap();
        assert_eq!(vec.capacity(), 4);

        vec.try_push(30).unwrap();
        assert_eq!(vec.capacity(), 4);

        vec.try_push(40).unwrap();
        assert_eq!(vec.len(), 4);
        assert_eq!(vec.capacity(), 4);

        assert!(vec.try_push(50).is_err());
    }
}
