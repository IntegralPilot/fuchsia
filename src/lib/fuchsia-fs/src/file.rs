// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utility functions for fuchsia.io files.

use crate::node::{CloseError, OpenError};
use fidl::{Persistable, persist, unpersist};
use flex_fuchsia_io as fio;
use thiserror::Error;

mod async_reader;
pub use async_reader::AsyncReader;

mod async_read_at;
pub use async_read_at::{Adapter, AsyncFile, AsyncGetSize, AsyncGetSizeExt, AsyncReadAt};
mod async_read_at_ext;
pub use async_read_at_ext::AsyncReadAtExt;
mod buffered_async_read_at;
pub use buffered_async_read_at::BufferedAsyncReadAt;

#[cfg(target_os = "fuchsia")]
#[cfg(not(feature = "fdomain"))]
pub use fuchsia::*;

#[cfg(target_os = "fuchsia")]
#[cfg(not(feature = "fdomain"))]
mod fuchsia {
    use super::*;
    use crate::node::{Kind, take_on_open_event};

    /// An error encountered while reading a named file
    #[derive(Debug, Error)]
    #[error("error reading '{path}': {source}")]
    pub struct ReadNamedError {
        pub(super) path: String,

        #[source]
        pub(super) source: ReadError,
    }

    impl ReadNamedError {
        /// Returns the path associated with this error.
        pub fn path(&self) -> &str {
            &self.path
        }

        /// Unwraps the inner read error, discarding the associated path.
        pub fn into_inner(self) -> ReadError {
            self.source
        }

        /// Returns true if the read failed because the file was no found.
        pub fn is_not_found_error(&self) -> bool {
            self.source.is_not_found_error()
        }
    }

    /// An error encountered while writing a named file
    #[derive(Debug, Error)]
    #[error("error writing '{path}': {source}")]
    pub struct WriteNamedError {
        pub(super) path: String,

        #[source]
        pub(super) source: WriteError,
    }

    impl WriteNamedError {
        /// Returns the path associated with this error.
        pub fn path(&self) -> &str {
            &self.path
        }

        /// Unwraps the inner write error, discarding the associated path.
        pub fn into_inner(self) -> WriteError {
            self.source
        }
    }

    /// Opens the given `path` from the current namespace as a [`FileProxy`].
    ///
    /// To connect to a filesystem node which doesn't implement fuchsia.io.File, use the functions
    /// in [`fuchsia_component::client`] instead.
    ///
    /// If the namespace path doesn't exist, or we fail to make the channel pair, this returns an
    /// error. However, if incorrect flags are sent, or if the rest of the path sent to the
    /// filesystem server doesn't exist, this will still return success. Instead, the returned
    /// FileProxy channel pair will be closed with an epitaph.
    pub fn open_in_namespace(path: &str, flags: fio::Flags) -> Result<fio::FileProxy, OpenError> {
        let (node, request) = fidl::endpoints::create_proxy();
        open_channel_in_namespace(path, flags, request)?;
        Ok(node)
    }

    /// Asynchronously opens the given [`path`] in the current namespace, serving the connection
    /// over [`request`]. Once the channel is connected, any calls made prior are serviced.
    ///
    /// To connect to a filesystem node which doesn't implement fuchsia.io.File, use the functions
    /// in [`fuchsia_component::client`] instead.
    ///
    /// If the namespace path doesn't exist, this returns an error. However, if incorrect flags are
    /// sent, or if the rest of the path sent to the filesystem server doesn't exist, this will
    /// still return success. Instead, the [`request`] channel will be closed with an epitaph.
    pub fn open_channel_in_namespace(
        path: &str,
        flags: fio::Flags,
        request: fidl::endpoints::ServerEnd<fio::FileMarker>,
    ) -> Result<(), OpenError> {
        let flags = flags | fio::Flags::PROTOCOL_FILE;
        let namespace = fdio::Namespace::installed().map_err(OpenError::Namespace)?;
        namespace.open(path, flags, request.into_channel()).map_err(OpenError::Namespace)
    }

