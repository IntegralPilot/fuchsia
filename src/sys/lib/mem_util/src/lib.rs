// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for working with the `fuchsia.mem` FIDL library.

use fidl_fuchsia_io as fio;
use fidl_fuchsia_mem as fmem;
use std::borrow::Cow;
use zx_status as zxs;

/// Open `path` from given `parent` directory, returning an [`fmem::Data`] of the contents.
///
/// Prioritizes returning an [`fmem::Data::Buffer`] if it can be done by reusing a VMO handle
/// from the directory's server.
pub async fn open_file_data(
    parent: &fio::DirectoryProxy,
    path: &str,
) -> Result<fmem::Data, FileError> {
    let file = fuchsia_fs::directory::open_file_async(parent, path, fio::PERM_READABLE)?;
    match file
        .get_backing_memory(fio::VmoFlags::READ)
        .await
        .map_err(|e| {
            // Don't swallow the root cause of the error without a trace. It may
            // be impossible to correlate resulting error to its root cause
            // otherwise.
            log::debug!("error for path={}: {}:", path, e);
            FileError::GetBufferError(e)
        })?
        .map_err(zxs::Status::err_from_raw)
    {
        Ok(vmo) => {
            let size = vmo.get_content_size().expect("failed to get VMO size");
            Ok(fmem::Data::Buffer(fmem::Buffer { vmo, size }))
        }
        Err(e) => {
            let _: zxs::Status = e;
            // we still didn't get a VMO handle, fallback to reads over the channel
            let bytes = fuchsia_fs::file::read(&file).await?;
            Ok(fmem::Data::Bytes(bytes))
        }
    }
}

/// Errors that can occur when operating on `DirectoryProxy`s and `FileProxy`s.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("Failed to open a File.")]
    OpenError(#[from] fuchsia_fs::node::OpenError),

    #[error("Couldn't read a file")]
    ReadError(#[from] fuchsia_fs::file::ReadError),

    #[error("FIDL call to retrieve a file's buffer failed")]
    GetBufferError(#[source] fidl::Error),
}

/// Retrieve the bytes in `data`, returning a reference if it's a `Data::Bytes` and a copy of
/// the bytes read from the VMO if it's a `Data::Buffer`.
pub fn bytes_from_data(data: &fmem::Data) -> Result<Cow<'_, [u8]>, DataError> {
    Ok(match data {
        fmem::Data::Buffer(buf) => {
            let size = buf.size as usize;
            let mut raw_bytes = vec![0; size];
            buf.vmo.read(&mut raw_bytes, 0).map_err(DataError::VmoReadError)?;
            Cow::Owned(raw_bytes)
        }
        fmem::Data::Bytes(b) => Cow::Borrowed(b),
        fmem::DataUnknown!() => return Err(DataError::UnrecognizedDataVariant),
    })
}

/// Errors that can occur when operating on `fuchsia.mem.Data` values.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DataError {
    #[error("Couldn't read from VMO")]
    VmoReadError(#[source] zxs::Status),

    #[error("Encountered an unrecognized variant of fuchsia.mem.Data")]
    UnrecognizedDataVariant,
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::sync::Arc;
    use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
    use vfs::execution_scope::ExecutionScope;
    use vfs::file::vmo::read_only;
    use vfs::file::{FileLike, FileOptions};
    use vfs::object_request::Representation;
    use vfs::{ObjectRequestRef, pseudo_directory};
    use zx_status::Status;

    #[fuchsia::test]
    async fn bytes_from_read_only() {
        let fs = pseudo_directory! {
            // `read_only` is a vmo file, returns the buffer in OnOpen
            "foo" => read_only("hello, world!"),
        };
        let directory = vfs::directory::serve_read_only(fs, ExecutionScope::new());

        let data = open_file_data(&directory, "foo").await.unwrap();
        match bytes_from_data(&data).unwrap() {
            Cow::Owned(b) => assert_eq!(b, b"hello, world!"),
            _ => panic!("must produce an owned value from reading contents of fmem::Data::Buffer"),
        }
    }

    /// Test that we get a VMO when the server supports `File/GetBackingMemory`.
    #[fuchsia::test]
    async fn bytes_from_vmo_from_get_buffer() {
        let vmo_data = b"hello, world!";
        let fs = pseudo_directory! {
            "foo" => read_only(vmo_data),
        };
        let directory = vfs::directory::serve_read_only(fs, ExecutionScope::new());

        let data = open_file_data(&directory, "foo").await.unwrap();
        match bytes_from_data(&data).unwrap() {
            Cow::Owned(b) => assert_eq!(b, vmo_data),
            _ => panic!("must produce an owned value from reading contents of fmem::Data::Buffer"),
        }
    }

    /// Test that we correctly fall back to reading through FIDL calls in a channel if the server
    /// doesn't support returning a VMO.
    #[fuchsia::test]
    async fn bytes_from_channel_fallback() {
        // This test File does not handle `File/GetBackingMemory` request, but will return
        // b"hello, world!" on File/Read`.
        struct NonVMOTestFile;

        impl DirectoryEntry for NonVMOTestFile {
            fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
                request.open_file(self)
            }
        }

        impl GetEntryInfo for NonVMOTestFile {
            fn entry_info(&self) -> EntryInfo {
                EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File)
            }
        }

        impl vfs::node::Node for NonVMOTestFile {
            async fn get_attributes(
                &self,
                _requested_attributes: fio::NodeAttributesQuery,
            ) -> Result<fio::NodeAttributes2, Status> {
                Err(Status::NOT_SUPPORTED)
            }
        }

        impl FileLike for NonVMOTestFile {
            fn open(
                self: Arc<Self>,
                scope: ExecutionScope,
                _options: FileOptions,
                object_request: ObjectRequestRef<'_>,
            ) -> Result<(), Status> {
                struct Connection;
                impl Representation for Connection {
                    type Protocol = fio::FileMarker;

                    async fn get_representation(
                        &self,
                        _requested_attributes: fio::NodeAttributesQuery,
                    ) -> Result<fio::Representation, Status> {
                        unreachable!()
                    }

                    async fn node_info(&self) -> Result<fio::NodeInfoDeprecated, Status> {
                        unreachable!()
                    }
                }
                let connection = Connection;
                let object_request = object_request.take();
                scope.spawn(async move {
                    if let Ok(mut file_requests) =
                        object_request.into_request_stream(&connection).await
                    {
                        let mut have_sent_bytes = false;
                        while let Some(Ok(request)) = file_requests.next().await {
                            match request {
                                fio::FileRequest::GetBackingMemory { flags: _, responder } => {
                                    responder.send(Err(Status::NOT_SUPPORTED.into_raw())).unwrap()
                                }
                                fio::FileRequest::Read { count: _, responder } => {
                                    let to_send: &[u8] = if !have_sent_bytes {
                                        have_sent_bytes = true;
                                        b"hello, world!"
                                    } else {
                                        &[]
                                    };
                                    responder.send(Ok(to_send)).unwrap();
                                }
                                unexpected => unimplemented!("{:#?}", unexpected),
                            }
                        }
                    }
                });
                Ok(())
            }
        }

        let fs = pseudo_directory! {
            "foo" => Arc::new(NonVMOTestFile),
        };
        let directory = vfs::directory::serve_read_only(fs, ExecutionScope::new());

        let data = open_file_data(&directory, "foo").await.unwrap();
        let data = bytes_from_data(&data).unwrap();
        assert_eq!(
            data,
            Cow::Borrowed(b"hello, world!"),
            "must produce a borrowed value from fmem::Data::Bytes"
        );
    }
}
