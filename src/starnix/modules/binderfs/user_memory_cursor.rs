// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::mm::{MemoryAccessor, MemoryAccessorExt};
use starnix_core::vfs::buffers::{InputBuffer, VecInputBuffer};
use starnix_types::user_buffer::UserBuffer;
use starnix_uapi::errors::Errno;
use starnix_uapi::user_address::UserAddress;
use zerocopy::FromBytes;

/// Allows for sequential reading of a task's userspace memory.
pub struct UserMemoryCursor {
    buffer: VecInputBuffer,
}

impl UserMemoryCursor {
    /// Create a new [`UserMemoryCursor`] starting at userspace address `addr` of length `len`.
    /// Upon creation, the cursor reads the entire user buffer then caches it.
    /// Any reads past `addr + len` will fail with `EINVAL`.
    pub fn new(ma: &dyn MemoryAccessor, addr: UserAddress, len: u64) -> Result<Self, Errno> {
        let buffer = ma.read_buffer(&UserBuffer { address: addr, length: len as usize })?;
        Ok(Self { buffer: buffer.into() })
    }

    /// Increment the read position.
    pub fn advance(&mut self, length: u64) -> Result<(), Errno> {
        self.buffer.advance(length as usize)
    }

    /// Read an object from userspace memory and increment the read position.
    pub fn read_object<T: FromBytes>(&mut self) -> Result<T, Errno> {
        self.buffer.read_object::<T>()
    }

    /// The total number of bytes read.
    pub fn bytes_read(&self) -> usize {
        self.buffer.bytes_read()
    }
}
