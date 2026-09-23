// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::kobject::DeviceMetadata;
use crate::device::{DeviceMode, simple_device_ops};
use crate::mm::{
    DesiredAddress, MappingName, MappingOptions, MemoryAccessorExt, ProtectionFlags,
    create_anonymous_mapping_memory,
};
use crate::task::syslog::{self, KmsgLevel};
use crate::task::{
    CurrentTask, EventHandler, Kernel, LogSubscription, Syslog, SyslogAccess, WaitCanceler, Waiter,
};
use crate::vfs::buffers::{InputBuffer, InputBufferExt as _, OutputBuffer};
use crate::vfs::{
    Anon, FileHandle, FileObject, FileOps, NamespaceNode, SeekTarget, fileops_impl_noop_sync,
    fileops_impl_seekless,
};
use starnix_logging::{Level, track_stub};
use starnix_sync::{DevKmsgLock, LockDepMutex};
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::error;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::UserAddress;
use starnix_uapi::vfs::FdEvents;
use std::mem::MaybeUninit;

#[derive(Default)]
pub struct DevNull;

pub fn new_null_file(current_task: &CurrentTask, flags: OpenFlags) -> FileHandle {
    Anon::new_private_file(current_task, Box::new(DevNull), flags, "[fuchsia:null]")
}

impl FileOps for DevNull {
    fileops_impl_seekless!();
    fileops_impl_noop_sync!();

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        // TODO(https://fxbug.dev/453758455) align /dev/null behavior with Linux
        // Writes to /dev/null on Linux treat the input buffer in an unconventional way. The actual
        // data is not touched and if the input parameters are plausible the device claims to
        // successfully write up to MAX_RW_COUNT bytes.  If the input parameters are outside of the
        // user accessible address space, writes will return EFAULT.
        let bytes_logged = match data.read_to_vec_limited(data.available()) {
            Ok(bytes) => bytes.len(),
            Err(_) => 0,
        };

        Ok(bytes_logged + data.drain())
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        Ok(0)
    }

    fn to_handle(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<Option<zx::NullableHandle>, Errno> {
        Ok(None)
    }
}

#[derive(Default)]
struct DevZero;
impl FileOps for DevZero {
    fileops_impl_seekless!();
    fileops_impl_noop_sync!();

    fn mmap(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        addr: DesiredAddress,
        memory_offset: u64,
        length: usize,
        prot_flags: ProtectionFlags,
        mut options: MappingOptions,
    ) -> Result<UserAddress, Errno> {
        // All /dev/zero mappings behave as anonymous mappings.
        //
        // This means that we always create a new zero-filled VMO for this mmap request.
        // Memory is never shared between two mappings of /dev/zero, even if
        // `MappingOptions::SHARED` is set.
        //
        // Similar to anonymous mappings, if this process were to request a shared mapping
        // of /dev/zero and then fork, the child and the parent process would share the
        // VMO created here.
        let memory = create_anonymous_mapping_memory(length as u64)?;

        options |= MappingOptions::ANONYMOUS;

        current_task.mm()?.map_memory(
            addr,
            memory,
            memory_offset,
            length,
            prot_flags,
            options,
            // We set the filename here, even though we are creating what is
            // functionally equivalent to an anonymous mapping. Doing so affects
            // the output of `/proc/self/maps` and identifies this mapping as
            // file-based.
            MappingName::File(file.to_mapping(None)?),
        )
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        Ok(data.drain())
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        data.zero()
    }
}

#[derive(Default)]
struct DevFull;
impl FileOps for DevFull {
    fileops_impl_seekless!();
    fileops_impl_noop_sync!();

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        error!(ENOSPC)
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        data.write_each(&mut |bytes| {
            bytes.fill(MaybeUninit::new(0));
            Ok(bytes.len())
        })
    }
}

struct ZeroOnDropBuffer {
    buf: Vec<u8>,
}

impl ZeroOnDropBuffer {
    fn new(size: usize) -> Self {
        Self { buf: vec![0u8; size] }
    }
}

impl Drop for ZeroOnDropBuffer {
    fn drop(&mut self) {
        // SAFETY: The pointers constructed here are bounded by the lifetime of self.buf.
        unsafe {
            for i in 0..self.buf.len() {
                std::ptr::write_volatile(self.buf.as_mut_ptr().add(i), 0);
            }
        }
    }
}

