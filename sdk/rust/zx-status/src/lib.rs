// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon status.

#![no_std]

use core::fmt;
use core::num::NonZero;

pub mod sys {
    pub use zx_types::{ZX_OK, zx_status_t};
}

macro_rules! assoc_values {
    ($typename:ident, [$($(#[$attr:meta])* $name:ident = $value:path;)*]) => {
        #[allow(non_upper_case_globals)]
        impl $typename {
            $(
                $(#[$attr])*
                pub const $name: $typename = match $typename::try_from_raw($value) {
                    Some(val) => val,
                    None => panic!("Status values must be non-zero"),
                };
            )*

            const fn assoc_const_name(&self) -> Option<&'static str> {
                match self.0.get() {
                    $(
                        $value => Some(stringify!($name)),
                    )*
                    _ => None,
                }
            }
        }

        impl ::core::fmt::Debug for $typename {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_str(concat!(stringify!($typename), "("))?;
                match self.assoc_const_name() {
                    Some(name) => f.write_str(&name)?,
                    None => ::core::fmt::Debug::fmt(&self.0.get(), f)?,
                }
                f.write_str(")")
            }
        }
    };
}

/// A non-zero Zircon status code representing an error.
///
/// Because this wraps a `core::num::NonZero<zx_types::zx_status_t>`, `Result<(), Status>`
/// has a niche at `0` (`ZX_OK`), guaranteeing that `Result<(), Status>` has the exact same
/// 4-byte memory layout and machine ABI as `zx_types::zx_status_t`.
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct Status(NonZero<zx_types::zx_status_t>);

impl Status {
    /// Returns the symbolic name of the status (e.g. `"INVALID_ARGS"`), or `"UNKNOWN"` if
    /// unrecognized.
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self.assoc_const_name() {
            Some(name) => name,
            None => "UNKNOWN",
        }
    }

    /// Returns `Ok(())` if the status was `ZX_OK` (0),
    /// otherwise returns `Err(status)`.
    #[inline]
    pub const fn ok(raw: zx_types::zx_status_t) -> Result<(), Status> {
        match NonZero::new(raw) {
            Some(err) => Err(Status(err)),
            None => Ok(()),
        }
    }

    /// Returns the raw `zx_status_t` code corresponding to a `Result<(), Status>`.
    ///
    /// Returns `ZX_OK` (`0`) for `Ok(())`, and the underlying error code for `Err(status)`.
    #[inline]
    pub const fn result_into_raw(res: Result<(), Self>) -> zx_types::zx_status_t {
        match res {
            Ok(()) => zx_types::ZX_OK,
            Err(status) => status.0.get(),
        }
    }

    /// Returns `Some(status)` if `raw` is not `ZX_OK`, otherwise returns `None`.
    #[inline]
    pub const fn try_from_raw(raw: zx_types::zx_status_t) -> Option<Self> {
        match NonZero::new(raw) {
            Some(err) => Some(Self(err)),
            None => None,
        }
    }

    /// Returns a `Status` for `raw`. If `raw` is `ZX_OK` (`0`), returns `Status::INTERNAL`.
    #[inline]
    pub const fn err_from_raw(raw: zx_types::zx_status_t) -> Self {
        match Self::try_from_raw(raw) {
            Some(status) => status,
            None => Self::INTERNAL,
        }
    }

    #[inline]
    pub const fn into_raw(self) -> zx_types::zx_status_t {
        self.0.get()
    }
}

/// Convenience re-export of `Status::ok`.
pub fn ok(raw: zx_types::zx_status_t) -> Result<(), Status> {
    Status::ok(raw)
}

// LINT.IfChange(zx_status_t)
assoc_values!(Status, [
    #[doc = "The system encountered an otherwise unspecified error while performing the"]
    #[doc = "operation."]
    INTERNAL               = zx_types::ZX_ERR_INTERNAL;
    #[doc = "The operation is not implemented, supported, or enabled."]
    NOT_SUPPORTED          = zx_types::ZX_ERR_NOT_SUPPORTED;
    #[doc = "The system was not able to allocate some resource needed for the operation."]
    NO_RESOURCES           = zx_types::ZX_ERR_NO_RESOURCES;
    #[doc = "The system was not able to allocate memory needed for the operation."]
    NO_MEMORY              = zx_types::ZX_ERR_NO_MEMORY;
    #[doc = "The system call was interrupted, but should be retried. This should not be"]
    #[doc = "seen outside of the VDSO."]
    INTERRUPTED_RETRY      = zx_types::ZX_ERR_INTERRUPTED_RETRY;
    #[doc = "An argument is invalid. For example, a null pointer when a null pointer is"]
    #[doc = "not permitted."]
    INVALID_ARGS           = zx_types::ZX_ERR_INVALID_ARGS;
    #[doc = "A specified handle value does not refer to a handle."]
    BAD_HANDLE             = zx_types::ZX_ERR_BAD_HANDLE;
    #[doc = "The subject of the operation is the wrong type to perform the operation."]
    #[doc = ""]
    #[doc = "For example: Attempting a message_read on a thread handle."]
    WRONG_TYPE             = zx_types::ZX_ERR_WRONG_TYPE;
    #[doc = "The specified syscall number is invalid."]
    BAD_SYSCALL            = zx_types::ZX_ERR_BAD_SYSCALL;
    #[doc = "An argument is outside the valid range for this operation."]
    OUT_OF_RANGE           = zx_types::ZX_ERR_OUT_OF_RANGE;
    #[doc = "The caller-provided buffer is too small for this operation."]
    BUFFER_TOO_SMALL       = zx_types::ZX_ERR_BUFFER_TOO_SMALL;
    #[doc = "The operation failed because the current state of the object does not allow"]
    #[doc = "it, or a precondition of the operation is not satisfied."]
    BAD_STATE              = zx_types::ZX_ERR_BAD_STATE;
    #[doc = "The time limit for the operation elapsed before the operation completed."]
    TIMED_OUT              = zx_types::ZX_ERR_TIMED_OUT;
    #[doc = "The operation cannot be performed currently but potentially could succeed if"]
    #[doc = "the caller waits for a prerequisite to be satisfied, like waiting for a"]
    #[doc = "handle to be readable or writable."]
    #[doc = ""]
    #[doc = "Example: Attempting to read from a channel that has no messages waiting but"]
    #[doc = "has an open remote will return `ZX_ERR_SHOULD_WAIT`. In contrast, attempting"]
    #[doc = "to read from a channel that has no messages waiting and has a closed remote"]
    #[doc = "end will return `ZX_ERR_PEER_CLOSED`."]
    SHOULD_WAIT            = zx_types::ZX_ERR_SHOULD_WAIT;
    #[doc = "The in-progress operation, for example, a wait, has been canceled."]
    CANCELED               = zx_types::ZX_ERR_CANCELED;
    #[doc = "The operation failed because the remote end of the subject of the operation"]
    #[doc = "was closed."]
    PEER_CLOSED            = zx_types::ZX_ERR_PEER_CLOSED;
    #[doc = "The requested entity is not found."]
    NOT_FOUND              = zx_types::ZX_ERR_NOT_FOUND;
    #[doc = "An object with the specified identifier already exists."]
    #[doc = ""]
    #[doc = "Example: Attempting to create a file when a file already exists with that"]
    #[doc = "name."]
    ALREADY_EXISTS         = zx_types::ZX_ERR_ALREADY_EXISTS;
    #[doc = "The operation failed because the named entity is already owned or controlled"]
    #[doc = "by another entity. The operation could succeed later if the current owner"]
    #[doc = "releases the entity."]
    ALREADY_BOUND          = zx_types::ZX_ERR_ALREADY_BOUND;
    #[doc = "The subject of the operation is currently unable to perform the operation."]
    #[doc = ""]
    #[doc = "This is used when there's no direct way for the caller to observe when the"]
    #[doc = "subject will be able to perform the operation and should thus retry."]
    UNAVAILABLE            = zx_types::ZX_ERR_UNAVAILABLE;
    #[doc = "The caller did not have permission to perform the specified operation."]
    ACCESS_DENIED          = zx_types::ZX_ERR_ACCESS_DENIED;
    #[doc = "Otherwise-unspecified error occurred during I/O."]
    IO                     = zx_types::ZX_ERR_IO;
    #[doc = "The entity the I/O operation is being performed on rejected the operation."]
    #[doc = ""]
    #[doc = "Example: an I2C device NAK'ing a transaction or a disk controller rejecting"]
    #[doc = "an invalid command, or a stalled USB endpoint."]
    IO_REFUSED             = zx_types::ZX_ERR_IO_REFUSED;
    #[doc = "The data in the operation failed an integrity check and is possibly"]
    #[doc = "corrupted."]
    #[doc = ""]
    #[doc = "Example: CRC or Parity error."]
    IO_DATA_INTEGRITY      = zx_types::ZX_ERR_IO_DATA_INTEGRITY;
    #[doc = "The data in the operation is currently unavailable and may be permanently"]
    #[doc = "lost."]
    #[doc = ""]
    #[doc = "Example: A disk block is irrecoverably damaged."]
    IO_DATA_LOSS           = zx_types::ZX_ERR_IO_DATA_LOSS;
    #[doc = "The device is no longer available (has been unplugged from the system,"]
    #[doc = "powered down, or the driver has been unloaded)."]
    IO_NOT_PRESENT         = zx_types::ZX_ERR_IO_NOT_PRESENT;
    #[doc = "More data was received from the device than expected."]
    #[doc = ""]
    #[doc = "Example: a USB \"babble\" error due to a device sending more data than the"]
    #[doc = "host queued to receive."]
    IO_OVERRUN             = zx_types::ZX_ERR_IO_OVERRUN;
    #[doc = "An operation did not complete within the required timeframe."]
    #[doc = ""]
    #[doc = "Example: A USB isochronous transfer that failed to complete due to an"]
    #[doc = "overrun or underrun."]
    IO_MISSED_DEADLINE     = zx_types::ZX_ERR_IO_MISSED_DEADLINE;
    #[doc = "The data in the operation is invalid parameter or is out of range."]
    #[doc = ""]
    #[doc = "Example: A USB transfer that failed to complete with TRB Error"]
    IO_INVALID             = zx_types::ZX_ERR_IO_INVALID;
    #[doc = "Path name is too long."]
    BAD_PATH               = zx_types::ZX_ERR_BAD_PATH;
    #[doc = "The object is not a directory or does not support directory operations."]
    #[doc = ""]
    #[doc = "Example: Attempted to open a file as a directory or attempted to do"]
    #[doc = "directory operations on a file."]
    NOT_DIR                = zx_types::ZX_ERR_NOT_DIR;
    #[doc = "Object is not a regular file."]
    NOT_FILE               = zx_types::ZX_ERR_NOT_FILE;
    #[doc = "This operation would cause a file to exceed a filesystem-specific size"]
    #[doc = "limit."]
    FILE_BIG               = zx_types::ZX_ERR_FILE_BIG;
    #[doc = "The filesystem or device space is exhausted."]
    NO_SPACE               = zx_types::ZX_ERR_NO_SPACE;
    #[doc = "The directory is not empty for an operation that requires it to be empty."]
    #[doc = ""]
    #[doc = "For example, non-recursively deleting a directory with files still in it."]
    NOT_EMPTY              = zx_types::ZX_ERR_NOT_EMPTY;
    #[doc = "An indicate to not call again."]
    #[doc = ""]
    #[doc = "The flow control values `ZX_ERR_STOP`, `ZX_ERR_NEXT`, and `ZX_ERR_ASYNC` are"]
    #[doc = "not errors and will never be returned by a system call or public API. They"]
    #[doc = "allow callbacks to request their caller perform some other operation."]
    #[doc = ""]
    #[doc = "For example, a callback might be called on every event until it returns"]
    #[doc = "something other than `ZX_OK`. This status allows differentiation between"]
    #[doc = "\"stop due to an error\" and \"stop because work is done.\""]
    STOP                   = zx_types::ZX_ERR_STOP;
    #[doc = "Advance to the next item."]
    #[doc = ""]
    #[doc = "The flow control values `ZX_ERR_STOP`, `ZX_ERR_NEXT`, and `ZX_ERR_ASYNC` are"]
    #[doc = "not errors and will never be returned by a system call or public API. They"]
    #[doc = "allow callbacks to request their caller perform some other operation."]
    #[doc = ""]
    #[doc = "For example, a callback could use this value to indicate it did not consume"]
    #[doc = "an item passed to it, but by choice, not due to an error condition."]
    NEXT                   = zx_types::ZX_ERR_NEXT;
    #[doc = "Ownership of the item has moved to an asynchronous worker."]
    #[doc = ""]
    #[doc = "The flow control values `ZX_ERR_STOP`, `ZX_ERR_NEXT`, and `ZX_ERR_ASYNC` are"]
    #[doc = "not errors and will never be returned by a system call or public API. They"]
    #[doc = "allow callbacks to request their caller perform some other operation."]
    #[doc = ""]
    #[doc = "Unlike `ZX_ERR_STOP`, which implies that iteration on an object"]
    #[doc = "should stop, and `ZX_ERR_NEXT`, which implies that iteration"]
    #[doc = "should continue to the next item, `ZX_ERR_ASYNC` implies"]
    #[doc = "that an asynchronous worker is responsible for continuing iteration."]
    #[doc = ""]
    #[doc = "For example, a callback will be called on every event, but one event needs"]
    #[doc = "to handle some work asynchronously before it can continue. `ZX_ERR_ASYNC`"]
    #[doc = "implies the worker is responsible for resuming iteration once its work has"]
    #[doc = "completed."]
    ASYNC                  = zx_types::ZX_ERR_ASYNC;
    #[doc = "The specified protocol is not supported."]
    PROTOCOL_NOT_SUPPORTED = zx_types::ZX_ERR_PROTOCOL_NOT_SUPPORTED;
    #[doc = "The host is unreachable."]
    ADDRESS_UNREACHABLE    = zx_types::ZX_ERR_ADDRESS_UNREACHABLE;
    #[doc = "Address is being used by someone else."]
    ADDRESS_IN_USE         = zx_types::ZX_ERR_ADDRESS_IN_USE;
    #[doc = "The socket is not connected."]
    NOT_CONNECTED          = zx_types::ZX_ERR_NOT_CONNECTED;
    #[doc = "The remote peer rejected the connection."]
    CONNECTION_REFUSED     = zx_types::ZX_ERR_CONNECTION_REFUSED;
    #[doc = "The connection was reset."]
    CONNECTION_RESET       = zx_types::ZX_ERR_CONNECTION_RESET;
    #[doc = "The connection was aborted."]
    CONNECTION_ABORTED     = zx_types::ZX_ERR_CONNECTION_ABORTED;
]);
// LINT.ThenChange(//zircon/vdso/errors.fidl)

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.assoc_const_name() {
            Some(name) => name.fmt(f),
            None => write!(f, "Unknown zircon status code: {}", self.0.get()),
        }
    }
}