    /// Write the given data into a file at `path` in the current namespace. The path must be an
    /// absolute path.
    /// * If the file already exists, replaces existing contents.
    /// * If the file does not exist, creates the file.
    pub async fn write_in_namespace<D>(path: &str, data: D) -> Result<(), WriteNamedError>
    where
        D: AsRef<[u8]>,
    {
        async {
            let flags =
                fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::FILE_TRUNCATE | fio::PERM_WRITABLE;
            let file = open_in_namespace(path, flags)?;

            write(&file, data).await?;

            let _ = close(file).await;
            Ok(())
        }
        .await
        .map_err(|source| WriteNamedError { path: path.to_owned(), source })
    }

    /// Write the given FIDL encoded message into a file at `path`. The path must be an absolute
    /// path.
    /// * If the file already exists, replaces existing contents.
    /// * If the file does not exist, creates the file.
    pub async fn write_fidl_in_namespace<T: Persistable>(
        path: &str,
        data: &mut T,
    ) -> Result<(), WriteNamedError> {
        let data = persist(data)
            .map_err(|source| WriteNamedError { path: path.to_owned(), source: source.into() })?;
        write_in_namespace(path, data).await?;
        Ok(())
    }

    /// Reads all data from the file at `path` in the current namespace. The path must be an
    /// absolute path.
    pub async fn read_in_namespace(path: &str) -> Result<Vec<u8>, ReadNamedError> {
        async {
            let file = open_in_namespace(
                path,
                fio::Flags::FLAG_SEND_REPRESENTATION
                    | fio::PERM_READABLE
                    | fio::Flags::PROTOCOL_FILE,
            )?;
            read_file_with_on_open_event(file).await
        }
        .await
        .map_err(|source| ReadNamedError { path: path.to_owned(), source })
    }

    /// Reads a utf-8 encoded string from the file at `path` in the current namespace. The path must
    /// be an absolute path.
    pub async fn read_in_namespace_to_string(path: &str) -> Result<String, ReadNamedError> {
        let bytes = read_in_namespace(path).await?;
        let string = String::from_utf8(bytes)
            .map_err(|source| ReadNamedError { path: path.to_owned(), source: source.into() })?;
        Ok(string)
    }

    /// Read the given FIDL message from binary file at `path` in the current namespace. The path
    /// must be an absolute path.
    /// FIDL structure should be provided at a read time.
    /// Incompatible data is populated as per FIDL ABI compatibility guide:
    /// https://fuchsia.dev/fuchsia-src/development/languages/fidl/guides/abi-compat
    pub async fn read_in_namespace_to_fidl<T: Persistable>(
        path: &str,
    ) -> Result<T, ReadNamedError> {
        let bytes = read_in_namespace(path).await?;
        unpersist(&bytes)
            .map_err(|source| ReadNamedError { path: path.to_owned(), source: source.into() })
    }