#[derive(Default)]
pub struct DevRandom;
impl FileOps for DevRandom {
    fileops_impl_seekless!();
    fileops_impl_noop_sync!();

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        Ok(data.drain())
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        // Zero out the memory where we stored the random values as they may be used to seed keys
        // or other cryptographic data structures and we do not want to accidentally leak the data.
        let mut rdm = ZeroOnDropBuffer::new(data.available());
        starnix_crypto::cprng_draw(&mut rdm.buf);
        data.write(&rdm.buf)
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: starnix_syscalls::SyscallArg,
    ) -> Result<starnix_syscalls::SyscallResult, Errno> {
        match request {
            starnix_uapi::RNDGETENTCNT => {
                let addr = starnix_uapi::user_address::UserRef::<i32>::new(UserAddress::from(arg));
                // Linux just returns 256 no matter what (as observed on 6.5.6).
                let result = 256;
                current_task.write_object(addr, &result).map(|_| starnix_syscalls::SUCCESS)
            }
            _ => error!(ENOTTY),
        }
    }
}

pub fn open_kmsg(
    current_task: &CurrentTask,
    _id: DeviceId,
    _node: &NamespaceNode,
    flags: OpenFlags,
) -> Result<Box<dyn FileOps>, Errno> {
    if flags.can_read() {
        Syslog::validate_access(current_task, SyslogAccess::DevKmsgRead)?;
    }
    let subscription = if flags.can_read() {
        Some(Syslog::snapshot_then_subscribe(current_task.kernel())?.into())
    } else {
        None
    };
    Ok(Box::new(DevKmsg(subscription)))
}

struct DevKmsg(Option<LockDepMutex<LogSubscription, DevKmsgLock>>);

impl FileOps for DevKmsg {
    fileops_impl_noop_sync!();

    fn has_persistent_offsets(&self) -> bool {
        false
    }

    fn is_seekable(&self) -> bool {
        true
    }

    fn seek(
        &self,
        _file: &crate::vfs::FileObject,
        current_task: &crate::task::CurrentTask,
        _current_offset: starnix_uapi::off_t,
        target: crate::vfs::SeekTarget,
    ) -> Result<starnix_uapi::off_t, starnix_uapi::errors::Errno> {
        match target {
            SeekTarget::Set(0) => {
                let Some(ref subscription) = self.0 else {
                    return Ok(0);
                };
                let mut guard = subscription.lock();
                *guard = Syslog::snapshot_then_subscribe(current_task.kernel())?;
                Ok(0)
            }
            SeekTarget::End(0) => {
                let Some(ref subscription) = self.0 else {
                    return Ok(0);
                };
                let mut guard = subscription.lock();
                *guard = Syslog::subscribe(current_task.kernel())?;
                Ok(0)
            }
            SeekTarget::Data(0) => {
                track_stub!(TODO("https://fxbug.dev/322874315"), "/dev/kmsg: SEEK_DATA");
                Ok(0)
            }
            // The following are implemented as documented on:
            // https://www.kernel.org/doc/Documentation/ABI/testing/dev-kmsg
            // The only accepted seek targets are "SEEK_END,0", "SEEK_SET,0" and "SEEK_DATA,0"
            // When given an invalid offset, ESPIPE is expected.
            SeekTarget::End(_) | SeekTarget::Set(_) | SeekTarget::Data(_) => {
                error!(ESPIPE, "Unsupported offset")
            }
            // According to the docs above and observations, this should be EINVAL, but dprintf
            // fails if we make it EINVAL.
            SeekTarget::Cur(_) => error!(ESPIPE),
            SeekTarget::Hole(_) => error!(EINVAL, "Unsupported seek target"),
        }
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        self.0.as_ref().map(|subscription| subscription.lock().wait(waiter, events, handler))
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        let mut events = FdEvents::empty();
        if let Some(subscription) = self.0.as_ref() {
            if subscription.lock().available()? > 0 {
                events |= FdEvents::POLLIN;
            }
        }
        Ok(events)
    }

