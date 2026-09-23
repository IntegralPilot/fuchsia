// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod raw_api {
    unsafe extern "C" {
        /// Adjusts parameters or performs specific maintenance operations on the memory
        /// allocator. Returns 1 on success, 0 on error.
        pub fn mallopt(
            param: std::os::raw::c_int,
            value: std::os::raw::c_int,
        ) -> std::os::raw::c_int;
    }
}

// mallopt() option to set the minimum time between attempts to release unused pages to
// to the operating system in milliseconds. The value is clamped on allocator config
// `MinReleaseToOsIntervalMs` and `MinReleaseToOsIntervalMs`.
pub const M_DECAY_TIME: i32 = -100;

// mallopt() option to immediately purge any memory not in use. This will release the memory back
// to the kernel. The value is ignored.
pub const M_PURGE: i32 = -101;

// mallopt() option to immediately purge all possible memory back to the kernel. This call can take
// longer than a normal purge since it examines everything. In some cases, it can take more than
// twice the time of a M_PURGE call. The value is ignored.
pub const M_PURGE_ALL: i32 = -104;

/// mallopt() option to set the minimum time between attempts to release unused pages to
/// the operating system in milliseconds. The value is clamped on allocator config
/// `MinReleaseToOsIntervalMs` and `MinReleaseToOsIntervalMs`.
pub fn mallopt(param: i32, value: i32) -> Result<(), ()> {
    let result = unsafe { raw_api::mallopt(param, value) };
    if result == 1 { Ok(()) } else { Err(()) }
}
