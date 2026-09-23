// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::object_handle::{WriteBytes, WriteObjectHandle};
use anyhow::Error;
use storage_device::buffer::Buffer;
use storage_units::BlockSize;

const BUFFER_SIZE: usize = 131_072;

pub struct Writer<'a, H> {
    handle: &'a H,
    buffer: Buffer<'a>,
    offset: u64,
}

impl<'a, H: WriteObjectHandle> Writer<'a, H> {
    pub async fn new(handle: &'a H) -> Self {
        Self { handle, buffer: handle.allocate_buffer(BUFFER_SIZE).await, offset: 0 }
    }
}

impl<H: WriteObjectHandle> WriteBytes for Writer<'_, H> {
    fn block_size(&self) -> BlockSize {
        self.handle.block_size()
    }

    async fn write_bytes(&mut self, mut buf: &[u8]) -> Result<(), Error> {
        while buf.len() > 0 {
            let to_do = std::cmp::min(buf.len(), BUFFER_SIZE);
            self.buffer.subslice_mut(..to_do).copy_from_slice(&buf[..to_do]);
            self.handle.write_or_append(Some(self.offset), self.buffer.subslice(..to_do)).await?;
            self.offset += to_do as u64;
            buf = &buf[to_do..];
        }
        Ok(())
    }

    async fn complete(self) -> Result<u64, Error> {
        self.handle.flush().await?;
        Ok(self.offset)
    }

    async fn skip(&mut self, amount: u64) -> Result<(), Error> {
        let mut left = amount as usize;
        self.buffer.subslice_mut(..std::cmp::min(left, BUFFER_SIZE)).fill(0);
        while left > 0 {
            let to_do = std::cmp::min(left, BUFFER_SIZE);
            self.handle.write_or_append(Some(self.offset), self.buffer.subslice(..to_do)).await?;
            self.offset += to_do as u64;
            left -= to_do;
        }
        Ok(())
    }
}