    fn read(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        file.blocking_op(current_task, FdEvents::POLLIN | FdEvents::POLLHUP, None, || {
            match self.0.as_ref().unwrap().lock().next() {
                Some(Ok(log)) => data.write(&log),
                Some(Err(err)) => Err(err),
                None => Ok(0),
            }
        })
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        let bytes = data.read_all()?;
        let extract_result = syslog::extract_level(&bytes);
        let (level, msg_bytes) = match extract_result {
            None => (Level::Info, bytes.as_slice()),
            Some((level, bytes_after_level)) => match level {
                // An error but keep the <level> str.
                KmsgLevel::Emergency | KmsgLevel::Alert | KmsgLevel::Critical => {
                    (Level::Error, bytes.as_slice())
                }
                KmsgLevel::Error => (Level::Error, bytes_after_level),
                KmsgLevel::Warning => (Level::Warn, bytes_after_level),
                // Log as info but show the <level>.
                KmsgLevel::Notice => (Level::Info, bytes.as_slice()),
                KmsgLevel::Info => (Level::Info, bytes_after_level),
                KmsgLevel::Debug => (Level::Debug, bytes_after_level),
            },
        };

        // We need to create and emit our own log record here, because the log macros will include
        // a file and line by default if the log message is ERROR level. This file/line is not
        // relevant to log messages forwarded from userspace, and the kmsg tag is hopefully enough
        // to distinguish messages forwarded this way.
        starnix_logging::with_current_task_info(|info| {
            starnix_logging::logger().log(
                // The log::RecordBuilder API only allows providing the body of a log message as
                // format_args!(), which cannot be assigned to bindings if it captures values
                // (https://doc.rust-lang.org/std/macro.format_args.html#lifetime-limitation).
                // So this creates the record in the same expression where it is used.
                &starnix_logging::Record::builder()
                    .level(level)
                    .key_values(&[
                        ("tag", LogOutputTag::Str("kmsg")),
                        ("tag", LogOutputTag::Display(info)),
                    ])
                    .args(format_args!(
                        "{}",
                        String::from_utf8_lossy(msg_bytes).trim_end_matches('\n')
                    ))
                    .build(),
            );
        });
        Ok(bytes.len())
    }
}

enum LogOutputTag<'a> {
    Str(&'a str),
    Display(&'a dyn std::fmt::Display),
}

impl<'a> starnix_logging::ToValue for LogOutputTag<'a> {
    fn to_value(&self) -> starnix_logging::Value<'_> {
        match self {
            Self::Str(s) => starnix_logging::Value::from_display(s),
            Self::Display(d) => starnix_logging::Value::from_dyn_display(d),
        }
    }
}

pub fn mem_device_init<'a>(kernel: &Kernel) -> Result<(), Errno> {
    let registry = &kernel.device_registry;

    let mem_class = registry.objects.mem_class();
    registry.register_device(
        kernel,
        "null".into(),
        DeviceMetadata::new("null".into(), DeviceId::NULL, DeviceMode::Char),
        mem_class.clone(),
        simple_device_ops::<DevNull>,
    )?;
    registry.register_device(
        kernel,
        "zero".into(),
        DeviceMetadata::new("zero".into(), DeviceId::ZERO, DeviceMode::Char),
        mem_class.clone(),
        simple_device_ops::<DevZero>,
    )?;
    registry.register_device(
        kernel,
        "full".into(),
        DeviceMetadata::new("full".into(), DeviceId::FULL, DeviceMode::Char),
        mem_class.clone(),
        simple_device_ops::<DevFull>,
    )?;
    registry.register_device(
        kernel,
        "random".into(),
        DeviceMetadata::new("random".into(), DeviceId::RANDOM, DeviceMode::Char),
        mem_class.clone(),
        simple_device_ops::<DevRandom>,
    )?;
    registry.register_device(
        kernel,
        "urandom".into(),
        DeviceMetadata::new("urandom".into(), DeviceId::URANDOM, DeviceMode::Char),
        mem_class.clone(),
        simple_device_ops::<DevRandom>,
    )?;
    registry.register_device(
        kernel,
        "kmsg".into(),
        DeviceMetadata::new("kmsg".into(), DeviceId::KMSG, DeviceMode::Char),
        mem_class,
        open_kmsg,
    )?;
    Ok(())
}
