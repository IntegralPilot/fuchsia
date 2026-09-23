// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::mem;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use crate::mm::MemoryAccessor;
use crate::task::CurrentTask;
use crate::vfs::FsStr;
use starnix_uapi::errors::{ENOSPC, Errno};
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::math::round_up_to_increment;
use starnix_uapi::user_address::UserAddress;
use starnix_uapi::{error, ino_t, off_t};

#[derive(Debug, Default, Copy, Clone, PartialOrd, Ord, PartialEq, Eq)]
pub struct DirectoryEntryType(u8);

// These values are defined in libc.
impl DirectoryEntryType {
    pub const UNKNOWN: DirectoryEntryType = DirectoryEntryType(0);
    pub const FIFO: DirectoryEntryType = DirectoryEntryType(1);
    pub const CHR: DirectoryEntryType = DirectoryEntryType(2);
    pub const DIR: DirectoryEntryType = DirectoryEntryType(4);
    pub const BLK: DirectoryEntryType = DirectoryEntryType(6);
    pub const REG: DirectoryEntryType = DirectoryEntryType(8);
    pub const LNK: DirectoryEntryType = DirectoryEntryType(10);
    pub const SOCK: DirectoryEntryType = DirectoryEntryType(12);

    pub fn from_bits(bits: u8) -> DirectoryEntryType {
        Self(bits)
    }

    pub fn from_mode(mode: FileMode) -> DirectoryEntryType {
        match mode.fmt() {
            FileMode::IFLNK => DirectoryEntryType::LNK,
            FileMode::IFREG => DirectoryEntryType::REG,
            FileMode::IFDIR => DirectoryEntryType::DIR,
            FileMode::IFCHR => DirectoryEntryType::CHR,
            FileMode::IFBLK => DirectoryEntryType::BLK,
            FileMode::IFIFO => DirectoryEntryType::FIFO,
            FileMode::IFSOCK => DirectoryEntryType::SOCK,
            _ => DirectoryEntryType::UNKNOWN,
        }
    }

    pub fn bits(&self) -> u8 {
        self.0
    }
}

const DIRENT64_PADDING_SIZE: usize = 5;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone, IntoBytes, KnownLayout, FromBytes, Immutable)]
struct DirentHeader64 {
    d_ino: u64,
    d_off: i64,
    d_reclen: u16,
    d_type: u8,
    padding: [u8; DIRENT64_PADDING_SIZE],
}

const DIRENT64_HEADER_SIZE: usize = mem::size_of::<DirentHeader64>() - DIRENT64_PADDING_SIZE;

pub trait DirentSink {
    /// Add the given directory entry to this buffer.
    ///
    /// In case of success, this will update the offset from the FileObject. Any other bookkeeping
    /// must be done by the caller after this method returns successfully.
    ///
    /// Returns error!(ENOSPC) if the entry does not fit.
    fn add(
        &mut self,
        inode_num: ino_t,
        offset: off_t,
        entry_type: DirectoryEntryType,
        name: &FsStr,
    ) -> Result<(), Errno>;

    /// The current offset to return.
    fn offset(&self) -> off_t;

    /// Optional information about the max number of bytes to send to the user.
    fn user_capacity(&self) -> Option<usize> {
        None
    }
}

struct BaseDirentSink<'a> {
    current_task: &'a CurrentTask,
    offset: &'a mut off_t,
    user_buffer: UserAddress,
    user_capacity: usize,
    actual: usize,
}

impl<'a> BaseDirentSink<'a> {
    fn add(&mut self, offset: off_t, buffer: &[u8]) -> Result<(), Errno> {
        if self.actual + buffer.len() > self.user_capacity {
            return error!(ENOSPC);
        }
        self.current_task.write_memory((self.user_buffer + self.actual)?, buffer)?;
        self.actual += buffer.len();
        *self.offset = offset;
        Ok(())
    }

    fn offset(&self) -> off_t {
        *self.offset
    }

    // Converts result value received from `add()` to `getdents()` result.
    //
    // `add()` may return `ENOSPC` if there was not enough space in the buffer for the new entry.
    // This error should be returned to the caller only if we didn't have space for the first
    // directory entry.
    //
    // We use `ENOSPC` rather than `EINVAL` to signal this condition because `EINVAL` is a very
    // generic error. We only want to perform this transformation in exactly the case where there
    // was not sufficient space in the DirentSink.
    fn map_result_with_actual(&self, result: Result<(), Errno>) -> Result<usize, Errno> {
        match result {
            Ok(()) => Ok(self.actual),
            Err(_) if self.actual > 0 => Ok(self.actual),
            Err(errno) if errno == ENOSPC => error!(EINVAL),
            Err(e) => Err(e),
        }
    }
}

pub struct DirentSink64<'a> {
    base: BaseDirentSink<'a>,
}

