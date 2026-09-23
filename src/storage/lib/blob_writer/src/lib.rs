// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_fxfs::BlobWriterProxy;

use futures::future::BoxFuture;
use futures::stream::{FuturesOrdered, StreamExt as _, TryStreamExt as _};

mod errors;
pub use errors::{CreateError, WriteError};

/// BlobWriter is a wrapper around the fuchsia.fxfs.BlobWriter fidl protocol. Clients will use this
/// library to write blobs to disk.
#[derive(Debug)]
pub struct BlobWriter {
    blob_writer_proxy: BlobWriterProxy,
    vmo: zx::Vmo,
    // Ordered queue of BytesReady requests. There are at most 2 outstanding requests on the
    // queue at any point in time. Each BytesReady request takes up at most half the ring
    // buffer (N).
    //
    // Our goal is to be constantly moving bytes out of the network and into storage without ever
    // having to wait for a fidl roundtrip. Maintaining 2 outstanding requests on the queue allows
    // us to pipeline requests, so that the server can respond to one request while the client is
    // creating another. Limiting the size of any particular request to N/2 allows each of the
    // two requests on the queue to be as big as they possibly can, which is particularly important
    // when storage is the limiting factor. Namely, we want to avoid situations where the server
    // has completed a small request and has to wait on a fidl roundtrip (i.e. has to wait for the
    // network to receive the response, create a new request, and send the request back).
    outstanding_writes:
        FuturesOrdered<BoxFuture<'static, Result<Result<u64, zx::Status>, fidl::Error>>>,
    // Number of bytes that have been written to the vmo, both acknowledged and unacknowledged.
    bytes_sent: u64,
    // Number of available bytes in the vmo (the size of the vmo minus the size of unacknowledged
    // writes).
    available: u64,
    // Size of the blob being written.
    blob_len: u64,
    // Size of the vmo.
    vmo_len: u64,
}

impl BlobWriter {
    /// Creates a `BlobWriter`.  Exactly `size` bytes are expected to be written into the writer.
    pub async fn create(
        blob_writer_proxy: BlobWriterProxy,
        size: u64,
    ) -> Result<Self, CreateError> {
        let vmo = blob_writer_proxy
            .get_vmo(size)
            .await
            .map_err(CreateError::Fidl)?
            .map_err(zx::Status::err_from_raw)
            .map_err(CreateError::GetVmo)?;
        let vmo_len = vmo.get_size().map_err(CreateError::GetSize)?;
        Ok(BlobWriter {
            blob_writer_proxy,
            vmo,
            outstanding_writes: FuturesOrdered::new(),
            bytes_sent: 0,
            available: vmo_len,
            blob_len: size,
            vmo_len,
        })
    }