impl core::error::Error for Status {}

impl From<NonZero<zx_types::zx_status_t>> for Status {
    #[inline]
    fn from(val: NonZero<zx_types::zx_status_t>) -> Self {
        Status(val)
    }
}

impl From<Status> for NonZero<zx_types::zx_status_t> {
    #[inline]
    fn from(status: Status) -> Self {
        status.0
    }
}

impl From<Status> for zx_types::zx_status_t {
    #[inline]
    fn from(status: Status) -> zx_types::zx_status_t {
        status.into_raw()
    }
}

impl TryFrom<zx_types::zx_status_t> for Status {
    type Error = TryFromRawError;

    #[inline]
    fn try_from(raw: zx_types::zx_status_t) -> Result<Self, Self::Error> {
        Status::try_from_raw(raw).ok_or(TryFromRawError)
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TryFromRawError;

impl fmt::Display for TryFromRawError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot convert ZX_OK (0) to Status")
    }
}

impl core::error::Error for TryFromRawError {}

impl From<Status> for Result<(), Status> {
    #[inline]
    fn from(src: Status) -> Result<(), Status> {
        Err(src)
    }
}

impl From<core::convert::Infallible> for Status {
    fn from(x: core::convert::Infallible) -> Status {
        match x {}
    }
}

#[cfg(test)]
mod test {
    extern crate std;
    use super::Status;

