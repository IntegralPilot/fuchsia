// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::marker::{PhantomData, PhantomPinned};
use evictor_bindings as bindings;
use pin_init::{PinInit, pin_data};
use zr::Opaque;

// Note: The upstream C++ `Evictor` comments and documentation are intentionally not copied
// over yet.
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct Evictor {
    raw: Opaque<bindings::Evictor>,
    phantom: PhantomData<PhantomPinned>,
}

zr::unsafe_pinned_drop_ffi!(Evictor, bindings::cpp_evictor_destroy);

impl Evictor {
    pub fn init() -> impl PinInit<Self, core::convert::Infallible> {
        zr::pin_init_ffi!(bindings::cpp_evictor_init)
    }
    /// Domain-specific conversion: returns raw pointer for `Evictor`.
    pub fn as_raw(&self) -> *mut bindings::Evictor {
        self.raw.get()
    }
}
