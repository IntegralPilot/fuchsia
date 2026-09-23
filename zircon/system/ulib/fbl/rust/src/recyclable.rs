// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::ffi::c_void;
use core::ptr::NonNull;
use kalloc::AllocError;

/// Trait for types that can be recycled (deallocated).
///
/// # Safety
///
/// Implementing this trait is unsafe because the implementer must ensure that:
/// - `allocate`, if overridden, returns a valid pointer that can be safely deallocated by
///   `recycle`.
/// - `recycle` correctly deallocates the pointer and does not cause double-free or use-after-free.
pub unsafe trait Recyclable: Sized {
    /// Allocates a new instance of `Self`.
    ///
    /// The default implementation returns `Err(AllocError)`, which is appropriate for types
    /// that cannot be allocated from Rust (e.g., C++ ref-counted objects).
    fn allocate(_value: Self) -> Result<NonNull<Self>, AllocError> {
        Err(AllocError)
    }

    /// Recycles the object.
    ///
    /// # Safety
    ///
    /// - The caller must ensure that `ptr` points to a valid, fully-initialized
    ///   instance of `Self` that has no other references.
    /// - The caller must not use the pointer or any references derived from it
    ///   after this call.
    unsafe fn recycle(ptr: NonNull<Self>);

    /// Helper for FFI functions to call `recycle`.
    ///
    /// # Safety
    ///
    /// - The caller must ensure that `ptr` is valid and points to a valid object
    ///   of type `Self` that has no other references.
    /// - The caller must not use the pointer or any references derived from it
    ///   after this call.
    unsafe fn recycle_ffi(ptr: *mut c_void) {
        // SAFETY: The caller of `recycle_ffi` must ensure that `ptr` is non-null
        // and points to a valid `Self` that can be safely recycled.
        unsafe {
            Self::recycle(NonNull::new_unchecked(ptr as *mut Self));
        }
    }
}

/// Trait for types that can be allocated in an uninitialized state.
///
/// # Safety
///
/// Implementing this trait is unsafe because the implementer must ensure that:
/// - `allocate_uninit` returns a valid pointer to uninitialized memory that can
///   be safely deallocated by `recycle_uninit`.
/// - `recycle_uninit` correctly deallocates the pointer without dropping the content
///   (as it is uninitialized) and does not cause double-free or use-after-free.
pub unsafe trait UninitRecyclable: Recyclable {
    /// Allocates a new uninitialized instance of `Self`.
    fn allocate_uninit() -> Result<NonNull<core::mem::MaybeUninit<Self>>, AllocError>;

    /// Recycles an uninitialized object.
    ///
    /// # Safety
    ///
    /// - The caller must ensure that `ptr` points to a valid (but possibly uninitialized)
    ///   instance of `Self` that has no other references.
    /// - The caller must not use the pointer or any references derived from it
    ///   after this call.
    unsafe fn recycle_uninit(ptr: NonNull<core::mem::MaybeUninit<Self>>);
}
