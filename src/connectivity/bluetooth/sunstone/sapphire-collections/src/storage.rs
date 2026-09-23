// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod storages;
#[cfg(feature = "std")]
pub use storages::Global;
pub use storages::{ArrayStorage, InlineStorage};

use core::alloc::Layout;
use core::ptr::NonNull;

use crate::AllocError;

/// The trait for allocating memory in a storage
///
/// # Safety
///
/// - [`Storage::resolve`] must return a valid pointer to the allocation when passed a valid
///   [`Storage::Handle`]
/// - Storage implementations must handle zero-sized allocations properly, that is a zero-sized
///   allocation shouldn't fail to allocate, and must resolve to properly-aligned memory
pub unsafe trait Storage {
    type Handle: Copy;

    /// Returns a pointer to the allocation represented by `handle`
    ///
    /// # Safety
    ///
    /// - `handle` must be valid
    unsafe fn resolve(&self, handle: Self::Handle) -> NonNull<()>;

    /// Allocates memory with a layout specified by `layout`
    ///
    /// Also returns the total amount of bytes actually allocated, which may be more than requested by `layout`
    fn allocate(&mut self, layout: Layout) -> Result<(Self::Handle, usize), AllocError>;

    /// Deallocates (and invalidates) a [`StorageHandle`] that was allocated with this [`Storage`]
    ///
    /// # Safety
    ///
    /// - `layout` must be the same layout that was used to allocate it, though the size may by
    ///   greater as long as its less than the available capacity returned by any of the allocation
    ///   methods ([`Storage::allocate`]/[`Storage::grow`]/[`Storage::shrink`])
    /// - `handle` must be valid
    unsafe fn deallocate(&mut self, layout: Layout, handle: Self::Handle);

    /// Grows (increases the size of) an allocation
    ///
    /// Similar to [`Storage::allocate`] this method also returns the number of bytes actually
    /// allocated, which may be more than requested with `new_layout`
    ///
    /// - The implementer guarantees that the allocation remains intact, preserving the contents up
    ///   to `old_layout.size()` in the new allocation
    ///
    /// # Safety
    ///
    /// - `new_layout.size() >= old_layout.size()`
    /// - `handle` must be valid
    /// - if this method succeeds, `handle` is now invalid and cannot be used
    unsafe fn grow(
        &mut self,
        old_layout: Layout,
        new_layout: Layout,
        handle: Self::Handle,
    ) -> Result<(Self::Handle, usize), AllocError>;

    /// Shrinks (decreases the size of) an allocation
    ///
    /// Similar to [`Storage::allocate`] this method also returns the number of bytes actually allocated, which may be more than requested with `new_layout`
    ///
    /// # Safety
    ///
    /// - `new_layout.size() <= old_layout.size()`
    /// - `handle` must be valid
    /// - if this method succeeds, `handle` is now invalid and cannot be used
    unsafe fn shrink(
        &mut self,
        old_layout: Layout,
        new_layout: Layout,
        handle: Self::Handle,
    ) -> Result<(Self::Handle, usize), AllocError>;
}

/// Family trait to consolidate storage families around the type `T` that it's used for.
pub trait StorageFamily {
    /// The storage type parameterized over a generic `T`
    type Storage<T>: Storage;
}

impl<S: Storage> StorageFamily for S {
    type Storage<T> = Self;
}
