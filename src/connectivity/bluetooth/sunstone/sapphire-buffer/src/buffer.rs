// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(feature = "std")]
extern crate std;

use core::marker::PhantomData;
use core::mem::ManuallyDrop;

/// Error indicating that a view operation exceeded active slice bounds or underlying buffer limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfBounds;

/// An unbound handle that must be bound into a [`Buffer`] before use or consumed via [`into_raw`](Self::into_raw).
pub struct UnboundHandle<'a> {
    raw: usize,
    _marker: PhantomData<fn(&'a ()) -> &'a ()>,
}

impl<'a> UnboundHandle<'a> {
    /// Creates a new unbound handle for a buffer.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that `raw` represents a valid handle for the associated
    /// [`BufferAccessor`], that it is unique for lease `'a`, and that it will remain valid until
    /// released.
    #[inline(always)]
    pub unsafe fn new(raw: usize) -> Self {
        Self { raw, _marker: PhantomData }
    }

    /// Consumes this handle without releasing its underlying buffer, returning the raw identifier.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring the returned identifier remains valid until used or
    /// released without invalid aliasing or double-release.
    #[inline(always)]
    pub unsafe fn into_raw(self) -> usize {
        ManuallyDrop::new(self).raw
    }

    #[inline(always)]
    fn bind(self) -> Handle<'a> {
        // SAFETY: UnboundHandle::new guarantees that self.raw is a valid handle identifier for `'a`.
        Handle { raw: unsafe { self.into_raw() }, _marker: PhantomData }
    }
}

impl<'a> Drop for UnboundHandle<'a> {
    fn drop(&mut self) {
        #[cfg(feature = "std")]
        if std::thread::panicking() {
            return;
        }
        panic!("UnboundHandle dropped without binding to a Buffer");
    }
}

/// Opaque identifier representing an underlying byte buffer within a [`BufferAccessor`].
#[derive(Debug, PartialEq, Eq)]
pub struct Handle<'a> {
    raw: usize,
    _marker: PhantomData<&'a ()>,
}

impl<'a> Handle<'a> {
    /// Returns the raw handle identifier.
    #[inline(always)]
    pub fn get(&self) -> usize {
        self.raw
    }

    /// Mutates the handle's raw identifier.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `raw` represents a valid handle for the associated
    /// [`BufferAccessor`] and that no invalid aliasing occurs.
    #[inline(always)]
    pub unsafe fn set(&mut self, raw: usize) {
        self.raw = raw;
    }
}

/// A contiguous byte buffer view backed by a [`BufferAccessor`].
pub struct Buffer<'a> {
    accessor: &'a dyn BufferAccessor,
    handle: Handle<'a>,
}

impl<'a> core::fmt::Debug for Buffer<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Buffer")
            .field("handle", &self.handle)
            .field("len", &self.len())
            .field("capacity", &self.capacity())
            .field("view", &self.as_slice())
            .finish()
    }
}

impl<'a> Buffer<'a> {
    /// Constructs a new `Buffer` instance from an accessor and an [`UnboundHandle`].
    ///
    /// # Safety
    ///
    /// The caller must guarantee that `unbound` was minted by `accessor` for lease `'a` and has
    /// not yet been released.
    #[inline(always)]
    pub unsafe fn new(accessor: &'a dyn BufferAccessor, unbound: UnboundHandle<'a>) -> Self {
        Self { accessor, handle: unbound.bind() }
    }

    /// Returns the current active slice view length in bytes.
    #[inline(always)]
    pub fn len(&self) -> usize {
        // SAFETY: self.handle is a valid handle for self.accessor per Buffer::new safety contract.
        unsafe { self.accessor.len(&self.handle) }
    }

