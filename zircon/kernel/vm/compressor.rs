// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use compressor_bindings as bindings;
use core::marker::{PhantomData, PhantomPinned};
use zr::Opaque;
use zx_status::Status;

// Note: The upstream C++ `VmCompressor` comments and documentation are intentionally not copied
// over yet.
#[repr(C)]
pub struct VmCompressor {
    raw: Opaque<bindings::VmCompressor>,
    phantom: PhantomData<PhantomPinned>,
}

impl VmCompressor {
    /// Domain-specific conversion: returns raw pointer for `VmCompressor`.
    pub fn as_raw(&self) -> *mut bindings::VmCompressor {
        self.raw.get()
    }

    /// Arms the compressor, ensuring the backup page is allocated. This must be called prior to
    /// starting compression.
    pub fn arm(&self) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` points to a live `VmCompressor`.
        let status = unsafe { bindings::cpp_vmcompressor_arm(self.as_raw()) };
        Status::ok(status)
    }
}
