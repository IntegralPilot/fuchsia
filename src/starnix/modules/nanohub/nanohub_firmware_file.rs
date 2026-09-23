// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_hardware_google_nanohub::DeviceMarker;
use fuchsia_component as fcomponent;
use starnix_core::mm::ProtectionFlags;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::buffers::{InputBuffer, OutputBuffer};
use starnix_core::vfs::{
    FdNumber, FileObject, FileOps, fileops_impl_nonseekable, fileops_impl_noop_sync,
};
use starnix_logging::{impossible_error, log_error};

use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::vfs::ResolveFlags;
use starnix_uapi::{errno, error};

pub struct NanohubFirmwareFile {}

const FIRMWARE_DIRECTORIES: &[&str] =
    &["/vendor/etc/firmware/", "/vendor/firmware/", "/data/vendor/mcu_firmware/"];

/// Split a firmware file name and offset integer.
///
/// **NOTE**: Filenames that contain spaces are not supported.
///
/// Examples:
///
/// ```
/// assert_eq!(
///     split_filename_and_offset(&String::from("some_file 1234")),
///     Ok(("some_file", 1234)),
/// );
/// assert_eq!(
///     split_filename_and_offset(&String::from("some file with spaces  5678")),
///     Err(errno!(EINVAL)),
/// );
/// ```
pub fn split_filename_and_offset(input_string: &String) -> Result<(&str, u64), Errno> {
    let content = input_string.trim();
    // Split on the first space.
    match content.find(' ') {
        Some(index) => {
            let (file_name, offset_string) = content.split_at(index);
            let offset: u64 = offset_string.trim().parse::<u64>().map_err(|_| errno!(EINVAL))?;
            Ok((file_name, offset))
        }
        None => Ok((content, 0)),
    }
}

impl FileOps for NanohubFirmwareFile {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        error!(EINVAL)
    }

    fn write(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        debug_assert!(_offset == 0);

        let input_bytes = data.read_all()?;
        let input_string = String::from_utf8(input_bytes).map_err(|_| errno!(ENOENT))?;

        let (file_name, offset) = split_filename_and_offset(&input_string)?;

        for prefix_path in FIRMWARE_DIRECTORIES {
            let firmware_full_path = prefix_path.to_string() + file_name;
            let firmware_open_result = current_task.open_file_at(
                FdNumber::AT_FDCWD,
                firmware_full_path.as_bytes().into(),
                OpenFlags::RDONLY,
                FileMode::default(),
                ResolveFlags::empty(),
            );
            if firmware_open_result.is_err() {
                continue;
            }

            let memory = firmware_open_result.unwrap().get_memory(
                current_task,
                None,
                ProtectionFlags::READ | ProtectionFlags::EXEC,
            )?;

            let vmo = memory
                .as_vmo()
                .ok_or_else(|| errno!(EINVAL))?
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .map_err(impossible_error)?;

            let device_proxy = fcomponent::client::connect_to_protocol_sync::<DeviceMarker>()
                .map_err(|_| errno!(ENOENT))?;

            device_proxy
                .download_firmware(vmo, offset, zx::MonotonicInstant::INFINITE)
                .map_err(|_| errno!(ENOENT))?
                .map_err(|_| errno!(ENOENT))?;

            return Ok(data.bytes_read());
        }

        log_error!("Unable to locate a firmware file in any know directories '{:?}'", file_name);
        error!(ENOENT)
    }
}

impl NanohubFirmwareFile {
    pub fn new() -> Box<Self> {
        Box::new(NanohubFirmwareFile {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::fuchsia::test]
    async fn test_split_filename_and_offset() {
        assert_eq!(
            split_filename_and_offset(&String::from("some_file 1234")),
            Ok(("some_file", 1234)),
        );

        assert_eq!(
            split_filename_and_offset(&String::from(" some_file   5678  ")),
            Ok(("some_file", 5678))
        );

        assert_eq!(split_filename_and_offset(&String::from("some_file")), Ok(("some_file", 0)));

        assert_eq!(
            split_filename_and_offset(&String::from("some_file 1234")),
            Ok(("some_file", 1234)),
        );
        assert_eq!(
            split_filename_and_offset(&String::from("some file with spaces    5678")),
            Err(errno!(EINVAL)),
        );
    }
}