    /// Extracts the stream from an OnOpen or OnRepresentation FileEvent.
    pub(super) fn extract_stream_from_on_open_event(
        event: fio::FileEvent,
    ) -> Result<Option<zx::Stream>, OpenError> {
        match event {
            #[cfg(any(
                fuchsia_api_level_at_least = "PLATFORM",
                not(fuchsia_api_level_at_least = "32")
            ))]
            fio::FileEvent::OnOpen_ { s: status, info } => {
                zx::Status::ok(status).map_err(OpenError::OpenError)?;
                let node_info = info.ok_or(OpenError::MissingOnOpenInfo)?;
                match *node_info {
                    fio::NodeInfoDeprecated::File(file_info) => Ok(file_info.stream),
                    node_info @ _ => Err(OpenError::UnexpectedNodeKind {
                        expected: Kind::File,
                        actual: Kind::kind_of(&node_info),
                    }),
                }
            }
            fio::FileEvent::OnRepresentation { payload } => match payload {
                fio::Representation::File(file_info) => Ok(file_info.stream),
                representation @ _ => Err(OpenError::UnexpectedNodeKind {
                    expected: Kind::File,
                    actual: Kind::kind_of2(&representation),
                }),
            },
            fio::FileEvent::_UnknownEvent { ordinal, .. } => {
                Err(OpenError::UnknownEvent { ordinal })
            }
        }
    }

    /// Reads the contents of a stream into a Vec.
    pub(super) fn read_contents_of_stream(stream: zx::Stream) -> Result<Vec<u8>, ReadError> {
        // TODO(https://fxbug.dev/324239375): Get the file size from the OnRepresentation event.
        let file_size =
            stream.seek(std::io::SeekFrom::End(0)).map_err(ReadError::ReadError)? as usize;
        let mut data = Vec::with_capacity(file_size);
        let mut remaining = file_size;
        while remaining > 0 {
            // read_at is used instead of read because the seek offset was moved to the end of the
            // file to determine the file size. Moving the seek offset back to the start of the file
            // would require another syscall.
            let actual = stream
                .read_at_uninit(
                    zx::StreamReadOptions::empty(),
                    data.len() as u64,
                    &mut data.spare_capacity_mut()[0..remaining],
                )
                .map_err(ReadError::ReadError)?;
            // A read of 0 bytes indicates the end of the file was reached. The file may have
            // changed size since the seek.
            if actual == 0 {
                break;
            }
            // SAFETY: read_at_uninit returns the number of bytes that were read and initialized.
            unsafe { data.set_len(data.len() + actual) };
            remaining -= actual;
        }
        Ok(data)
    }

    /// Writes `contents` into `stream`.
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    pub(super) fn write_to_stream(stream: zx::Stream, contents: &[u8]) -> Result<(), WriteError> {
        let mut offset: usize = 0;
        while offset < contents.len() {
            let actual = stream
                .write(zx::StreamWriteOptions::empty(), &contents[offset..])
                .map_err(WriteError::WriteError)?;
            // Per the syscall docs, 0 will never be returned (an error is returned instead), but
            // make sure we don't loop forever.
            debug_assert!(actual > 0);
            offset += actual;
        }
        Ok(())
    }

    /// Reads the contents of `file` into a Vec. `file` must have been opened with
    /// `SEND_REPRESENTATION` and the event must not have been read yet.
    pub(crate) async fn read_file_with_on_open_event(
        file: fio::FileProxy,
    ) -> Result<Vec<u8>, ReadError> {
        let event = take_on_open_event(&file).await.map_err(ReadError::Open)?;
        let stream = extract_stream_from_on_open_event(event).map_err(ReadError::Open)?;

        if let Some(stream) = stream {
            read_contents_of_stream(stream)
        } else {
            // Fall back to FIDL reads if the file doesn't support streams.
            read(&file).await
        }
    }

    /// Writes the contents of `buf` into `file`. `file` must have been opened with
    /// `SEND_REPRESENTATION` and the event must not have been read yet.
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    pub(crate) async fn write_file_with_on_open_event(
        file: &fio::FileProxy,
        contents: &[u8],
    ) -> Result<(), WriteError> {
        let event = take_on_open_event(file).await.map_err(WriteError::Open)?;
        let stream = extract_stream_from_on_open_event(event).map_err(WriteError::Open)?;

        if let Some(stream) = stream {
            write_to_stream(stream, contents)
        } else {
            // Fall back to FIDL writes if the file doesn't support streams.
            write(file, contents).await
        }
    }
}