impl<'a> DirentSink64<'a> {
    pub fn new(
        current_task: &'a CurrentTask,
        offset: &'a mut off_t,
        user_buffer: UserAddress,
        user_capacity: usize,
    ) -> Self {
        Self {
            base: BaseDirentSink { current_task, offset, user_buffer, user_capacity, actual: 0 },
        }
    }

    pub fn map_result_with_actual(&self, result: Result<(), Errno>) -> Result<usize, Errno> {
        self.base.map_result_with_actual(result)
    }
}

impl DirentSink for DirentSink64<'_> {
    fn add(
        &mut self,
        inode_num: ino_t,
        offset: off_t,
        entry_type: DirectoryEntryType,
        name: &FsStr,
    ) -> Result<(), Errno> {
        let content_size = DIRENT64_HEADER_SIZE + name.len();
        let entry_size = round_up_to_increment(content_size + 1, 8)?; // +1 for the null terminator.
        let mut buffer = Vec::with_capacity(entry_size);
        let header = DirentHeader64 {
            d_ino: inode_num,
            d_off: offset,
            d_reclen: entry_size as u16,
            d_type: entry_type.bits(),
            ..DirentHeader64::default()
        };
        let header_bytes = header.as_bytes();
        buffer.extend_from_slice(&header_bytes[..DIRENT64_HEADER_SIZE]);
        buffer.extend_from_slice(name);
        buffer.resize(buffer.len() + (entry_size - content_size), b'\0');
        assert_eq!(buffer.len(), entry_size);
        self.base.add(offset, &buffer)
    }

    fn offset(&self) -> off_t {
        self.base.offset()
    }

    fn user_capacity(&self) -> Option<usize> {
        Some(self.base.user_capacity)
    }
}

#[cfg(target_arch = "x86_64")]
pub use x86_64::*;

#[cfg(target_arch = "x86_64")]
mod x86_64 {
    use super::{BaseDirentSink, DirectoryEntryType, DirentSink};
    use crate::task::CurrentTask;
    use crate::vfs::FsStr;
    use starnix_uapi::errors::Errno;
    use starnix_uapi::math::round_up_to_increment;
    use starnix_uapi::user_address::UserAddress;
    use starnix_uapi::{ino_t, off_t};
    use std::mem;
    use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

    const DIRENT32_PADDING_SIZE: usize = 6;
    const DIRENT32_HEADER_SIZE: usize = mem::size_of::<DirentHeader32>() - DIRENT32_PADDING_SIZE;

    #[repr(C)]
    #[derive(Debug, Default, Copy, Clone, IntoBytes, KnownLayout, FromBytes, Immutable)]
    struct DirentHeader32 {
        d_ino: u64,
        d_off: i64,
        d_reclen: u16,
        padding: [u8; DIRENT32_PADDING_SIZE],
        // pad: u8, // Zero padding byte
        // d_type: u8, // File type
    }

    pub struct DirentSink32<'a> {
        base: BaseDirentSink<'a>,
    }

    impl<'a> DirentSink32<'a> {
        pub fn new(
            current_task: &'a CurrentTask,
            offset: &'a mut off_t,
            user_buffer: UserAddress,
            user_capacity: usize,
        ) -> Self {
            Self {
                base: BaseDirentSink {
                    current_task,
                    offset,
                    user_buffer,
                    user_capacity,
                    actual: 0,
                },
            }
        }

        pub fn map_result_with_actual(&self, result: Result<(), Errno>) -> Result<usize, Errno> {
            self.base.map_result_with_actual(result)
        }
    }

    impl DirentSink for DirentSink32<'_> {
        fn add(
            &mut self,
            inode_num: ino_t,
            offset: off_t,
            entry_type: DirectoryEntryType,
            name: &FsStr,
        ) -> Result<(), Errno> {
            let content_size = DIRENT32_HEADER_SIZE + name.len();
            // +1 for the null terminator, +1 for the type.
            let entry_size = round_up_to_increment(content_size + 2, 8)?;
            let mut buffer = Vec::with_capacity(entry_size);
            let header = DirentHeader32 {
                d_ino: inode_num,
                d_off: offset,
                d_reclen: entry_size as u16,
                ..DirentHeader32::default()
            };
            let header_bytes = header.as_bytes();
            buffer.extend_from_slice(&header_bytes[..DIRENT32_HEADER_SIZE]);
            buffer.extend_from_slice(name);
            buffer.resize(buffer.len() + (entry_size - content_size - 1), b'\0');
            buffer.push(entry_type.bits()); // Include the type.
            assert_eq!(buffer.len(), entry_size);
            self.base.add(offset, &buffer)
        }

        fn offset(&self) -> off_t {
            self.base.offset()
        }

        fn user_capacity(&self) -> Option<usize> {
            Some(self.base.user_capacity)
        }
    }
}
