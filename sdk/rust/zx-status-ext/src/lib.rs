// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io;
use zx_status::Status;

/// Extension trait for `zx_status::Status` to provide conversions to `std::io` types.
pub trait StatusExt {
    fn into_io_error(self) -> io::Error;
    fn into_io_error_kind(self) -> io::ErrorKind;
}

impl StatusExt for Status {
    fn into_io_error(self) -> io::Error {
        io::Error::from(self.into_io_error_kind())
    }

    fn into_io_error_kind(self) -> io::ErrorKind {
        use std::io::ErrorKind::*;
        match self {
            Status::INTERRUPTED_RETRY => Interrupted,
            Status::BAD_HANDLE => BrokenPipe,
            Status::TIMED_OUT => TimedOut,
            Status::SHOULD_WAIT => WouldBlock,
            Status::PEER_CLOSED => ConnectionAborted,
            Status::NOT_FOUND => NotFound,
            Status::ALREADY_EXISTS => AlreadyExists,
            Status::ALREADY_BOUND => AlreadyExists,
            Status::UNAVAILABLE => AddrNotAvailable,
            Status::ACCESS_DENIED => PermissionDenied,
            Status::IO_REFUSED => ConnectionRefused,
            Status::IO_DATA_INTEGRITY => InvalidData,

            Status::BAD_PATH | Status::INVALID_ARGS | Status::OUT_OF_RANGE | Status::WRONG_TYPE => {
                InvalidInput
            }

            Status::NEXT
            | Status::STOP
            | Status::NO_SPACE
            | Status::FILE_BIG
            | Status::NOT_FILE
            | Status::NOT_DIR
            | Status::IO_DATA_LOSS
            | Status::IO
            | Status::CANCELED
            | Status::BAD_STATE
            | Status::BUFFER_TOO_SMALL
            | Status::BAD_SYSCALL
            | Status::INTERNAL
            | Status::NOT_SUPPORTED
            | Status::NO_RESOURCES
            | Status::NO_MEMORY
            | _ => Other,
        }
    }
}

/// Extension trait for `std::io::ErrorKind` to provide conversions to `zx_status::Status`.
pub trait IoErrorKindExt {
    fn to_status(self) -> Status;
}

impl IoErrorKindExt for io::ErrorKind {
    fn to_status(self) -> Status {
        use std::io::ErrorKind::*;
        match self {
            NotFound => Status::NOT_FOUND,
            PermissionDenied => Status::ACCESS_DENIED,
            ConnectionRefused => Status::IO_REFUSED,
            ConnectionAborted => Status::PEER_CLOSED,
            AddrInUse => Status::ALREADY_BOUND,
            AddrNotAvailable => Status::UNAVAILABLE,
            BrokenPipe => Status::PEER_CLOSED,
            AlreadyExists => Status::ALREADY_EXISTS,
            WouldBlock => Status::SHOULD_WAIT,
            InvalidInput => Status::INVALID_ARGS,
            TimedOut => Status::TIMED_OUT,
            Interrupted => Status::INTERRUPTED_RETRY,
            UnexpectedEof | WriteZero | ConnectionReset | NotConnected | Other | _ => Status::IO,
        }
    }
}