/// An error encountered while reading a file
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum ReadError {
    #[error("while opening the file: {0:?}")]
    Open(#[from] OpenError),

    #[error("read call failed: {0:?}")]
    Fidl(#[from] fidl::Error),

    #[error("read failed with status: {0}")]
    ReadError(#[source] zx_status::Status),

    #[error("file was not a utf-8 encoded string: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
}

impl ReadError {
    /// Returns true if the read failed because the file was no found.
    pub fn is_not_found_error(&self) -> bool {
        matches!(self, ReadError::Open(e) if e.is_not_found_error())
    }
}

/// An error encountered while writing a file
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum WriteError {
    #[error("while opening the file: {0}")]
    Open(#[from] OpenError),

    #[error("while linking the file: {0}")]
    Link(#[from] zx_status::Status),

    #[error("while renaming the file: {0}")]
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    Rename(#[from] crate::node::RenameError),

    #[error("write call failed: {0}")]
    Fidl(#[from] fidl::Error),

    #[error("write failed with status: {0}")]
    WriteError(#[source] zx_status::Status),

    #[error("file endpoint reported more bytes written than were provided")]
    Overwrite,
}

/// Gracefully closes the file proxy from the remote end.
pub async fn close(file: fio::FileProxy) -> Result<(), CloseError> {
    let result = file.close().await.map_err(CloseError::SendCloseRequest)?;
    result.map_err(|s| CloseError::CloseError(zx_status::Status::err_from_raw(s)))
}

/// Writes the given data into the given file.
pub async fn write<D>(file: &fio::FileProxy, data: D) -> Result<(), WriteError>
where
    D: AsRef<[u8]>,
{
    let mut data = data.as_ref();

    while !data.is_empty() {
        let bytes_written = file
            .write(&data[..std::cmp::min(fio::MAX_BUF as usize, data.len())])
            .await?
            .map_err(|s| WriteError::WriteError(zx_status::Status::err_from_raw(s)))?;

        if bytes_written > data.len() as u64 {
            return Err(WriteError::Overwrite);
        }

        data = &data[bytes_written as usize..];
    }
    Ok(())
}

/// Write the given FIDL message in a binary form into a file open for writing.
pub async fn write_fidl<T: Persistable>(
    file: &fio::FileProxy,
    data: &mut T,
) -> Result<(), WriteError> {
    write(file, persist(data)?).await?;
    Ok(())
}

/// Reads all data from the given file's current offset to the end of the file.
pub async fn read(file: &fio::FileProxy) -> Result<Vec<u8>, ReadError> {
    let mut out = Vec::new();

    loop {
        let mut bytes = file
            .read(fio::MAX_BUF)
            .await?
            .map_err(|s| ReadError::ReadError(zx_status::Status::err_from_raw(s)))?;
        if bytes.is_empty() {
            break;
        }
        out.append(&mut bytes);
    }
    Ok(out)
}

/// Attempts to read a number of bytes from the given file's current offset.
/// This function may return less data than expected.
pub async fn read_num_bytes(file: &fio::FileProxy, num_bytes: u64) -> Result<Vec<u8>, ReadError> {
    let mut data = vec![];

    // Read in chunks of |MAX_BUF| bytes.
    // This is the maximum buffer size supported over FIDL.
    let mut bytes_left = num_bytes;
    while bytes_left > 0 {
        let bytes_to_read = std::cmp::min(bytes_left, fio::MAX_BUF);
        let mut bytes = file
            .read(bytes_to_read)
            .await?
            .map_err(|s| ReadError::ReadError(zx_status::Status::err_from_raw(s)))?;

        if bytes.is_empty() {
            break;
        }

        bytes_left -= bytes.len() as u64;
        data.append(&mut bytes);
    }

    // Remove excess data read in, if any.
    let num_bytes = num_bytes as usize;
    if data.len() > num_bytes {
        data.drain(num_bytes..data.len());
    }

    Ok(data)
}

/// Reads a utf-8 encoded string from the given file's current offset to the end of the file.
pub async fn read_to_string(file: &fio::FileProxy) -> Result<String, ReadError> {
    let bytes = read(file).await?;
    let string = String::from_utf8(bytes)?;
    Ok(string)
}

/// Read the given FIDL message from binary form from a file open for reading.
/// FIDL structure should be provided at a read time.
/// Incompatible data is populated as per FIDL ABI compatibility guide:
/// https://fuchsia.dev/fuchsia-src/development/languages/fidl/guides/abi-compat
pub async fn read_fidl<T: Persistable>(file: &fio::FileProxy) -> Result<T, ReadError> {
    let bytes = read(file).await?;
    Ok(unpersist(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory;
    use crate::node::{Kind, take_on_open_event};
    use assert_matches::assert_matches;
    use fidl_fidl_test_schema::{DataTable1, DataTable2};
    use fuchsia_async as fasync;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::TempDir;
    use vfs::ToObjectRequest;
    use vfs::execution_scope::ExecutionScope;
    use vfs::file::vmo::{VmoFile, read_only};

    const DATA_FILE_CONTENTS: &str = "Hello World!\n";

    // open_in_namespace

    #[fasync::run_singlethreaded(test)]
    async fn open_in_namespace_opens_real_file() {
        let exists = open_in_namespace("/pkg/data/file", fio::PERM_READABLE).unwrap();
        assert_matches!(close(exists).await, Ok(()));
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_in_namespace_opens_fake_file_under_of_root_namespace_entry() {
        let notfound = open_in_namespace("/pkg/fake", fio::PERM_READABLE).unwrap();
        // The open error is not detected until the proxy is interacted with.
        assert_matches!(close(notfound).await, Err(_));
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_in_namespace_rejects_fake_root_namespace_entry() {
        assert_matches!(
            open_in_namespace("/fake", fio::PERM_READABLE),
            Err(OpenError::Namespace(zx_status::Status::NOT_FOUND))
        );
    }

    // write_in_namespace

    #[fasync::run_singlethreaded(test)]
    async fn write_in_namespace_creates_file() {
        let tempdir = TempDir::new().unwrap();
        let path = tempdir.path().join(Path::new("new-file")).to_str().unwrap().to_owned();

        // Write contents.
        let data = b"\x80"; // Non UTF-8 data: a continuation byte as the first byte.
        write_in_namespace(&path, data).await.unwrap();

        // Verify contents.
        let contents = std::fs::read(&path).unwrap();
        assert_eq!(&contents, &data);
    }

    #[fasync::run_singlethreaded(test)]
    async fn write_in_namespace_overwrites_existing_file() {
        let tempdir = TempDir::new().unwrap();
        let path = tempdir.path().join(Path::new("existing-file")).to_str().unwrap().to_owned();

        // Write contents.
        let original_data = b"\x80\x81"; // Non UTF-8 data: a continuation byte as the first byte.
        write_in_namespace(&path, original_data).await.unwrap();

        // Over-write contents.
        let new_data = b"\x82"; // Non UTF-8 data: a continuation byte as the first byte.
        write_in_namespace(&path, new_data).await.unwrap();

        // Verify contents.
        let contents = std::fs::read(&path).unwrap();
        assert_eq!(&contents, &new_data);
    }

    #[fasync::run_singlethreaded(test)]
    async fn write_in_namespace_fails_on_invalid_namespace_entry() {
        assert_matches!(
            write_in_namespace("/fake", b"").await,
            Err(WriteNamedError { path, source: WriteError::Open(_) }) if path == "/fake"
        );
        let err = write_in_namespace("/fake", b"").await.unwrap_err();
        assert_eq!(err.path(), "/fake");
        assert_matches!(err.into_inner(), WriteError::Open(_));
    }

    // write

    #[fasync::run_singlethreaded(test)]
    async fn write_writes_to_file() {
        let tempdir = TempDir::new().unwrap();
        let dir = directory::open_in_namespace(
            tempdir.path().to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .unwrap();

        // Write contents.
        let file =
            directory::open_file(&dir, "file", fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE)
                .await
                .unwrap();
        let data = b"\x80"; // Non UTF-8 data: a continuation byte as the first byte.
        write(&file, data).await.unwrap();

        // Verify contents.
        let contents = std::fs::read(tempdir.path().join(Path::new("file"))).unwrap();
        assert_eq!(&contents, &data);
    }

    #[fasync::run_singlethreaded(test)]
    async fn write_writes_to_file_in_chunks_if_needed() {
        let tempdir = TempDir::new().unwrap();
        let dir = directory::open_in_namespace(
            tempdir.path().to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .unwrap();

        // Write contents.
        let file =
            directory::open_file(&dir, "file", fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE)
                .await
                .unwrap();
        let data = "abc".repeat(10000);
        write(&file, &data).await.unwrap();

        // Verify contents.
        let contents = std::fs::read_to_string(tempdir.path().join(Path::new("file"))).unwrap();
        assert_eq!(&contents, &data);
    }

    #[fasync::run_singlethreaded(test)]
    async fn write_appends_to_file() {
        let tempdir = TempDir::new().unwrap();
        let dir = directory::open_in_namespace(
            tempdir.path().to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .unwrap();

        // Create and write to the file.
        let file =
            directory::open_file(&dir, "file", fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE)
                .await
                .unwrap();
        write(&file, "Hello ").await.unwrap();
        write(&file, "World!\n").await.unwrap();
        close(file).await.unwrap();

        // Verify contents.
        let contents = std::fs::read(tempdir.path().join(Path::new("file"))).unwrap();
        assert_eq!(&contents[..], DATA_FILE_CONTENTS.as_bytes());
    }

    #[fasync::run_singlethreaded(test)]
    async fn write_stream() {
        let tempdir = TempDir::new().unwrap();
        let dir = directory::open_in_namespace(
            tempdir.path().to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .unwrap();

        // Create and write to the file.
        let file =
            directory::open_file(&dir, "file", fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE)
                .await
                .unwrap();
        write(&file, "Hello ").await.unwrap();
        write(&file, "World!\n").await.unwrap();
        close(file).await.unwrap();

        // Verify contents.
        let contents = std::fs::read(tempdir.path().join(Path::new("file"))).unwrap();
        assert_eq!(&contents[..], DATA_FILE_CONTENTS.as_bytes());
    }

    // read

    #[fasync::run_singlethreaded(test)]
    async fn read_reads_to_end_of_file() {
        let file = open_in_namespace("/pkg/data/file", fio::PERM_READABLE).unwrap();

        let contents = read(&file).await.unwrap();
        assert_eq!(&contents[..], DATA_FILE_CONTENTS.as_bytes());
    }

    #[fasync::run_singlethreaded(test)]
    async fn read_reads_from_current_position() {
        let file = open_in_namespace("/pkg/data/file", fio::PERM_READABLE).unwrap();

        // Advance past the first byte.
        let _: Vec<u8> = file.read(1).await.unwrap().unwrap();

        // Verify the rest of the file is read.
        let contents = read(&file).await.unwrap();
        assert_eq!(&contents[..], "ello World!\n".as_bytes());
    }

    // read_in_namespace

    #[fasync::run_singlethreaded(test)]
    async fn read_in_namespace_reads_contents() {
        let contents = read_in_namespace("/pkg/data/file").await.unwrap();
        assert_eq!(&contents[..], DATA_FILE_CONTENTS.as_bytes());
    }

    #[fasync::run_singlethreaded(test)]
    async fn read_in_namespace_fails_on_invalid_namespace_entry() {
        assert_matches!(
            read_in_namespace("/fake").await,
            Err(ReadNamedError { path, source: ReadError::Open(_) }) if path == "/fake"
        );
        let err = read_in_namespace("/fake").await.unwrap_err();
        assert_eq!(err.path(), "/fake");
        assert_matches!(err.into_inner(), ReadError::Open(_));
    }

    // read_to_string

    #[fasync::run_singlethreaded(test)]
    async fn read_to_string_reads_data_file() {
        let file = open_in_namespace("/pkg/data/file", fio::PERM_READABLE).unwrap();
        assert_eq!(read_to_string(&file).await.unwrap(), DATA_FILE_CONTENTS);
    }

    // read_in_namespace_to_string

    #[fasync::run_singlethreaded(test)]
    async fn read_in_namespace_to_string_reads_data_file() {
        assert_eq!(
            read_in_namespace_to_string("/pkg/data/file").await.unwrap(),
            DATA_FILE_CONTENTS
        );
    }

    // write_fidl

    #[fasync::run_singlethreaded(test)]
    async fn write_fidl_writes_to_file() {
        let tempdir = TempDir::new().unwrap();
        let dir = directory::open_in_namespace(
            tempdir.path().to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .unwrap();

        // Write contents.
        let file =
            directory::open_file(&dir, "file", fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE)
                .await
                .unwrap();

        let mut data = DataTable1 {
            num: Some(42),
            string: Some(DATA_FILE_CONTENTS.to_string()),
            ..Default::default()
        };

        // Binary encoded FIDL message, with header and padding.
        let fidl_bytes = persist(&data).unwrap();

        write_fidl(&file, &mut data).await.unwrap();

        // Verify contents.
        let contents = std::fs::read(tempdir.path().join(Path::new("file"))).unwrap();
        assert_eq!(&contents, &fidl_bytes);
    }

    #[fasync::run_singlethreaded(test)]
    async fn read_fidl_reads_from_file() {
        let file = open_in_namespace("/pkg/data/fidl_file", fio::PERM_READABLE).unwrap();

        let contents = read_fidl::<DataTable2>(&file).await.unwrap();

        let data = DataTable2 {
            num: Some(42),
            string: Some(DATA_FILE_CONTENTS.to_string()),
            new_field: None,
            ..Default::default()
        };
        assert_eq!(&contents, &data);
    }

    #[test]
    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    fn extract_stream_from_on_open_event_with_stream() {
        let vmo = zx::Vmo::create(0).unwrap();
        let stream = zx::Stream::create(zx::StreamOptions::empty(), &vmo, 0).unwrap();
        let event = fio::FileEvent::OnOpen_ {
            s: 0,
            info: Some(Box::new(fio::NodeInfoDeprecated::File(fio::FileObject {
                stream: Some(stream),
                event: None,
            }))),
        };
        let stream = extract_stream_from_on_open_event(event)
            .expect("Not a file")
            .expect("Stream not present");
        assert!(!stream.is_invalid());
    }

    #[test]
    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    fn extract_stream_from_on_open_event_without_stream() {
        let event = fio::FileEvent::OnOpen_ {
            s: 0,
            info: Some(Box::new(fio::NodeInfoDeprecated::File(fio::FileObject {
                stream: None,
                event: None,
            }))),
        };
        let stream = extract_stream_from_on_open_event(event).expect("Not a file");
        assert!(stream.is_none());
    }

    #[test]
    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    fn extract_stream_from_on_open_event_with_open_error() {
        let event = fio::FileEvent::OnOpen_ { s: zx::Status::NOT_FOUND.into_raw(), info: None };
        let result = extract_stream_from_on_open_event(event);
        assert_matches!(result, Err(OpenError::OpenError(zx::Status::NOT_FOUND)));
    }

    #[test]
    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    fn extract_stream_from_on_open_event_not_a_file() {
        let event = fio::FileEvent::OnOpen_ {
            s: 0,
            info: Some(Box::new(fio::NodeInfoDeprecated::Service(fio::Service))),
        };
        let result = extract_stream_from_on_open_event(event);
        assert_matches!(
            result,
            Err(OpenError::UnexpectedNodeKind { expected: Kind::File, actual: Kind::Service })
        );
    }

    #[test]
    fn extract_stream_from_on_representation_event_with_stream() {
        let vmo = zx::Vmo::create(0).unwrap();
        let stream = zx::Stream::create(zx::StreamOptions::empty(), &vmo, 0).unwrap();
        let event = fio::FileEvent::OnRepresentation {
            payload: fio::Representation::File(fio::FileInfo {
                stream: Some(stream),
                ..Default::default()
            }),
        };
        let stream = extract_stream_from_on_open_event(event)
            .expect("Not a file")
            .expect("Stream not present");
        assert!(!stream.is_invalid());
    }

    #[test]
    fn extract_stream_from_on_representation_event_without_stream() {
        let event = fio::FileEvent::OnRepresentation {
            payload: fio::Representation::File(fio::FileInfo::default()),
        };
        let stream = extract_stream_from_on_open_event(event).expect("Not a file");
        assert!(stream.is_none());
    }

    #[test]
    fn extract_stream_from_on_representation_event_not_a_file() {
        let event = fio::FileEvent::OnRepresentation {
            payload: fio::Representation::Directory(Default::default()),
        };
        let result = extract_stream_from_on_open_event(event);
        assert_matches!(
            result,
            Err(OpenError::UnexpectedNodeKind { expected: Kind::File, actual: Kind::Directory })
        );
    }

    #[test]
    fn read_contents_of_stream_with_contents() {
        let data = b"file-contents".repeat(1000);
        let vmo = zx::Vmo::create(data.len() as u64).unwrap();
        vmo.write(&data, 0).unwrap();
        let stream = zx::Stream::create(zx::StreamOptions::MODE_READ, &vmo, 0).unwrap();
        let contents = read_contents_of_stream(stream).unwrap();
        assert_eq!(contents, data);
    }

    #[test]
    fn read_contents_of_stream_with_empty_stream() {
        let vmo = zx::Vmo::create(0).unwrap();
        let stream = zx::Stream::create(zx::StreamOptions::MODE_READ, &vmo, 0).unwrap();
        let contents = read_contents_of_stream(stream).unwrap();
        assert!(contents.is_empty());
    }

    fn serve_file(file: Arc<VmoFile>, flags: fio::Flags) -> fio::FileProxy {
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::FileMarker>();
        flags.to_object_request(server_end).handle(|object_request| {
            vfs::file::serve(file, ExecutionScope::new(), &flags, object_request)
        });
        proxy
    }

    #[fasync::run_singlethreaded(test)]
    async fn read_file_with_on_open_event_with_stream() {
        let data = b"file-contents".repeat(1000);
        let vmo_file = read_only(&data);
        const FLAGS: fio::Flags = fio::PERM_READABLE.union(fio::Flags::FLAG_SEND_REPRESENTATION);

        {
            // Ensure that the file supports streams.
            let file = serve_file(vmo_file.clone(), FLAGS);
            let event = take_on_open_event(&file).await.unwrap();
            extract_stream_from_on_open_event(event).unwrap().expect("Stream not present");
        }

        let file = serve_file(vmo_file.clone(), FLAGS);
        let contents = read_file_with_on_open_event(file).await.unwrap();
        assert_eq!(contents, data);
    }

    #[fasync::run_singlethreaded(test)]
    async fn read_missing_file_in_namespace() {
        assert_matches!(
            read_in_namespace("/pkg/data/missing").await,
            Err(e) if e.is_not_found_error()
        );
    }

    #[test]
    fn write_to_stream_with_contents() {
        let data = b"file-contents".repeat(1000);
        let vmo = zx::Vmo::create(data.len() as u64).unwrap();
        let stream = zx::Stream::create(zx::StreamOptions::MODE_WRITE, &vmo, 0).unwrap();
        write_to_stream(stream, &data).unwrap();

        let mut read_back = vec![0; data.len()];
        vmo.read(&mut read_back, 0).unwrap();
        assert_eq!(read_back, data);
    }

    #[test]
    fn write_to_stream_empty() {
        let data = b"";
        let vmo = zx::Vmo::create(0).unwrap();
        let stream = zx::Stream::create(zx::StreamOptions::MODE_WRITE, &vmo, 0).unwrap();
        write_to_stream(stream, data).unwrap();
        assert_eq!(vmo.get_size().unwrap(), 0);
    }
}
