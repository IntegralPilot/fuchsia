// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::{io_err_str, zx_status_str};
use std::io;

#[test]
fn test_io_err_str() {
    assert_eq!(
        io_err_str(io::Error::new(io::ErrorKind::NotFound, "test")),
        "No such file or directory"
    );
    assert_eq!(
        io_err_str(io::Error::new(io::ErrorKind::PermissionDenied, "test")),
        "Permission denied"
    );
    assert_eq!(io_err_str(io::Error::new(io::ErrorKind::AlreadyExists, "test")), "File exists");
    assert_eq!(io_err_str(io::Error::new(io::ErrorKind::Other, "test")), "I/O error");
}

#[test]
#[cfg(target_os = "fuchsia")]
fn test_zx_status_str() {
    assert_eq!(zx_status_str(zx::Status::NOT_FOUND), "NOT_FOUND");
    assert_eq!(zx_status_str(zx::Status::ACCESS_DENIED), "ACCESS_DENIED");
    assert_eq!(zx_status_str(zx::Status::try_from_raw(-99999).unwrap()), "ZX_STATUS_UNKNOWN");
}
