// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::VMEX_RESOURCE;
use crate::mm::memory::MemoryObject;
use starnix_logging::{CATEGORY_STARNIX_MM, impossible_error};
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use starnix_uapi::user_address::UserAddress;
use std::mem::MaybeUninit;
use std::sync::Arc;
use zx;

pub struct PrivateAnonymousMemoryManager {
    /// Memory object backing private, anonymous memory allocations in this address space.
    pub backing: Arc<MemoryObject>,
}

impl PrivateAnonymousMemoryManager {
    pub fn new(backing_size: u64) -> Self {
        let backing = Arc::new(
            MemoryObject::from(
                zx::Vmo::create(backing_size)
                    .unwrap()
                    .replace_as_executable(&VMEX_RESOURCE)
                    .unwrap(),
            )
            .with_zx_name(b"starnix:memory_manager"),
        );
        Self { backing }
    }

    pub fn read_memory<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        self.backing.read_uninit(bytes, addr.ptr() as u64).map_err(|_| errno!(EFAULT))
    }

    pub fn write_memory(&self, addr: UserAddress, bytes: &[u8]) -> Result<(), Errno> {
        self.backing.write(bytes, addr.ptr() as u64).map_err(|_| errno!(EFAULT))
    }

    pub fn zero(&self, addr: UserAddress, length: usize) -> Result<usize, Errno> {
        self.backing
            .op_range(zx::VmoOp::ZERO, addr.ptr() as u64, length as u64)
            .map_err(|_| errno!(EFAULT))?;
        Ok(length)
    }

    pub fn move_pages(
        &self,
        source: &std::ops::Range<UserAddress>,
        dest: UserAddress,
    ) -> Result<(), Errno> {
        let length = source.end - source.start;
        let dest_memory_offset = dest.ptr() as u64;
        let source_memory_offset = source.start.ptr() as u64;
        self.backing
            .memmove(
                zx::TransferDataOptions::empty(),
                dest_memory_offset,
                source_memory_offset,
                length.try_into().unwrap(),
            )
            .map_err(impossible_error)?;
        Ok(())
    }

    pub fn snapshot(&self, backing_size: u64) -> Result<Self, Errno> {
        fuchsia_trace::duration!(CATEGORY_STARNIX_MM, "private_anonymous_snapshot");
        Ok(Self {
            backing: Arc::new(
                self.backing
                    .create_child(zx::VmoChildOptions::SNAPSHOT, 0, backing_size)
                    .map_err(impossible_error)?
                    .replace_as_executable(&VMEX_RESOURCE)
                    .map_err(impossible_error)?,
            ),
        })
    }
}
