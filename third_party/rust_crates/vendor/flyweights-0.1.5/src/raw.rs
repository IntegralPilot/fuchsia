// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::alloc::{alloc, dealloc, handle_alloc_error};

use core::alloc::Layout;
use core::ptr::{slice_from_raw_parts, NonNull};
use core::sync::atomic::{AtomicUsize, Ordering};

/// The maximum refcount is `isize::MAX` to match the standard library behavior for `Arc`.
const MAX_REFCOUNT: usize = isize::MAX as usize;

// The header for a memory allocation.
#[repr(C)]
struct Header {
    ref_count: AtomicUsize,
    len: usize,
}

/// A ZST used to indicate a pointer which points to the payload portion of a memory allocation.
///
/// A `Payload` is always located immediately after a `Header`, and is the start of the bytes of the
/// slice. Users always point to the `Payload` because the most common operation we do is `as_slice`
/// and if we already have the payload pointer then we can avoid a pointer offset operation.
#[repr(transparent)]
pub struct Payload {
    // This guarantees `Payload` will always be a ZST with the same alignment as `Header`.`
    _align: [Header; 0],
}

impl Payload {
    #[inline]
    fn layout(len: usize) -> Layout {
        let (layout, byte_offset) = Layout::new::<Header>()
            .extend(Layout::array::<u8>(len).unwrap())
            .expect("attempted to allocate a FlyStr that was too large (~isize::MAX)");

        debug_assert_eq!(byte_offset, size_of::<Header>());
        debug_assert!(layout.align() > 1);

        layout
    }

    /// Returns a pointer to a `Payload` containing a copy of `bytes`.
    pub fn alloc(bytes: &[u8]) -> NonNull<Self> {
        let layout = Self::layout(bytes.len());

        // SAFETY: `layout` always has non-zero size because `Header` has non-zero size.
        let ptr = unsafe { alloc(layout) };
        if ptr.is_null() {
            handle_alloc_error(layout);
        }

        let header = ptr.cast::<Header>();
        // SAFETY: `header` points to memory suitable for a `Header` and is valid for writes.
        unsafe {
            header.write(Header {
                ref_count: AtomicUsize::new(1),
                len: bytes.len(),
            });
        }

        // SAFETY: The size of `Header` fits in an `isize` and `header` points to an allocation
        // of at least that size. Even if the payload is zero-sized, one-past-the-end pointers are
        // valid to create.
        let payload = unsafe { header.add(1).cast::<u8>() };

        // SAFETY: `payload` points to `bytes.len()` bytes, per the constructed layout.
        // `copy_to_nonoverlapping` is a no-op for bytes of length 0, this is always sound to do. We
        // just have to make sure that the pointers are properly aligned, which they always are for
        // `u8` which has an alignment of 1.
        unsafe {
            bytes.as_ptr().copy_to_nonoverlapping(payload, bytes.len());
        }

        // SAFETY: Allocation succeeded, so `payload` is guaranteed not to be null.
        unsafe { NonNull::new_unchecked(payload.cast::<Payload>()) }
    }

    /// Deallocates the payload.
    ///
    /// # Safety
    ///
    /// `ptr` must be to a `Payload` returned from `Payload::alloc`.
    #[inline]
    pub unsafe fn dealloc(ptr: *mut Self) {
        // SAFETY: The caller guaranteed that `ptr` is to a `Payload` returned from `Payload::alloc`.
        let header = unsafe { Self::header(ptr) };
        // SAFETY: `header` points to a `Header` which is valid for reads.
        let len = unsafe { (*header).len };
        let layout = Self::layout(len);

        // SAFETY: `header` points to a memory allocation with layout `layout`.
        unsafe {
            dealloc(header.cast(), layout);
        }
    }

    /// Returns the current refcount of a `Payload`.
    ///
    /// # Safety
    ///
    /// `ptr` must be to a `Payload` returned from `Payload::alloc`.
    #[cfg(test)]
    #[inline]
    pub unsafe fn refcount(ptr: *mut Self) -> usize {
        // SAFETY: The caller guaranteed that `ptr` is to a `Payload` returned from `Payload::alloc`.
        let header = unsafe { Self::header(ptr) };
        // SAFETY: `header` points to a `Header` which is valid for reads and writes.
        unsafe { (*header).ref_count.load(Ordering::Relaxed) }
    }

    /// Increments the refcount of a `Payload` by one.
    ///
    /// # Safety
    ///
    /// `ptr` must be to a `Payload` returned from `Payload::alloc`.
    #[inline]
    pub unsafe fn inc_ref(ptr: *mut Self) -> usize {
        // SAFETY: The caller guaranteed that `ptr` is to a `Payload` returned from `Payload::alloc`.
        let header = unsafe { Self::header(ptr) };
        // SAFETY: `header` points to a `Header` which is valid for reads and writes. Relaxed
        // ordering is sufficient because headers and payloads are immutable. Any other ordering
        // requirements are enforced externally, e.g. by thread synchronization to send data.
        let prev_count = unsafe { (*header).ref_count.fetch_add(1, Ordering::Relaxed) };
        if prev_count > MAX_REFCOUNT {
            std::process::abort();
        }
        prev_count
    }

    /// Decrements the refcount of a `Payload` by one, returning the refcount prior to decrementing.
    ///
    /// Decrementing the refcount to zero does not deallocate the payload. Deallocating the payload
    /// must be done manually.
    ///
    /// # Safety
    ///
    /// `ptr` must be to a `Payload` returned from `Payload::alloc`.
    #[inline]
    pub unsafe fn dec_ref(ptr: *mut Self) -> usize {
        // SAFETY: The caller guaranteed that `ptr` is to a `Payload` returned from `Payload::alloc`.
        let header = unsafe { Self::header(ptr) };
        // SAFETY: `header` points to a `Header` which is valid for reads and writes. Relaxed
        // ordering is sufficient here because the contained value is immutable and can't be
        // modified after creation. We also don't have to drop any data, which isn't a necessary
        // requirement but does provide an additional security.
        unsafe { (*header).ref_count.fetch_sub(1, Ordering::Relaxed) }
    }

    /// # Safety
    ///
    /// `ptr` must be to a `Payload` returned from `Payload::alloc`.
    #[inline]
    unsafe fn header(ptr: *mut Self) -> *mut Header {
        // SAFETY: `Payload` pointers are always preceded by a `Header`.
        unsafe { ptr.cast::<Header>().sub(1) }
    }

    /// Returns the length of the byte slice contained in a `Payload`.
    ///
    /// # Safety
    ///
    /// `ptr` must be to a `Payload` returned from `Payload::alloc`.
    #[inline]
    pub unsafe fn len(ptr: *mut Self) -> usize {
        // SAFETY: The caller guaranteed that `ptr` is to a `Payload` returned from `Payload::alloc`.
        unsafe { (*Self::header(ptr)).len }
    }

    /// Returns a pointer to the byte slice of a `Payload`.
    ///
    /// # Safety
    ///
    /// `ptr` must be to a `Payload` returned from `Payload::alloc`.
    #[inline]
    pub unsafe fn bytes(ptr: *mut Self) -> *const [u8] {
        // SAFETY: The caller guaranteed that `ptr` is to a `Payload` returned from `Payload::alloc`.
        let len = unsafe { Self::len(ptr) };
        slice_from_raw_parts(ptr.cast::<u8>(), len)
    }
}