    /// Returns `true` if the current active slice view is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the total underlying allocation capacity in bytes.
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        // SAFETY: self.handle is a valid handle for self.accessor per Buffer::new safety contract.
        unsafe { self.accessor.capacity(&self.handle) }
    }

    /// Advances the front of the view by `offset` bytes.
    #[inline(always)]
    pub fn truncate_front(&mut self, offset: usize) -> Result<(), OutOfBounds> {
        // SAFETY: self.handle is a valid handle for self.accessor per Buffer::new safety contract.
        unsafe { self.accessor.truncate_front(&mut self.handle, offset) }
    }

    /// Reduces the back of the view by `size` bytes.
    #[inline(always)]
    pub fn truncate_back(&mut self, size: usize) -> Result<(), OutOfBounds> {
        // SAFETY: self.handle is a valid handle for self.accessor per Buffer::new safety contract.
        unsafe { self.accessor.truncate_back(&mut self.handle, size) }
    }

    /// Restores `size` bytes to the front of the view that were previously truncated.
    #[inline(always)]
    pub fn reclaim_front(&mut self, size: usize) -> Result<(), OutOfBounds> {
        // SAFETY: self.handle is a valid handle for self.accessor per Buffer::new safety contract.
        unsafe { self.accessor.reclaim_front(&mut self.handle, size) }
    }

    /// Restores `size` bytes to the back of the view that were previously truncated.
    #[inline(always)]
    pub fn reclaim_back(&mut self, size: usize) -> Result<(), OutOfBounds> {
        // SAFETY: self.handle is a valid handle for self.accessor per Buffer::new safety contract.
        unsafe { self.accessor.reclaim_back(&mut self.handle, size) }
    }

    /// Resets the view to cover the entire underlying buffer allocation.
    #[inline(always)]
    pub fn reset_view(&mut self) {
        // SAFETY: self.handle is a valid handle for self.accessor per Buffer::new safety contract.
        unsafe { self.accessor.reset_view(&mut self.handle) }
    }

    /// Returns an immutable slice representing the current buffer view.
    #[inline(always)]
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: self.handle is a valid handle for self.accessor per Buffer::new safety contract.
        unsafe { self.accessor.as_slice(&self.handle) }
    }

    /// Returns a mutable slice representing the current buffer view.
    #[inline(always)]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: self.handle is a valid handle for self.accessor per Buffer::new safety contract.
        unsafe { self.accessor.as_mut_slice(&mut self.handle) }
    }

    /// Splits this buffer into its accessor reference and an [`UnboundHandle`], without releasing the buffer.
    #[inline(always)]
    pub fn into_parts(self) -> (&'a dyn BufferAccessor, UnboundHandle<'a>) {
        let parts = core::mem::ManuallyDrop::new(self);
        // SAFETY: self.handle was guaranteed valid for lease `'a` when Buffer was constructed.
        let unbound = unsafe { UnboundHandle::new(parts.handle.get()) };
        (parts.accessor, unbound)
    }
}

impl<'a> AsRef<[u8]> for Buffer<'a> {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl<'a> AsMut<[u8]> for Buffer<'a> {
    fn as_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

impl<'a> Drop for Buffer<'a> {
    fn drop(&mut self) {
        // SAFETY: self.handle is a valid handle for self.accessor per Buffer::new safety contract.
        unsafe { self.accessor.release(&mut self.handle) }
    }
}

/// Trait for inspecting, resizing, slicing, and releasing underlying byte buffers.
pub trait BufferAccessor: core::any::Any {
    /// Returns the current active slice view length of the buffer identified by `handle`.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid handle identifier minted by this accessor that has not yet been released.
    unsafe fn len(&self, handle: &Handle<'_>) -> usize;

    /// Returns the total original allocated capacity of the buffer identified by `handle`.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid handle identifier minted by this accessor that has not yet been released.
    unsafe fn capacity(&self, handle: &Handle<'_>) -> usize;

    /// Advances the front of the view by `offset` bytes.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid handle identifier minted by this accessor that has not yet been released.
    unsafe fn truncate_front(
        &self,
        handle: &mut Handle<'_>,
        offset: usize,
    ) -> Result<(), OutOfBounds>;

    /// Reduces the back of the view by `size` bytes.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid handle identifier minted by this accessor that has not yet been released.
    unsafe fn truncate_back(&self, handle: &mut Handle<'_>, size: usize)
    -> Result<(), OutOfBounds>;

    /// Restores `size` bytes to the front of the view that were previously truncated.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid handle identifier minted by this accessor that has not yet been released.
    unsafe fn reclaim_front(&self, handle: &mut Handle<'_>, size: usize)
    -> Result<(), OutOfBounds>;

    /// Restores `size` bytes to the back of the view that were previously truncated.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid handle identifier minted by this accessor that has not yet been released.
    unsafe fn reclaim_back(&self, handle: &mut Handle<'_>, size: usize) -> Result<(), OutOfBounds>;

    /// Resets the view to cover the entire underlying buffer capacity.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid handle identifier minted by this accessor that has not yet been released.
    unsafe fn reset_view(&self, handle: &mut Handle<'_>);

    /// Returns an immutable slice representing the current buffer view.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid handle identifier minted by this accessor that has not yet been released.
    unsafe fn as_slice<'b>(&self, handle: &'b Handle<'_>) -> &'b [u8];

    /// Returns a mutable slice representing the current buffer view.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid handle identifier minted by this accessor that has not yet been released.
    unsafe fn as_mut_slice<'b>(&self, handle: &'b mut Handle<'_>) -> &'b mut [u8];

    /// Releases the underlying buffer resources identified by `handle`.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid handle identifier minted by this accessor that has not yet been released.
    /// After calling this method, `handle` must never be used again.
    unsafe fn release(&self, handle: &mut Handle<'_>);
}
