// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zx;

unsafe extern "C" {
    #[link_name = "mexec_boot"]
    fn mexec_boot_c(mexec_resource: zx::sys::zx_handle_t) -> zx::sys::zx_status_t;
}

/// Performs an mexec boot.
///
/// This function will obtain the kernel and data ZBIs, prepare them, and then
/// call the mexec syscall.
///
/// `mexec_resource` must be a handle to the mexec resource.
///
/// This function does not return on success. On failure, it returns a
/// `zx::Status`.
pub fn mexec_boot(mexec_resource: zx::Unowned<'_, zx::Resource>) -> Result<(), zx::Status> {
    // SAFETY: The C++ function does not take ownership of the handle.
    let status = unsafe { mexec_boot_c(mexec_resource.raw_handle()) };
    zx::Status::ok(status)?;
    // The mexec_boot function should not return on success. If it does, it's an error.
    Err(zx::Status::INTERNAL)
}
