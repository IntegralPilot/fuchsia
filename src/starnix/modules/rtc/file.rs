// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::task::CurrentTask;
use starnix_core::vfs::{
    FileObject, FileOps, InputBuffer, OutputBuffer, SeekTarget, fileops_impl_noop_sync,
};
use starnix_logging::{log_debug, track_stub};

use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_uapi::RTC_SET_TIME;
use starnix_uapi::errors::{Errno, error};

/// Implements Real Time Clock (RTC) operations.
pub struct RtcFile;

impl RtcFile {
    /// Create a new RTC device file.
    pub fn new_file(_current_task: &CurrentTask) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(Self {}))
    }
}

impl FileOps for RtcFile {
    fileops_impl_noop_sync!();

    fn is_seekable(&self) -> bool {
        false
    }

    fn seek(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _current_offset: starnix_uapi::off_t,
        _target: SeekTarget,
    ) -> Result<starnix_uapi::off_t, starnix_uapi::errors::Errno> {
        error!(ESPIPE, "seek on rtc")
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        match request {
            RTC_SET_TIME => {
                // Ignore the request, return success. Starnix updates the time through
                // settimeofday. This ioctl might be important for programs that are
                // allowed to set the RTC reported time directly. We do not currently
                // have plans to make that available.
                track_stub!(
                    TODO("https://fxbug.dev/469486947"),
                    "rtc ioctl RTC_SET_TIME incomplete implementation",
                    RTC_SET_TIME
                );
                log_debug!("RtcFile::ioctl: RTC_SET_TIME {arg:?}");
                Ok(SUCCESS)
            }
            unknown_ioctl => {
                track_stub!(TODO("https://fxbug.dev/322874368"), "rtc ioctl", unknown_ioctl);
                error!(ENOSYS, format!("rtc ioctl: {unknown_ioctl}"))
            }
        }
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        // because fileops_impl_nonseekable!()
        debug_assert!(offset == 0);
        error!(EINVAL, "read on rtc")
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        // because fileops_impl_nonseekable!()
        debug_assert!(offset == 0);
        error!(EINVAL, "write on rtc")
    }
}
