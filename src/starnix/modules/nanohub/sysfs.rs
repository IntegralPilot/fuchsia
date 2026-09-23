// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::sysfs_errno;
use std::marker::PhantomData;

use fidl::endpoints::{DiscoverableProtocolMarker, ProtocolMarker};
use fuchsia_component::client::connect_to_protocol_sync;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::{
    AppendLockWriteGuard, FileObject, FileOps, FsNode, FsNodeOps, InputBuffer, OutputBuffer,
    fileops_impl_noop_sync,
};
use starnix_core::{fileops_impl_seekable, fs_node_impl_not_dir};
use starnix_logging::log_error;
use starnix_sync::{LockDepMutex, NanohubContentsLock, NanohubServiceLock};
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{errno, error};
use zx;

/// Convert an `Option<T>` (usually from a FIDL table) to a `Result<T, SysfsError>`.
pub fn try_get<T>(o: Option<T>) -> Result<T, SysfsError> {
    o.ok_or_else(|| {
        log_error!("Missing expected value from Nanohub service method response.");
        sysfs_errno!(EINVAL)
    })
}

/// A wrapper around `starnix_uapi::errors::Errno`.
#[derive(Debug, PartialEq)]
pub struct SysfsError(pub Errno);

/// An equivalent to `starnix_uapi::errno!`, but for `SysfsError`.
#[macro_export]
macro_rules! sysfs_errno {
    ($error_code:ident) => {
        $crate::sysfs::SysfsError(errno!($error_code))
    };
}

/// An equivalent to `starnix_uapi::error!`, but for `SysfsError`.
#[macro_export]
macro_rules! sysfs_error {
    ($error_code:ident) => {
        Err($crate::sysfs::SysfsError(errno!($error_code)))
    };
}

impl From<fidl::Error> for SysfsError {
    /// Generate the sysfs error response for a FIDL transport error.
    ///
    /// This allows you to handle the error case simply by using the ? operator.
    fn from(value: fidl::Error) -> Self {
        log_error!("FIDL error while calling service method: {value:?}");
        sysfs_errno!(EINVAL)
    }
}

impl From<i32> for SysfsError {
    /// Generate the sysfs error response for a zx.Status error result from a FIDL method.
    ///
    /// This allows you to handle the error case simply by using the ? operator.
    fn from(value: i32) -> Self {
        let status = zx::Status::err_from_raw(value);
        log_error!("Service method responded with an error: {status:?}");
        sysfs_errno!(EINVAL)
    }
}

/// Operations supported by sysfs files.
///
/// These are analogous to (but not identical to) the operations exposed by Linux sysfs attributes.
pub trait SysfsOps<S: fidl::endpoints::SynchronousProxy>: Default + Send + Sync + 'static {
    /// Get the value of the attribute.
    fn show(&self, _service: &S) -> Result<String, SysfsError> {
        sysfs_error!(EINVAL)
    }

    /// Store a new value for the attribute.
    fn store(&self, _service: &S, _value: String) -> Result<(), SysfsError> {
        sysfs_error!(EINVAL)
    }
}

enum SysfsContentsState {
    Unarmed,
    Armed(String),
    Err(Errno),
}

impl AsRef<SysfsContentsState> for SysfsContentsState {
    fn as_ref(&self) -> &Self {
        &self
    }
}

/// A file that exposes data via sysfs semantics.
pub struct SysfsFile<P: ProtocolMarker, O: SysfsOps<P::SynchronousProxy>> {
    /// Implementations for sysfs operations.
    sysfs_ops: Box<O>,

    /// An active connection to the FIDL service.
    service: LockDepMutex<P::SynchronousProxy, NanohubServiceLock>,

    /// The buffered contents of the return of the underlying FIDL method.
    ///
    /// To match SysFS behavior on Linux, this buffer is only populated/refreshed when the file is
    /// read from offset position 0, so at the moment the file is opened (i.e., at the moment this
    /// struct is instantiated), the contents are `Unarmed`. Once the the buffer is populated,
    /// the result contains either the data returned by the `show` method or a Linux error code.
    contents: LockDepMutex<SysfsContentsState, NanohubContentsLock>,

