// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow};
use libc::{c_char, c_int};
use std::ffi::CString;

// From libffi.so
unsafe extern "C" {
    fn blobfs_export_blobs(source_path: *const c_char, output_path: *const c_char) -> c_int;
}

/// Attempts to extract the `source_path` blobfs to the `output_path` directory.
pub fn blobfs_export(source_path: &str, output_path: &str) -> Result<()> {
    unsafe {
        let c_source_path = CString::new(source_path).unwrap();
        let c_output_path = CString::new(output_path).unwrap();

        let result = blobfs_export_blobs(
            c_source_path.as_ptr() as *const c_char,
            c_output_path.as_ptr() as *const c_char,
        );

        if result == 0 {
            return Ok(());
        } else {
            Err(anyhow!(format!("Failed to extract blobfs error_code: {}", result)))
        }
    }
}
