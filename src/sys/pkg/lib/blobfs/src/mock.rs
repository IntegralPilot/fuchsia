// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Mock implementation of blobfs for blobfs::Client.

use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_io as fio;
use fuchsia_hash::Hash;
use futures::StreamExt as _;
use std::collections::HashSet;
use std::convert::TryInto as _;

/// A testing server implementation of /blob.
///
/// Mock does not handle requests until instructed to do so.
pub struct Mock {
    pub(super) stream: fio::DirectoryRequestStream,
    pub(super) reader_stream: ffxfs::BlobReaderRequestStream,
    pub(super) creator_stream: ffxfs::BlobCreatorRequestStream,
}

impl Mock {
    /// Consume the next BlobCreator request, verifying it is intended to create the blob identified
    /// by `merkle`. Fail the request with `e`.
    ///
    /// # Panics
    ///
    /// Panics on error or assertion violation (unexpected requests or a mismatched open call)
    pub async fn fail_create(&mut self, merkle: Hash, e: ffxfs::CreateBlobError) {
        match self.creator_stream.next().await {
            Some(Ok(ffxfs::BlobCreatorRequest::Create { hash, allow_existing, responder })) => {
                assert_eq!(Hash::from(hash), merkle);
                assert!(!allow_existing);
                let () = responder.send(Err(e)).unwrap();
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    /// Consume the next BlobCreator request, verifying it is intended to create the blob identified
    /// by `merkle` with the same `allow_existing`. Return a `BlobWriter` for validating the writes.
    ///
    /// # Panics
    ///
    /// Panics on error or assertion violation (unexpected requests or a mismatched open call)
    pub async fn expect_create_blob(
        &mut self,
        merkle: Hash,
        expected_allow_existing: bool,
    ) -> BlobWriter {
        match self.creator_stream.next().await {
            Some(Ok(ffxfs::BlobCreatorRequest::Create { hash, allow_existing, responder })) => {
                assert_eq!(Hash::from(hash), merkle);
                assert_eq!(allow_existing, expected_allow_existing);
                let (writer, stream) =
                    fidl::endpoints::create_request_stream::<ffxfs::BlobWriterMarker>();
                let () = responder.send(Ok(writer)).unwrap();
                BlobWriter { stream, vmo: None }
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    /// Consume the next BlobReader request, verifying it is intended to open the blob identified
    /// by `merkle`. Either serve the contents of `res.ok()` or fail the open with `res.err()`.
    ///
    /// # Panics
    ///
    /// Panics on error or assertion violation (unexpected requests or a mismatched open call)
    pub async fn expect_open_blob(&mut self, merkle: Hash, res: Result<Vec<u8>, zx::Status>) {
        match self.reader_stream.next().await {
            Some(Ok(ffxfs::BlobReaderRequest::GetVmo { blob_hash, responder })) => {
                assert_eq!(Hash::from(blob_hash), merkle);
                match res {
                    Ok(content) => {
                        let vmo = zx::Vmo::create(content.len().try_into().unwrap()).unwrap();
                        let () = vmo.write(&content, 0).unwrap();
                        let () = responder.send(Ok(vmo)).unwrap();
                    }
                    Err(s) => {
                        let () = responder.send(Err(s.into_raw())).unwrap();
                    }
                }
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    /// Consume the next BlobCreator request, verifying it is intended to check whether the blob
    /// identified by `merkle` needs to be overwritten. Either respond with `res.ok()` or fail with
    /// `res.err()`.
    ///
    /// # Panics
    ///
    /// Panics on error or assertion violation (unexpected requests or a mismatched call)
    pub async fn expect_needs_overwrite(&mut self, merkle: Hash, res: Result<bool, zx::Status>) {
        match self.creator_stream.next().await {
            Some(Ok(ffxfs::BlobCreatorRequest::NeedsOverwrite { blob_hash, responder })) => {
                assert_eq!(Hash::from(blob_hash), merkle);
                match res {
                    Ok(needs_overwrite) => {
                        let () = responder.send(Ok(needs_overwrite)).unwrap();
                    }
                    Err(s) => {
                        let () = responder.send(Err(s.into_raw())).unwrap();
                    }
                }
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    /// Consume N directory requests, verifying they are intended to determine whether the blobs
    /// specified `readable` and `missing` are readable or not using `GetVmo`, responding to the
    /// check based on which collection the hash is in.
    ///
    /// # Panics
    ///
    /// Panics on error or assertion violation (unexpected requests, request for unspecified blob)
    pub async fn expect_readable_missing_checks(&mut self, readable: &[Hash], missing: &[Hash]) {
        let mut readable = readable.iter().copied().collect::<HashSet<_>>();
        let mut missing = missing.iter().copied().collect::<HashSet<_>>();

        while !(readable.is_empty() && missing.is_empty()) {
            match self.creator_stream.next().await {
                Some(Ok(ffxfs::BlobCreatorRequest::NeedsOverwrite { blob_hash, responder })) => {
                    let hash = Hash::from(blob_hash);
                    if readable.remove(&hash) {
                        let () = responder.send(Ok(false)).unwrap();
                    } else if missing.remove(&hash) {
                        let () = responder.send(Err(zx::Status::NOT_FOUND.into_raw())).unwrap();
                    } else {
                        panic!("Unexpected blob existance check for {hash}");
                    }
                }
                other => panic!("unexpected request: {other:?}"),
            }
        }
    }

    /// Expects and handles a call to [`Client::filter_to_missing_blobs`].
    /// Verifies the call intends to determine whether the blobs specified in `readable` and
    /// `missing` are readable or not, responding to the check based on which collection the hash is
    /// in.
    ///
    /// # Panics
    ///
    /// Panics on error or assertion violation (unexpected requests, request for unspecified blob)
    pub async fn expect_filter_to_missing_blobs_with_readable_missing_ids(
        &mut self,
        readable: &[Hash],
        missing: &[Hash],
    ) {
        self.expect_readable_missing_checks(readable, missing).await;
    }

    /// Asserts that the request stream closes without any further requests.
    ///
    /// # Panics
    ///
    /// Panics on error
    pub async fn expect_done(mut self) {
        match self.stream.next().await {
            None => {}
            Some(request) => panic!("unexpected request: {request:?}"),
        }
    }
}

/// A testing server implementation of fuchsia.fxfs/BlobWriter.
pub struct BlobWriter {
    stream: ffxfs::BlobWriterRequestStream,
    vmo: Option<zx::Vmo>,
}

impl BlobWriter {
    /// Asserts that the request stream closes without any further requests.
    ///
    /// # Panics
    ///
    /// Panics on error
    pub async fn expect_done(mut self) {
        match self.stream.next().await {
            None => {}
            Some(request) => panic!("unexpected request: {request:?}"),
        }
    }

    /// Asserts that GetVmo is called with the indicated size.
    ///
    /// # Panics
    ///
    /// Panics on error
    pub async fn expect_get_vmo(&mut self, expected_size: u64) -> &mut Self {
        match self.stream.next().await {
            Some(Ok(ffxfs::BlobWriterRequest::GetVmo { size, responder })) => {
                assert_eq!(expected_size, size);
                let vmo = zx::Vmo::create(expected_size).unwrap();
                assert!(self.vmo.is_none());
                self.vmo = Some(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap());
                let () = responder.send(Ok(vmo)).unwrap();
            }
            req => panic!("unexpected request {req:?}"),
        }
        self
    }

    /// Asserts that BytesWritten is called and responds with a data integrity error.
    ///
    /// # Panics
    ///
    /// Panics on error
    pub async fn fail_bytes_written(&mut self) -> &mut Self {
        match self.stream.next().await {
            Some(Ok(ffxfs::BlobWriterRequest::BytesReady { bytes_written: _, responder })) => {
                let () = responder.send(Err(zx::Status::IO_DATA_INTEGRITY.into_raw())).unwrap();
            }
            req => panic!("unexpected request {req:?}"),
        }
        self
    }

    /// Asserts that `content` is written to this freshly created `BlobWriter` in a single
    /// `BytesWritten`.
    ///
    /// # Panics
    ///
    /// Panics on error
    pub async fn expect_payload(mut self, content: &[u8]) {
        self.expect_get_vmo(content.len().try_into().unwrap()).await;
        match self.stream.next().await {
            Some(Ok(ffxfs::BlobWriterRequest::BytesReady { bytes_written, responder })) => {
                assert_eq!(bytes_written, u64::try_from(content.len()).unwrap());
                let vmo = self.vmo.unwrap();
                let mut buf = vec![0; content.len()];
                let () = vmo.read(&mut buf, 0).unwrap();
                assert_eq!(content, &buf[..content.len()]);
                let () = responder.send(Ok(())).unwrap();
            }
            req => panic!("unexpected request {req:?}"),
        }
    }
}
