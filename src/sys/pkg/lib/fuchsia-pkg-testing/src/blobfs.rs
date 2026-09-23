// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fake implementation of blobfs for blobfs::Client.

use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_hash::Hash;
use futures::stream::TryStreamExt as _;
use tempfile::TempDir;

/// A fake blobfs backed by temporary storage.
///
/// The name of the blob file is not guaranteed to match the merkle root of the content.
/// Be aware that this implementation does not send USER_0 signal, so `has_blob()` will always
/// return false.
pub struct Fake {
    root: TempDir,
    _reader_server: fasync::Task<()>,
}

impl Fake {
    /// Creates a new fake blobfs and client.
    /// Uses fuchsia_async::Task::spawn and so must be called with an executor installed.
    ///
    /// # Panics
    ///
    /// Panics on error
    pub fn new() -> (Self, blobfs::Client) {
        let root = TempDir::new().unwrap();

        let (reader, reader_stream) =
            fidl::endpoints::create_proxy_and_stream::<ffxfs::BlobReaderMarker>();
        let reader_server = fasync::Task::spawn(serve_reader(root_proxy(&root), reader_stream));

        let blobfs = blobfs::Client::new(root_proxy(&root), None, reader, None).unwrap();
        let fake = Self { root, _reader_server: reader_server };
        (fake, blobfs)
    }

    /// Add a new blob to fake blobfs.
    ///
    /// # Panics
    ///
    /// Panics on error
    pub fn add_blob(&self, hash: Hash, data: impl AsRef<[u8]>) {
        std::fs::write(self.root.path().join(hash.to_string()), data).unwrap();
    }

    /// Delete a blob from the fake blobfs.
    ///
    /// # Panics
    ///
    /// Panics on error
    pub fn delete_blob(&self, hash: Hash) {
        std::fs::remove_file(self.root.path().join(hash.to_string())).unwrap();
    }
}

fn root_proxy(root: &TempDir) -> fio::DirectoryProxy {
    fuchsia_fs::directory::open_in_namespace(root.path().to_str().unwrap(), fio::PERM_READABLE)
        .unwrap()
}

async fn serve_reader(blobs: fio::DirectoryProxy, mut stream: ffxfs::BlobReaderRequestStream) {
    while let Some(req) = stream.try_next().await.unwrap() {
        match req {
            ffxfs::BlobReaderRequest::GetVmo { blob_hash, responder } => {
                match fuchsia_fs::directory::open_file(
                    &blobs,
                    &Hash::from(blob_hash).to_string(),
                    fio::PERM_READABLE,
                )
                .await
                {
                    Ok(blob) => {
                        let vmo = blob
                            .get_backing_memory(fio::VmoFlags::READ)
                            .await
                            .unwrap()
                            .map_err(zx::Status::err_from_raw)
                            .unwrap();
                        let () = responder.send(Ok(vmo)).unwrap();
                    }
                    Err(fuchsia_fs::node::OpenError::OpenError(status))
                        if status == zx::Status::NOT_FOUND =>
                    {
                        let () = responder.send(Err(status.into_raw())).unwrap();
                    }
                    Err(e) => panic!("unexpected error {e:?}"),
                }
            }
        }
    }
}
