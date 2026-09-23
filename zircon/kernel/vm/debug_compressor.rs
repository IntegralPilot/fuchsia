// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::marker::{PhantomData, PhantomPinned};
use page_queues_bindings as bindings;
use pin_init::{PinInit, pin_data};
use zr::Opaque;

// Note: The upstream C++ `DebugCompressor` comments and documentation are intentionally not copied
// over yet.
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct DebugCompressor {
    raw: Opaque<bindings::VmDebugCompressor>,
    phantom: PhantomData<PhantomPinned>,
}

zr::unsafe_pinned_drop_ffi!(DebugCompressor, bindings::cpp_debug_compressor_destroy);

impl DebugCompressor {
    pub fn init() -> impl PinInit<Self, core::convert::Infallible> {
        zr::pin_init_ffi!(bindings::cpp_debug_compressor_init)
    }
    /// Domain-specific conversion: returns raw pointer for `DebugCompressor`.
    pub fn as_raw(&self) -> *mut bindings::VmDebugCompressor {
        self.raw.get()
    }
}