    /// Begins writing `bytes` to the server.
    ///
    /// If `bytes` contains all of the remaining unwritten bytes of the blob, i.e. the sum of the
    /// lengths of the `bytes` slices from this and all prior calls to `write` is equal to the size
    /// given to `create`, then the returned Future will not complete until all of the writes have
    /// been acknowledged by the server and the blob can be opened for read.
    /// Otherwise, the returned Future may complete before the write of `bytes` has been
    /// acknowledged by the server.
    ///
    /// Returns an error if the length of `bytes` exceeds the remaining available space in the
    /// blob, calculated as per `size`.
    pub async fn write(&mut self, mut bytes: &[u8]) -> Result<(), WriteError> {
        if self.bytes_sent + bytes.len() as u64 > self.blob_len {
            return Err(WriteError::EndOfBlob);
        }
        while !bytes.is_empty() {
            debug_assert!(self.outstanding_writes.len() <= 2);
            // Wait until there is room in the vmo and fewer than 2 outstanding writes.
            if self.available == 0 || self.outstanding_writes.len() == 2 {
                let bytes_ackd = self
                    .outstanding_writes
                    .next()
                    .await
                    .ok_or_else(|| WriteError::QueueEnded)?
                    .map_err(WriteError::Fidl)?
                    .map_err(WriteError::BytesReady)?;
                self.available += bytes_ackd;
            }

            let bytes_to_send_len = {
                let mut bytes_to_send_len = std::cmp::min(self.available, bytes.len() as u64);
                // If all the remaining bytes do not fit in the vmo, split writes to prevent
                // blocking the server on an ack roundtrip.
                if self.blob_len - self.bytes_sent > self.vmo_len {
                    bytes_to_send_len = std::cmp::min(bytes_to_send_len, self.vmo_len / 2)
                }
                bytes_to_send_len
            };

            let (bytes_to_send, remaining_bytes) = bytes.split_at(bytes_to_send_len as usize);
            bytes = remaining_bytes;

            let vmo_index = self.bytes_sent % self.vmo_len;
            let (bytes_to_send_before_wrap, bytes_to_send_after_wrap) = bytes_to_send
                .split_at(std::cmp::min((self.vmo_len - vmo_index) as usize, bytes_to_send.len()));

            self.vmo.write(bytes_to_send_before_wrap, vmo_index).map_err(WriteError::VmoWrite)?;
            if !bytes_to_send_after_wrap.is_empty() {
                self.vmo.write(bytes_to_send_after_wrap, 0).map_err(WriteError::VmoWrite)?;
            }

            let write_fut = self.blob_writer_proxy.bytes_ready(bytes_to_send_len);
            self.outstanding_writes.push_back(Box::pin(async move {
                write_fut
                    .await
                    .map(|res| res.map(|()| bytes_to_send_len).map_err(zx::Status::err_from_raw))
            }));
            self.available -= bytes_to_send_len;
            self.bytes_sent += bytes_to_send_len;
        }
        debug_assert!(self.bytes_sent <= self.blob_len);

        // The last write call should not complete until the blob is completely written.
        if self.bytes_sent == self.blob_len {
            while let Some(result) =
                self.outstanding_writes.try_next().await.map_err(WriteError::Fidl)?
            {
                match result {
                    Ok(bytes_ackd) => self.available += bytes_ackd,
                    Err(e) => return Err(WriteError::BytesReady(e)),
                }
            }
            // This should not be possible.
            if self.available != self.vmo_len {
                return Err(WriteError::EndOfBlob);
            }
        }
        Ok(())
    }

