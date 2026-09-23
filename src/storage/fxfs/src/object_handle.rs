// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::object_store::{DirType, PosixAttributes, Timestamp};
use anyhow::Error;
use async_trait::async_trait;
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use storage_device::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use storage_units::BlockSize;

// Some places use Default and assume that zero is an invalid object ID, so this cannot be changed
// easily.
pub const INVALID_OBJECT_ID: u64 = 0;

/// A handle for a generic object.  For objects with a data payload, use the ReadObjectHandle or
/// WriteObjectHandle traits.
pub trait ObjectHandle: Send + Sync + 'static {
    /// Returns the object identifier for this object which will be unique for the store that the
    /// object is contained in, but not necessarily unique within the entire system.
    fn object_id(&self) -> u64;

    /// Returns the filesystem block size, which should be at least as big as the device block size,
    /// but not necessarily the same.
    fn block_size(&self) -> BlockSize;

    /// Allocates a buffer for doing I/O (read and write) for the object.
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_>;

    /// Sets tracing for this object.
    fn set_trace(&self, _v: bool) {}
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectProperties {
    /// The number of references to this object.
    pub refs: u64,
    /// The number of bytes allocated to all extents across all attributes for this object.
    pub allocated_size: u64,
    /// The logical content size for the default data attribute of this object, i.e. the size of a
    /// file.  (Objects with no data attribute have size 0.)
    pub data_attribute_size: u64,
    /// The timestamp at which the object was created (i.e. crtime).
    pub creation_time: Timestamp,
    /// The timestamp at which the objects's data was last modified (i.e. mtime).
    pub modification_time: Timestamp,
    /// The timestamp at which the object was last read (i.e. atime).
    pub access_time: Timestamp,
    /// The timestamp at which the object's status was last modified (i.e. ctime).
    pub change_time: Timestamp,
    /// The number of sub-directories.
    pub sub_dirs: u64,
    /// POSIX attributes: mode, uid, gid, rdev
    pub posix_attributes: Option<PosixAttributes>,
    /// The type of directory (encryption, casefolding, etc.)
    pub dir_type: DirType,
}

#[async_trait]
pub trait ReadObjectHandle: ObjectHandle {
    /// Fills |buf| with up to |buf.len()| bytes read from |offset| on the underlying device.
    /// |offset| and |buf| must both be block-aligned.
    async fn read(&self, offset: u64, buf: MutableBufferRef<'_>) -> Result<usize, Error>;

    /// Returns the size of the object.
    fn get_size(&self) -> u64;
}

pub trait WriteObjectHandle: ObjectHandle {
    /// Writes |buf.len())| bytes at |offset| (or the end of the file), returning the object size
    /// after writing.
    /// The writes may be cached, in which case a later call to |flush| is necessary to persist the
    /// writes.
    fn write_or_append(
        &self,
        offset: Option<u64>,
        buf: BufferRef<'_>,
    ) -> impl Future<Output = Result<u64, Error>> + Send;

    /// Truncates the object to |size| bytes.
    /// The truncate may be cached, in which case a later call to |flush| is necessary to persist
    /// the truncate.
    fn truncate(&self, size: u64) -> impl Future<Output = Result<(), Error>> + Send;

    /// Flushes all pending data and metadata updates for the object.
    fn flush(&self) -> impl Future<Output = Result<(), Error>> + Send;
}

/// This trait is an asynchronous streaming writer.
pub trait WriteBytes: Sized {
    fn block_size(&self) -> BlockSize;

    /// Buffers writes to be written to the underlying handle. This may flush bytes immediately
    /// or when buffers are full.
    fn write_bytes(&mut self, buf: &[u8]) -> impl Future<Output = Result<(), Error>> + Send;

    /// Called to flush to the handle. The total number of bytes written is returned.
    fn complete(self) -> impl Future<Output = Result<u64, Error>> + Send;

    /// Moves the offset forward by `amount`, which will result in zeroes in the output stream, even
    /// if no other data is appended to it.
    fn skip(&mut self, amount: u64) -> impl Future<Output = Result<(), Error>> + Send;
}

// Implements ReadObjectHandle for things like `Arc<dyn ReadObjectHandle>` and
// `Box<dyn ReadObjectHandle>`.  The below impl of `ObjectHandle` is also necessary for this.
impl<T: Deref<Target = dyn ReadObjectHandle> + Send + Sync + 'static> ReadObjectHandle for T {
    // Manual expansion of `async_trait` to avoid double boxing the `Future`.
    fn read<'a, 'b, 'c>(
        &'a self,
        offset: u64,
        buf: MutableBufferRef<'b>,
    ) -> Pin<Box<dyn Future<Output = Result<usize, Error>> + Send + 'c>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        (**self).read(offset, buf)
    }

    fn get_size(&self) -> u64 {
        (**self).get_size()
    }
}

impl<T: Deref<Target = dyn ReadObjectHandle> + Send + Sync + 'static> ObjectHandle for T {
    fn object_id(&self) -> u64 {
        (**self).object_id()
    }

    fn block_size(&self) -> BlockSize {
        (**self).block_size()
    }

    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        (**self).allocate_buffer(size)
    }

    fn set_trace(&self, v: bool) {
        (**self).set_trace(v)
    }
}