    _phantom: PhantomData<P>,
}

impl<P: DiscoverableProtocolMarker, O: SysfsOps<P::SynchronousProxy>> SysfsFile<P, O> {
    pub fn new(sysfs_ops: Box<O>) -> Result<Box<Self>, Errno> {
        let service = connect_to_protocol_sync::<P>().map_err(|e| {
            log_error!("Error connecting to service: {:?}", e);
            errno!(EIO)
        })?;

        Ok(Box::new(SysfsFile {
            sysfs_ops,
            service: service.into(),
            contents: SysfsContentsState::Unarmed.into(),
            _phantom: PhantomData,
        }))
    }

    /// Refresh the data in the file buffer by calling `show`.
    ///
    /// This is analogous to sysfs rearming in Linux.
    fn rearm(&self) {
        let op_result = self.sysfs_ops.show(&self.service.lock());
        let mut contents_guard = self.contents.lock();
        *contents_guard = match op_result {
            Ok(value) => SysfsContentsState::Armed(value),
            Err(error) => SysfsContentsState::Err(error.0),
        }
    }
}

impl<P: DiscoverableProtocolMarker, O: SysfsOps<P::SynchronousProxy>> FileOps for SysfsFile<P, O> {
    fileops_impl_seekable!();
    fileops_impl_noop_sync!();

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        // Reading from offset 0 refreshes the contents buffer via the underlying `show` method.
        if offset == 0 {
            self.rearm();
        };

        let contents_guard = self.contents.lock();

        match contents_guard.as_ref() {
            SysfsContentsState::Err(error) => Err(error.clone()),
            SysfsContentsState::Unarmed => {
                log_error!("Failed to read SysFS file with no contents");
                error!(EINVAL)
            }
            SysfsContentsState::Armed(value) => {
                // Write the slice of data requested by the caller from the contents buffer,
                // returning the number of bytes written. If the offset is at the end of the
                // contents (e.g., if the contents have already been read), this will write 0
                // bytes, indicating EOF. If an error occurred, nothing will be written and the
                // error will be returned.
                let bytes = value.as_bytes();
                let start = offset.min(bytes.len());
                let end = (offset + data.available()).min(bytes.len());
                data.write(&bytes[start..end])
            }
        }
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        // The entire contents of the input buffer are read and stored in the attribute, regardless
        // of the current file position.
        data.read_all()
            .and_then(|bytes| {
                String::from_utf8(bytes)
                    .map_err(|e| {
                        log_error!("Failed convert to input buffer to string: {e:?}");
                        errno!(EINVAL)
                    })
                    .map(|v| (v.len(), v))
            })
            .and_then(|(num_bytes, value)| {
                self.sysfs_ops
                    .store(&self.service.lock(), value)
                    .map(|_| num_bytes)
                    .map_err(|e| e.0)
            })
    }
}

/// File node for sysfs files.
pub struct SysfsNode<P: ProtocolMarker, O: SysfsOps<P::SynchronousProxy>> {
    _phantom_p: PhantomData<P>,
    _phantom_o: PhantomData<O>,
}

impl<P: DiscoverableProtocolMarker, O: SysfsOps<P::SynchronousProxy>> SysfsNode<P, O> {
    pub fn new() -> Self {
        Self { _phantom_p: PhantomData, _phantom_o: PhantomData }
    }
}

impl<P: DiscoverableProtocolMarker, O: SysfsOps<P::SynchronousProxy>> FsNodeOps
    for SysfsNode<P, O>
{
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        SysfsFile::<P, O>::new(Box::new(O::default())).map(|f| f as Box<dyn FileOps>)
    }

    /// Support no-op trunc for Nanohub sysfs endpoints
    /// This is a safe no-op as these nodes do not have persistent file backings
    /// and only support show/store operations which are mapped to read/write FileOps.
    fn truncate(
        &self,
        _guard: &AppendLockWriteGuard<'_>,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _length: u64,
    ) -> Result<(), Errno> {
        Ok(())
    }
}
