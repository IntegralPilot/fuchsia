// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon resources.

use crate::{BootInstant, Koid, NullableHandle, Resource, Status, ok, sys};
use bitflags::bitflags;
use bstr::BStr;

/// An object representing a Zircon 'debuglog' object.
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct DebugLog(NullableHandle);
impl_handle_based!(DebugLog);

bitflags! {
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct DebugLogOpts: u32 {
        const READABLE = sys::ZX_LOG_FLAG_READABLE;
    }
}

impl DebugLog {
    /// Create a debug log object that allows access to read from and write to the kernel debug
    /// logging facility.
    ///
    /// Wraps the
    /// [zx_debuglog_create]((https://fuchsia.dev/fuchsia-src/reference/syscalls/debuglog_create.md)
    /// syscall.
    pub fn create(resource: &Resource, opts: DebugLogOpts) -> Result<DebugLog, Status> {
        let mut handle = 0;
        let status =
            unsafe { sys::zx_debuglog_create(resource.raw_handle(), opts.bits(), &mut handle) };
        ok(status)?;
        unsafe { Ok(DebugLog::from(NullableHandle::from_raw(handle))) }
    }

    /// Write a message to the kernel debug log.
    ///
    /// Wraps the
    /// [zx_debuglog_write]((https://fuchsia.dev/fuchsia-src/reference/syscalls/debuglog_write.md)
    /// syscall.
    pub fn write(&self, message: &[u8]) -> Result<(), Status> {
        // TODO(https://fxbug.dev/42108144): Discussion ongoing over whether debuglog levels are supported, so no
        // options parameter for now.
        let status = unsafe {
            sys::zx_debuglog_write(self.raw_handle(), 0, message.as_ptr(), message.len())
        };
        ok(status)
    }

    /// Read a single log record from the kernel debug log.
    ///
    /// The DebugLog object must have been created with DebugLogOpts::READABLE, or this will return
    /// an error.
    ///
    /// Wraps the
    /// [zx_debuglog_read]((https://fuchsia.dev/fuchsia-src/reference/syscalls/debuglog_read.md)
    /// syscall.
    pub fn read(&self) -> Result<DebugLogRecord, Status> {
        let mut record = sys::zx_log_record_t::default();
        let bytes_written = unsafe {
            sys::zx_debuglog_read(
                self.raw_handle(),
                0, /* options are unused, must be 0 */
                std::ptr::from_mut(&mut record).cast::<u8>(),
                std::mem::size_of_val(&record),
            )
        };
        // On error, zx_debuglog_read returns a negative value. All other values indicate success.
        if bytes_written < 0 {
            Err(Status::err_from_raw(bytes_written))
        } else {
            DebugLogRecord::from_raw(&record)
        }
    }
}

/// A record from the kernel's debuglog.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct DebugLogRecord {
    pub sequence: u64,
    pub timestamp: BootInstant,
    pub severity: DebugLogSeverity,
    pub pid: Koid,
    pub tid: Koid,
    pub flags: u8,
    data: [u8; sys::ZX_LOG_RECORD_DATA_MAX],
    datalen: u16,
}

impl DebugLogRecord {
    /// Convert a raw debuglog record into this typed wrapper.
    pub fn from_raw(raw: &sys::zx_log_record_t) -> Result<Self, Status> {
        if raw.header.datalen <= sys::ZX_LOG_RECORD_DATA_MAX as u16 {
            Ok(Self {
                timestamp: BootInstant::from_nanos(raw.header.timestamp),
                sequence: raw.header.sequence,
                severity: DebugLogSeverity::from_raw(raw.header.severity),
                pid: Koid::from_raw(raw.header.pid),
                tid: Koid::from_raw(raw.header.tid),
                flags: raw.header.flags,
                data: raw.data,
                datalen: raw.header.datalen,
            })
        } else {
            Err(Status::INTERNAL)
        }
    }

    /// Returns the message data for the record.
    pub fn data(&self) -> &BStr {
        BStr::new(&self.data[..self.datalen as usize])
    }
}

/// The severity a kernel log message can have.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DebugLogSeverity {
    /// Record was written without a known severity.
    Unknown,
    /// Trace records include detailed information about program execution.
    Trace,
    /// Debug records include development-facing information about program execution.
    Debug,
    /// Info records include general information about program execution. (default)
    Info,
    /// Warning records include information about potentially problematic operations.
    Warn,
    /// Error records include information about failed operations.
    Error,
    /// Fatal records convey information about operations which cause a program's termination.
    Fatal,
}

impl DebugLogSeverity {
    fn from_raw(raw: u8) -> Self {
        match raw {
            sys::DEBUGLOG_TRACE => Self::Trace,
            sys::DEBUGLOG_DEBUG => Self::Debug,
            sys::DEBUGLOG_INFO => Self::Info,
            sys::DEBUGLOG_WARNING => Self::Warn,
            sys::DEBUGLOG_ERROR => Self::Error,
            sys::DEBUGLOG_FATAL => Self::Fatal,
            _ => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Instant, Signals, cprng_draw};
    use fidl_fuchsia_kernel as fkernel;
    use fuchsia_component::client::connect_channel_to_protocol;

    // expect_message_in_debuglog will read the last 10000 messages in zircon's debuglog, looking
    // for a message that equals `sent_msg`. If found, the function returns. If the first 10,000
    // messages doesn't contain `sent_msg`, it will panic.
    fn expect_message_in_debuglog(sent_msg: String) {
        use zx::Channel;
        let (client_end, server_end) = Channel::create();
        connect_channel_to_protocol::<fkernel::DebuglogResourceMarker>(server_end).unwrap();
        let service = fkernel::DebuglogResourceSynchronousProxy::new(client_end);
        let resource =
            service.get(zx::MonotonicInstant::INFINITE).expect("couldn't get debuglog resource");
        // This test and fuchsia-zircon are different crates, so we need
        // to use from_raw to convert between the zx handle and this test handle.
        // See https://fxbug.dev/42173139 for details.
        let resource = unsafe { Resource::from(NullableHandle::from_raw(resource.into_raw())) };
        let debuglog = DebugLog::create(&resource, DebugLogOpts::READABLE).unwrap();
        for _ in 0..10000 {
            match debuglog.read() {
                Ok(record) => {
                    if record.data() == sent_msg.as_bytes() {
                        // We found our log!
                        return;
                    }
                }
                Err(status) if status == Status::SHOULD_WAIT => {
                    debuglog
                        .wait_one(Signals::LOG_READABLE, Instant::INFINITE)
                        .expect("Failed to wait for log readable");
                    continue;
                }
                Err(status) => {
                    panic!("Unexpected error from zx_debuglog_read: {}", status);
                }
            }
        }
        panic!("first 10000 log messages didn't include the one we sent!");
    }

    #[test]
    fn read_from_nonreadable() {
        let resource = Resource::from(NullableHandle::invalid());
        let debuglog = DebugLog::create(&resource, DebugLogOpts::empty()).unwrap();
        assert!(debuglog.read().err() == Some(Status::ACCESS_DENIED));
    }

    #[test]
    fn write_and_read_back() {
        let mut bytes = [0; 8];
        cprng_draw(&mut bytes);
        let message = format!("log message {:?}", bytes);

        let resource = Resource::from(NullableHandle::invalid());
        let debuglog = DebugLog::create(&resource, DebugLogOpts::empty()).unwrap();
        debuglog.write(message.as_bytes()).unwrap();
        expect_message_in_debuglog(message);
    }
}
