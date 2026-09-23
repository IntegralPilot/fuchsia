// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zx::sys::{zx_handle_t, zx_paddr_t, zx_status_t};

// LINT.IfChange

#[repr(C)]
pub struct FakeBtiPinnedVmoInfo {
    pub vmo: zx_handle_t,
    pub size: u64,
    pub offset: u64,
}

unsafe extern "C" {
    /// All physical addresses returned by zx_bti_pin with a fake BTI will be set to this value.
    pub static g_fake_bti_phys_addr: usize;

    /// Creates a fake BTI object.  `out` is assigned to a handle which can be interacted with using
    /// all of the usual system calls for a BTI object (and can be wrapped in a [`zx::Bti`]).
    ///
    /// The caller takes responsibility for closing `out`.
    ///
    /// # Safety
    ///
    /// `out` must be a valid pointer.
    pub fn fake_bti_create(out: *mut zx_handle_t) -> zx_status_t;

    /// Sets the paddrs used by the fake BTI.  Whenever `zx_bti_pin` is called, these static paddrs
    /// will be written out into the resulting array.  If more physical addresses are needed than
    /// are available in paddrs, `zx_bti_pin` will return an error.
    ///
    /// `bti` must have been created via [`fake_bti_create`], otherwise an error will be returned.
    ///
    /// # Safety
    ///
    /// `paddrs` must be a valid pointer to `count` elements.
    pub fn fake_bti_set_paddrs(
        bti: zx_handle_t,
        paddrs: *const zx_paddr_t,
        count: usize,
    ) -> zx_status_t;

    /// Fake BTI stores all pinned VMOs for testing purposes. Tests can call this method to get
    /// duplicates of all pinned VMO handles, as well as the pinned pages size and offset for each
    /// VMO.  It's the caller's responsibility to close all the returned VMO handles.
    ///
    /// # Safety
    ///
    /// `out_vmo_info` must point to `out_vmo_info_count` elements.
    pub fn fake_bti_get_pinned_vmo(
        bti: zx_handle_t,
        out_vmo_info: *mut FakeBtiPinnedVmoInfo,
        out_vmo_info_count: usize,
        out_actual: *mut usize,
    ) -> zx_status_t;

    /// Fake BTI stores all the fake physical addresses that is returned by |zx_bti_pin|.
    /// Tests can call this method to get the fake physical addresses corresponding to |vmo_info|.
    ///
    /// # Safety
    ///
    /// `out_paddrs` must point to `out_paddrs_count` elements.
    pub fn fake_bti_get_vmo_phys_address(
        bti: zx_handle_t,
        vmo_info: *const FakeBtiPinnedVmoInfo,
        out_paddrs: *mut zx_paddr_t,
        out_paddrs_count: usize,
        out_actual: *mut usize,
    ) -> zx_status_t;
}

// LINT.ThenChange(//sdk/lib/driver/fake-bti/rust/ffi.h)