    pub fn vmo_size(&self) -> u64 {
        self.vmo_len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_fxfs::{BlobWriterMarker, BlobWriterRequest};
    use fuchsia_sync::Mutex;
    use futures::{FutureExt, pin_mut, select};
    use std::sync::Arc;

    const VMO_SIZE: usize = 4096;

    async fn check_blob_writer(
        write_fun: impl AsyncFnOnce(BlobWriterProxy, &[u8]),
        data: &[u8],
        writes: &[(usize, usize)],
    ) {
        let (proxy, mut stream) = create_proxy_and_stream::<BlobWriterMarker>();
        let count = Arc::new(Mutex::new(0));
        let count_clone = count.clone();
        let expected_count = writes.len();
        let mut check_vmo = None;
        let mock_server = async move {
            while let Some(request) = stream.next().await {
                match request {
                    Ok(BlobWriterRequest::GetVmo { responder, .. }) => {
                        let vmo = zx::Vmo::create(VMO_SIZE as u64).expect("failed to create vmo");
                        let vmo_dup = vmo
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("failed to duplicate VMO");
                        check_vmo = Some(vmo);
                        responder.send(Ok(vmo_dup)).unwrap();
                    }
                    Ok(BlobWriterRequest::BytesReady { responder, bytes_written, .. }) => {
                        let vmo = check_vmo.as_ref().unwrap();
                        let mut count_locked = count.lock();
                        let mut buf = vec![0; bytes_written as usize];
                        let data_range = writes[*count_locked];
                        let vmo_offset = data_range.0 % VMO_SIZE;
                        if vmo_offset + bytes_written as usize > VMO_SIZE {
                            let split = VMO_SIZE - vmo_offset;
                            vmo.read(&mut buf[0..split], vmo_offset as u64).unwrap();
                            vmo.read(&mut buf[split..], 0).unwrap();
                        } else {
                            vmo.read(&mut buf, vmo_offset as u64).unwrap();
                        }
                        assert_eq!(bytes_written, (data_range.1 - data_range.0) as u64);
                        assert_eq!(&data[data_range.0..data_range.1], buf);
                        *count_locked += 1;
                        responder.send(Ok(())).unwrap();
                    }
                    _ => {
                        unreachable!()
                    }
                }
            }
        }
        .fuse();

        pin_mut!(mock_server);

        select! {
            _ = mock_server => unreachable!(),
            _ = write_fun(proxy, data).fuse() => {
                assert_eq!(*count_clone.lock(), expected_count);
            }
        }
    }

    #[fuchsia::test]
    async fn invalid_write_past_end_of_blob() {
        let mut data = [0; VMO_SIZE];
        rand::fill(&mut data[..]);

        let write_fun = async move |proxy: BlobWriterProxy, data: &[u8]| {
            let mut blob_writer = BlobWriter::create(proxy, data.len() as u64)
                .await
                .expect("failed to create BlobWriter");
            let () = blob_writer.write(&data).await.unwrap();
            let invalid_write = [0; 4096];
            assert_matches!(blob_writer.write(&invalid_write).await, Err(WriteError::EndOfBlob));
        };

        check_blob_writer(write_fun, &data, &[(0, VMO_SIZE)]).await;
    }

    #[fuchsia::test]
    async fn do_not_split_writes_if_blob_fits_in_vmo() {
        let mut data = vec![0; VMO_SIZE - 1];
        rand::fill(&mut data[..]);

        let write_fun = async move |proxy: BlobWriterProxy, data: &[u8]| {
            let mut blob_writer = BlobWriter::create(proxy, data.len() as u64)
                .await
                .expect("failed to create BlobWriter");
            let () = blob_writer.write(&data[..]).await.unwrap();
        };

        check_blob_writer(write_fun, &data, &[(0, 4095)]).await;
    }

    #[fuchsia::test]
    async fn split_writes_if_blob_does_not_fit_in_vmo() {
        let mut data = vec![0; VMO_SIZE + 1];
        rand::fill(&mut data[..]);

        let write_fun = async move |proxy: BlobWriterProxy, data: &[u8]| {
            let mut blob_writer = BlobWriter::create(proxy, data.len() as u64)
                .await
                .expect("failed to create BlobWriter");
            let () = blob_writer.write(&data[..]).await.unwrap();
        };

        check_blob_writer(write_fun, &data, &[(0, 2048), (2048, 4096), (4096, 4097)]).await;
    }

    #[fuchsia::test]
    async fn third_write_wraps() {
        let mut data = vec![0; 1024 * 6];
        rand::fill(&mut data[..]);

        let writes =
            [(0, 1024 * 2), (1024 * 2, 1024 * 3), (1024 * 3, 1024 * 5), (1024 * 5, 1024 * 6)];

        let write_fun = async move |proxy: BlobWriterProxy, data: &[u8]| {
            let mut blob_writer = BlobWriter::create(proxy, data.len() as u64)
                .await
                .expect("failed to create BlobWriter");
            for (i, j) in writes {
                let () = blob_writer.write(&data[i..j]).await.unwrap();
            }
        };

        check_blob_writer(write_fun, &data, &writes).await;
    }

    #[fuchsia::test]
    async fn many_wraps() {
        let mut data = vec![0; VMO_SIZE * 3];
        rand::fill(&mut data[..]);

        let write_fun = async move |proxy: BlobWriterProxy, data: &[u8]| {
            let mut blob_writer = BlobWriter::create(proxy, data.len() as u64)
                .await
                .expect("failed to create BlobWriter");
            let () = blob_writer.write(&data[0..1]).await.unwrap();
            let () = blob_writer.write(&data[1..]).await.unwrap();
        };

        check_blob_writer(
            write_fun,
            &data,
            &[
                (0, 1),
                (1, 2049),
                (2049, 4097),
                (4097, 6145),
                (6145, 8193),
                (8193, 10241),
                (10241, 12288),
            ],
        )
        .await;
    }
}