    #[test]
    fn status_debug_format() {
        let cases = [
            ("Status(BAD_SYSCALL)", Status::BAD_SYSCALL),
            ("Status(NEXT)", Status::NEXT),
            ("Status(-5050)", Status::err_from_raw(-5050)),
        ];
        for &(expected, value) in &cases {
            assert_eq!(expected, std::format!("{value:?}"));
        }
    }

    #[test]
    fn status_into_result() {
        let err_result: Result<(), Status> = Status::BAD_SYSCALL.into();
        assert_eq!(err_result, Err(Status::BAD_SYSCALL));
    }

    #[test]
    fn status_layout() {
        use core::mem::size_of;
        assert_eq!(size_of::<Status>(), 4);
        assert_eq!(size_of::<Result<(), Status>>(), 4);
    }

    #[test]
    fn test_err_from_raw() {
        assert_eq!(Status::err_from_raw(zx_types::ZX_OK), Status::INTERNAL);
        assert_eq!(Status::err_from_raw(zx_types::ZX_ERR_NOT_FOUND), Status::NOT_FOUND);
    }

    #[test]
    fn test_try_from() {
        use super::TryFromRawError;
        assert_eq!(Status::try_from(zx_types::ZX_OK), Err(TryFromRawError));
        assert_eq!(Status::try_from(zx_types::ZX_ERR_NOT_FOUND), Ok(Status::NOT_FOUND));
    }

    #[test]
    fn test_as_str() {
        assert_eq!(Status::INVALID_ARGS.as_str(), "INVALID_ARGS");
        assert_eq!(Status::err_from_raw(-9999).as_str(), "UNKNOWN");
    }
}
