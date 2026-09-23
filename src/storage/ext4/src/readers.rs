// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::Mutex;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
use thiserror::Error;

#[cfg(target_os = "fuchsia")]
pub use self::fuchsia::*;

#[derive(Error, Debug, PartialEq)]
pub enum ReaderError {
    #[error("Read error at: 0x{:X}", _0)]
    Read(u64),
    #[error("Out of bound read 0x{:X} when size is 0x{:X}", _0, _1)]
    OutOfBounds(u64, u64),
    #[error("Write error at: 0x{:X}", _0)]
    Write(u64),
    #[error("{} not supported", _0)]
    NotSupported(String),
    #[error("Write flush error")]
    WriteFlush,
}

pub trait Reader: Send + Sync {
    fn read(&self, offset: u64, data: &mut [u8]) -> Result<(), ReaderError>;
}

// For simpler usage of Reader trait objects with Parser, we also implement the Reader trait
// for Arc and Box. This allows callers of Parser::new to pass trait objects or real objects
// without having to create custom wrappers or duplicate implementations.
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

// Same as Reader but has write capability.
pub trait ReaderWriter: Reader {
    fn write(&self, _offset: u64, _data: &[u8]) -> Result<(), ReaderError> {
        Err(ReaderError::NotSupported("Write".to_string()))
    }
    fn sync(&self) -> Result<(), ReaderError> {
        Ok(())
    }
}

impl Reader for Box<dyn ReaderWriter> {
    fn read(&self, offset: u64, data: &mut [u8]) -> Result<(), ReaderError> {
        self.as_ref().read(offset, data)
    }
}

impl ReaderWriter for Box<dyn ReaderWriter> {
    fn write(&self, offset: u64, data: &[u8]) -> Result<(), ReaderError> {
        self.as_ref().write(offset, data)
    }
    fn sync(&self) -> Result<(), ReaderError> {
        self.as_ref().sync()
    }
}

impl Reader for Arc<dyn ReaderWriter> {
    fn read(&self, offset: u64, data: &mut [u8]) -> Result<(), ReaderError> {
        self.as_ref().read(offset, data)
    }
}

impl ReaderWriter for Arc<dyn ReaderWriter> {
    fn write(&self, offset: u64, data: &[u8]) -> Result<(), ReaderError> {
        self.as_ref().write(offset, data)
    }
    fn sync(&self) -> Result<(), ReaderError> {
        self.as_ref().sync()
    }
}

/// IoAdapter wraps any reader that supports std::io::{Read|Seek}.
pub struct IoAdapter<T>(Mutex<T>);

impl<T> IoAdapter<T> {
    pub fn new(inner: T) -> Self {
        Self(Mutex::new(inner))
    }
}

impl<T: Read + Seek + Send + Sync> Reader for IoAdapter<T> {
    fn read(&self, offset: u64, data: &mut [u8]) -> Result<(), ReaderError> {
        let mut reader = self.0.lock();
        reader.seek(SeekFrom::Start(offset)).map_err(|_| ReaderError::Read(offset))?;
        reader.read_exact(data).map_err(|_| ReaderError::Read(offset))
    }
}

impl<T: Read + Seek + Send + Sync> ReaderWriter for IoAdapter<T> {}

pub struct VecReader {
    data: Vec<u8>,
}

impl Reader for VecReader {
    fn read(&self, offset: u64, data: &mut [u8]) -> Result<(), ReaderError> {
        let data_len = data.len() as u64;
        let self_data_len = self.data.len() as u64;
        let offset_max = offset + data_len;
        if offset_max > self_data_len {
            return Err(ReaderError::OutOfBounds(offset_max, self_data_len));
        }

        let offset_for_range: usize = offset.try_into().unwrap();

        match self.data.get(offset_for_range..offset_for_range + data.len()) {
            Some(slice) => {
                data.clone_from_slice(slice);
                Ok(())
            }
            None => Err(ReaderError::Read(offset)),
        }
    }
}

impl ReaderWriter for VecReader {
    fn write(&self, _offset: u64, _data: &[u8]) -> Result<(), ReaderError> {
        Err(ReaderError::NotSupported("Write".to_string()))
    }
}

impl VecReader {
    pub fn new(filesystem: Vec<u8>) -> Self {
        VecReader { data: filesystem }
    }
}

#[cfg(target_os = "fuchsia")]
mod fuchsia {
    use super::{Reader, ReaderError, ReaderWriter};
    use anyhow::Error;
    use block_client::{Cache, RemoteBlockClientSync};
    use fidl::endpoints::ClientEnd;
    use fidl_fuchsia_storage_block::BlockMarker;
    use fuchsia_sync::Mutex;
    use log::error;
    use std::io::Write as _;
    use std::sync::Arc;

    pub struct VmoReader {
        vmo: Arc<zx::Vmo>,
    }

    impl Reader for VmoReader {
        fn read(&self, offset: u64, data: &mut [u8]) -> Result<(), ReaderError> {
            match self.vmo.read(data, offset) {
                Ok(_) => Ok(()),
                Err(zx::Status::OUT_OF_RANGE) => {
                    let size = self.vmo.get_size().map_err(|_| ReaderError::Read(u64::MAX))?;
                    Err(ReaderError::OutOfBounds(offset, size))
                }
                Err(_) => Err(ReaderError::Read(offset)),
            }
        }
    }

    impl ReaderWriter for VmoReader {
        fn write(&self, _offset: u64, _data: &[u8]) -> Result<(), ReaderError> {
            Err(ReaderError::NotSupported("Write".to_string()))
        }
    }

    impl VmoReader {
        pub fn new(vmo: Arc<zx::Vmo>) -> Self {
            VmoReader { vmo }
        }
    }

    pub struct BlockDeviceReader {
        block_cache: Mutex<Cache>,
    }

    impl Reader for BlockDeviceReader {
        fn read(&self, offset: u64, data: &mut [u8]) -> Result<(), ReaderError> {
            self.block_cache.lock().read_at(data, offset).map_err(|e| {
                error!("Encountered error while reading block device: {}", e);
                ReaderError::Read(offset)
            })
        }
    }

    impl ReaderWriter for BlockDeviceReader {
        fn write(&self, offset: u64, data: &[u8]) -> Result<(), ReaderError> {
            self.block_cache.lock().write_at(data, offset).map_err(|e| {
                error!("Encountered error while writing block device: {}", e);
                ReaderError::Write(offset)
            })
        }

        fn sync(&self) -> Result<(), ReaderError> {
            self.block_cache.lock().flush().map_err(|e| {
                error!("Encountered error while flushing block cache: {}", e);
                ReaderError::WriteFlush
            })
        }
    }

    impl BlockDeviceReader {
        pub fn from_client_end(client_end: ClientEnd<BlockMarker>) -> Result<Self, Error> {
            Ok(Self {
                block_cache: Mutex::new(Cache::new(RemoteBlockClientSync::new(client_end)?)?),
            })
        }
    }
}
