// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io;

// Implementing these functions ourselves reduces the binary size of the shell.

pub fn io_err_str(e: io::Error) -> String {
    let msg = match e.kind() {
        io::ErrorKind::NotFound => "No such file or directory",
        io::ErrorKind::PermissionDenied => "Permission denied",
        io::ErrorKind::AlreadyExists => "File exists",
        io::ErrorKind::WouldBlock => "Would block",
        io::ErrorKind::InvalidInput => "Invalid argument",
        io::ErrorKind::InvalidData => "Invalid data",
        io::ErrorKind::TimedOut => "Timed out",
        io::ErrorKind::WriteZero => "Write zero",
        io::ErrorKind::Interrupted => "Interrupted",
        io::ErrorKind::UnexpectedEof => "Unexpected EOF",
        _ => "I/O error",
    };
    msg.to_string()
}

#[cfg(target_os = "fuchsia")]
pub fn zx_status_str(status: zx::Status) -> String {
    let msg = match status {
        zx::Status::INTERNAL => "INTERNAL_ERROR",
        zx::Status::NOT_SUPPORTED => "NOT_SUPPORTED",
        zx::Status::NO_RESOURCES => "NO_RESOURCES",
        zx::Status::NO_MEMORY => "NO_MEMORY",
        zx::Status::INVALID_ARGS => "INVALID_ARGS",
        zx::Status::BAD_HANDLE => "BAD_HANDLE",
        zx::Status::WRONG_TYPE => "WRONG_TYPE",
        zx::Status::OUT_OF_RANGE => "OUT_OF_RANGE",
        zx::Status::BUFFER_TOO_SMALL => "BUFFER_TOO_SMALL",
        zx::Status::BAD_STATE => "BAD_STATE",
        zx::Status::TIMED_OUT => "TIMED_OUT",
        zx::Status::SHOULD_WAIT => "SHOULD_WAIT",
        zx::Status::ALREADY_EXISTS => "ALREADY_EXISTS",
        zx::Status::PEER_CLOSED => "PEER_CLOSED",
        zx::Status::NOT_FOUND => "NOT_FOUND",
        zx::Status::ACCESS_DENIED => "ACCESS_DENIED",
        _ => "ZX_STATUS_UNKNOWN",
    };
    msg.to_string()
}
