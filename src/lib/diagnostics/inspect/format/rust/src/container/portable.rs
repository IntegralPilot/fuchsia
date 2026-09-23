// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{BlockContainer, CopyBytes, ReadBytes, WriteBytes};

#[derive(Debug)]
pub struct Container {
    buffer: Vec<u8>,
}

impl Container {
    #[allow(clippy::result_unit_err)]
    pub fn read_and_write(size: usize) -> Result<(Self, ()), ()> {
        let buffer = vec![0; size];
        Ok((Self { buffer }, ()))
    }

    pub fn read_only(buffer: &[u8]) -> Self {
        Self { buffer: buffer.to_vec() }
    }
}

impl BlockContainer for Container {
    type Data = Vec<u8>;
    type ShareableData = ();

    #[inline]
    fn len(&self) -> usize {
        self.buffer.len()
    }
}

impl ReadBytes for Container {
    #[inline]
    fn get_slice_at(&self, offset: usize, size: usize) -> Option<&[u8]> {
        self.buffer.get_slice_at(offset, size)
    }
}

impl CopyBytes for Container {
    #[inline]
    fn copy_bytes_at(&self, offset: usize, dst: &mut [u8]) {
        self.buffer.copy_bytes_at(offset, dst)
    }
}

impl WriteBytes for Container {
    #[inline]
    fn get_slice_mut_at(&mut self, offset: usize, size: usize) -> Option<&mut [u8]> {
        let end = offset.checked_add(size)?;
        self.buffer.get_mut(offset..end)
    }
}
