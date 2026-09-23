// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::vm::compressor::VmCompressor;
use compression_bindings as bindings;
use core::marker::{PhantomData, PhantomPinned};
use core::pin::Pin;
use pin_init::{PinInit, pin_data};
use zr::{Opaque, pin_init_ffi};

// Note: The upstream C++ `VmCompression` comments and documentation are intentionally not copied
// over yet.
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct VmCompression {
    raw: Opaque<bindings::VmCompression>,
    phantom: PhantomData<PhantomPinned>,
}

zr::unsafe_pinned_drop_ffi!(VmCompression, bindings::cpp_vmcompression_destroy);

impl VmCompression {
    /// Domain-specific conversion: returns raw pointer for `VmCompression`.
    pub fn as_raw(&self) -> *mut bindings::VmCompression {
        self.raw.get()
    }
}

/// An RAII wrapper around holding a locked reference to a `VmCompressor`.
///
/// Acquiring a compressor may block until one becomes available, and callers should be prepared
/// for extended wait times. The guard must not outlive the `VmCompression` it was acquired from.
#[pin_data(PinnedDrop)]
#[repr(transparent)]
pub struct CompressorGuard<'a> {
    opaque: Opaque<bindings::VmCompression_CompressorGuard>,
    phantom: PhantomData<&'a VmCompression>,
    // The C++ guard owns a held `Guard<Mutex>`, which may only be relocated by its move
    // constructor. Marking this field `#[pin]` is what makes the type `!Unpin`, and so prevents
    // safe code from moving it out of the `Pin` it is constructed in.
    #[pin]
    _pin: PhantomPinned,
}

zr::unsafe_pinned_drop_ffi!(
    CompressorGuard<'_>,
    bindings::cpp_vmcompression_compressor_guard_destroy
);

impl<'a> CompressorGuard<'a> {
    /// Retrieves a compressor from `compression`, wrapped in an RAII guard. Once the compressor is
    /// finished with it can be dropped, which will release it for reuse.
    pub fn new(compression: &'a VmCompression) -> impl PinInit<Self> {
        pin_init_ffi!(bindings::cpp_vmcompression_acquire_compressor, compression.as_raw())
    }

    /// Returns a reference to the guarded `VmCompressor`.
    pub fn get(self: Pin<&mut Self>) -> &VmCompressor {
        // SAFETY: Nothing is moved out of the returned reference; it is only used to read
        // `opaque`'s address.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: `this.opaque.get()` points to a live `CompressorGuard`.
        let raw = unsafe { bindings::cpp_vmcompression_compressor_guard_get(this.opaque.get()) };
        // SAFETY: The shim returns the address of a reference, so it is never null, and
        // `VmCompressor` is a `repr(C)` wrapper whose only field is the corresponding opaque
        // binding type. The compressor is owned by the guard, so it stays live for as long as
        // `this` is borrowed.
        unsafe { &*raw.cast::<VmCompressor>() }
    }
}

impl fbl::HasRefCount for VmCompression {
    fn ref_count(&self) -> &fbl::RefCounted {
        // SAFETY: `cpp_vmcompression_get_ref_counted` returns a valid pointer to the C++
        // `fbl::RefCounted` subobject of `VmCompression`.
        unsafe {
            &*(bindings::cpp_vmcompression_get_ref_counted(self.as_raw()) as *const fbl::RefCounted)
        }
    }
}

unsafe impl fbl::Recyclable for VmCompression {
    unsafe fn recycle(ptr: core::ptr::NonNull<Self>) {
        unsafe { bindings::cpp_vmcompression_free(ptr.as_ptr() as *mut bindings::VmCompression) }
    }
}
