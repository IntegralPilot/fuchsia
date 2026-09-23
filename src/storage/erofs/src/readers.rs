// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ops::Range;
use std::sync::Arc;
use thiserror::Error;
use zerocopy::{FromBytes, IntoBytes};

/// Errors that can occur during reading from a reader.
#[derive(Error, Debug, Clone, PartialEq)]
pub enum ReaderError {
    #[error("Failed to set up reader with status: {}", _0)]
    Setup(zx::Status),
    #[error("Read error at 0x{:X}..0x{:X} with status: {}", _0.start, _0.end, _1)]
    Read(Range<u64>, zx::Status),
    #[error("Out of bound read 0x{:X}..0x{:X} when size is 0x{:X}", _0.start, _0.end, _1)]
    OutOfBounds(Range<u64>, u64),
}

/// A reader for reading data from a source.
pub trait Reader: Send + Sync {
    /// Reads `data.len()` bytes from the reader at the given `offset`.
    fn read(&self, offset: u64, data: &mut [u8]) -> Result<(), ReaderError>;
}

pub trait ReaderExt {
    fn read_object<T>(&self, offset: u64) -> Result<T, ReaderError>
    where
        T: FromBytes + IntoBytes + Sized;
}

impl ReaderExt for dyn Reader + '_ {
    fn read_object<T>(&self, offset: u64) -> Result<T, ReaderError>
    where
        T: FromBytes + IntoBytes + Sized,
    {
        let mut object = T::new_zeroed();
        self.read(offset, object.as_mut_bytes())?;
        Ok(object)
    }
}

impl Reader for Box<dyn Reader> {
    fn read(&self, offset: u64, data: &mut [u8]) -> Result<(), ReaderError> {
        self.as_ref().read(offset, data)
    }
}

impl Reader for Arc<dyn Reader> {
    fn read(&self, offset: u64, data: &mut [u8]) -> Result<(), ReaderError> {
        self.as_ref().read(offset, data)
    }
}

/// A reader that reads from a vector of bytes.
pub struct VecReader {
    data: Vec<u8>,
}

impl Reader for VecReader {
    fn read(&self, offset: u64, data: &mut [u8]) -> Result<(), ReaderError> {
        let data_len = data.len() as u64;
        let self_data_len = self.data.len() as u64;
        let offset_max = offset + data_len;
        if offset_max > self_data_len {
            return Err(ReaderError::OutOfBounds(offset..offset_max, self_data_len));
        }

        let offset_for_range: usize = offset.try_into().unwrap();
        // UNWRAP SAFETY: we already checked the bounds.
        let slice = self.data.get(offset_for_range..offset_for_range + data.len()).unwrap();
        data.clone_from_slice(slice);
        Ok(())
    }
}

impl VecReader {
    /// Creates a new reader from a vector of bytes.
    pub fn new(filesystem: Vec<u8>) -> Self {
        VecReader { data: filesystem }
    }
}

/// A reader that reads from a VMO.
pub struct VmoReader {
    vmo: Arc<zx::Vmo>,
    size: u64,
}

impl Reader for VmoReader {
    fn read(&self, offset: u64, data: &mut [u8]) -> Result<(), ReaderError> {
        match self.vmo.read(data, offset) {
            Ok(_) => Ok(()),
            Err(zx::Status::OUT_OF_RANGE) => {
                Err(ReaderError::OutOfBounds(offset..offset + data.len() as u64, self.size))
            }
            Err(status) => Err(ReaderError::Read(offset..offset + data.len() as u64, status)),
        }
    }
}

impl VmoReader {
    /// Creates a new reader from a VMO.
    pub fn new(vmo: Arc<zx::Vmo>) -> Result<Self, ReaderError> {
        let size = vmo.get_size().map_err(|status| ReaderError::Setup(status))?;
        Ok(VmoReader { vmo, size })
    }
}
