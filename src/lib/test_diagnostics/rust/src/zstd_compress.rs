// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::channel::mpsc;
use futures::SinkExt;
use std::cell::RefCell;
use thiserror::Error;
use zstd::stream::raw::Operation;

const BUFFER_SIZE: usize = 1024 * 1024 * 4; // 4 MB
const CHANNEL_SIZE: usize = 10; // 1 MB

thread_local! {
    static BUFFER: RefCell<Vec<u8>> = RefCell::new(vec![0; BUFFER_SIZE]);
}

/// Error encountered during compression or Decompression.
#[derive(Debug, Error)]
pub enum Error {
    /// Error Decompressing bytes.
    #[error("Error Decompressing bytes:  pos: {1}, len: {2}, error: {0:?}")]
    Decompress(#[source] std::io::Error, usize, usize),

    /// Error compressing bytes.
    #[error("Error compressing bytes:  pos: {1}, len: {2}, error: {0:?}")]
    Compress(#[source] std::io::Error, usize, usize),

    /// Error decompressing while flushing the decoder.
    #[error("Error Decompressing while flushing:  {0:?}")]
    DecompressFinish(#[source] std::io::Error),

    /// Error compressing while flushing the encoder.
    #[error("Error compressing while flushing:  {0:?}")]
    CompressFinish(#[source] std::io::Error),

    /// Error while sending data on the mpsc channel.
    #[error("Error while sending on mpsc channel:  {0:?}")]
    Send(#[source] mpsc::SendError),
}

/// A decoder that decompresses data using the Zstandard algorithm.
pub struct Decoder<'a> {
    sender: mpsc::Sender<Vec<u8>>,
    decoder: zstd::stream::raw::Decoder<'a>,
}

impl Decoder<'static> {
    /// Creates a new `Decoder` and returns a receiver for the decompressed data.
    ///
    /// The `Decoder` will decompress data in chunks and send the decompressed chunks
    /// over the returned `mpsc::Receiver`.
    pub fn new() -> (Self, mpsc::Receiver<Vec<u8>>) {
        let (sender, receiver) = mpsc::channel(CHANNEL_SIZE);
        let decoder = Self { sender: sender, decoder: zstd::stream::raw::Decoder::new().unwrap() };
        (decoder, receiver)
    }

    /// Decompresses the given bytes and sends the decompressed data over the channel.
    ///
    /// This method decompresses the input data in chunks, using a thread-local buffer
    /// to store the decompressed data. The decompressed chunks are then sent over
    /// the channel to the receiver.
    pub async fn decompress(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let len = bytes.len();
        let mut pos = 0;
        while pos != len {
            let decoded_bytes = BUFFER.with_borrow_mut(|buf| {
                let status = self
                    .decoder
                    .run_on_buffers(&bytes[pos..], buf.as_mut_slice())
                    .map_err(|e| Error::Decompress(e, pos, len))?;
                pos += status.bytes_read;
                Ok::<Vec<u8>, Error>(buf[..status.bytes_written].to_vec())
            })?;
            self.sender.send(decoded_bytes).await.map_err(Error::Send)?;
        }
        Ok(())
    }

    /// Flushes the decoder and sends any remaining decompressed data over the channel.
    ///
    /// This method should always be called after all input data has been decompressed to ensure
    /// that all decompressed data is sent to the receiver.
    pub async fn finish(mut self) -> Result<(), Error> {
        loop {
            let (remaining_bytes, decoded_bytes) = BUFFER.with_borrow_mut(|buf| {
                let mut out_buffer = zstd::stream::raw::OutBuffer::around(buf.as_mut_slice());
                let remaining_bytes =
                    self.decoder.flush(&mut out_buffer).map_err(Error::DecompressFinish)?;
                Ok::<(usize, Vec<u8>), Error>((remaining_bytes, out_buffer.as_slice().to_vec()))
            })?;
            if !decoded_bytes.is_empty() {
                self.sender.send(decoded_bytes).await.map_err(Error::Send)?;
            }
            if remaining_bytes == 0 {
                break;
            }
        }
        Ok(())
    }
}

/// An encoder that compresses data using the Zstandard algorithm.
pub struct Encoder<'a> {
    sender: mpsc::Sender<Vec<u8>>,
    encoder: zstd::stream::raw::Encoder<'a>,
}

impl Encoder<'static> {
    /// Creates a new `Encoder` with the given compression level and returns a receiver
    /// for the compressed data.
    ///
    /// The `Encoder` will compress data in chunks and send the compressed chunks
    /// over the returned `mpsc::Receiver`.
    pub fn new(level: i32) -> (Self, mpsc::Receiver<Vec<u8>>) {
        let (sender, receiver) = mpsc::channel(CHANNEL_SIZE);
        let decoder =
            Self { sender: sender, encoder: zstd::stream::raw::Encoder::new(level).unwrap() };
        (decoder, receiver)
    }

    /// Compresses the given bytes and sends the compressed data over the channel.
    ///
    /// This method compresses the input data in chunks, using a thread-local buffer
    /// to store the compressed data. The compressed chunks are then sent over
    /// the channel to the receiver.
    pub async fn compress(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let len = bytes.len();
        let mut pos = 0;
        while pos != len {
            let encoded_bytes = BUFFER.with_borrow_mut(|buf| {
                let status = self
                    .encoder
                    .run_on_buffers(&bytes[pos..], buf.as_mut_slice())
                    .map_err(|e| Error::Compress(e, pos, len))?;
                pos += status.bytes_read;
                Ok::<Vec<u8>, Error>(buf[..status.bytes_written].to_vec())
            })?;
            self.sender.send(encoded_bytes).await.map_err(Error::Send)?;
        }
        Ok(())
    }

    /// Flushes the encoder and sends any remaining compressed data over the channel.
    ///
    /// This method should be called after all input data has been compressed to ensure
    /// that all compressed data is sent to the receiver.
    pub async fn finish(mut self) -> Result<(), Error> {
        loop {
            let (remaining_bytes, encoded_bytes) = BUFFER.with_borrow_mut(|buf| {
                let mut out_buffer = zstd::stream::raw::OutBuffer::around(buf.as_mut_slice());
                let remaining_bytes =
                    self.encoder.finish(&mut out_buffer, true).map_err(Error::CompressFinish)?;
                Ok::<(usize, Vec<u8>), Error>((remaining_bytes, out_buffer.as_slice().to_vec()))
            })?;
            if !encoded_bytes.is_empty() {
                self.sender.send(encoded_bytes).await.map_err(Error::Send)?;
            }
            if remaining_bytes == 0 {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use futures::StreamExt;
    use test_case::test_case;

    #[test_case(Vec::from(b"This is a test string"); "normal test string")]
    #[test_case(Vec::from(b""); "empty string")]
    #[fuchsia::test]
    async fn test_compress_decompress(original_data: Vec<u8>) {
        let (mut encoder, mut rx) = Encoder::new(0);
        let (mut decoder, mut drx) = Decoder::new();

        // Compress the data
        encoder.compress(original_data.as_slice()).await.unwrap();
        encoder.finish().await.unwrap();

        // Receive the compressed data
        let mut compressed_data = Vec::new();
        while let Some(chunk) = rx.next().await {
            compressed_data.extend_from_slice(&chunk);
        }

        assert_ne!(compressed_data.len(), original_data.len());

        // Decompress the data
        decoder.decompress(&compressed_data).await.unwrap();
        decoder.finish().await.unwrap();

        // Receive the decompressed data
        let mut decompressed_data = Vec::new();
        while let Some(chunk) = drx.next().await {
            decompressed_data.extend_from_slice(&chunk);
        }

        // Assert that the decompressed data matches the original data
        assert_eq!(original_data.as_slice(), &decompressed_data[..]);
    }

    #[fuchsia::test]
    async fn test_compress_decompress_large_chunked() {
        let (mut encoder, mut rx) = Encoder::new(0);
        let (mut decoder, mut drx) = Decoder::new();

        let original_data = vec![b'a'; BUFFER_SIZE * 10 + 100];
        let chunk_size = 2 * 1024 * 1024; // 2 MB

        // Compress the data in chunks
        let compress_fut = async {
            for i in (0..original_data.len()).step_by(chunk_size) {
                encoder
                    .compress(&original_data[i..i + chunk_size.min(original_data.len() - i)])
                    .await
                    .unwrap();
            }
            encoder.finish().await.unwrap();
        };
        let mut compressed_len = 0;
        let decompress_fut = async {
            while let Some(compressed_chunk) = rx.next().await {
                compressed_len += compressed_chunk.len();
                decoder.decompress(&compressed_chunk).await.unwrap();
            }
            decoder.finish().await.unwrap();
        };

        let mut decompressed_data = Vec::new();
        let collect_final_data = async {
            // Receive decompressed chunks
            while let Some(chunk) = drx.next().await {
                decompressed_data.extend_from_slice(&chunk);
            }
        };

        futures::join!(compress_fut, decompress_fut, collect_final_data);

        assert!(compressed_len < original_data.len());
        assert_eq!(original_data, decompressed_data);
    }

    #[fuchsia::test]
    async fn test_compress_decompress_random_chunked() {
        let (mut encoder, mut rx) = Encoder::new(0);
        let (mut decoder, mut drx) = Decoder::new();

        let mut original_data = vec![0u8; BUFFER_SIZE * 5 + 100];
        rand::fill(&mut original_data[..]); // Fill with random data
        let chunk_size = 2 * 1024 * 1024; // 2 MB

        // Compress the data in chunks
        let compress_fut = async {
            for i in (0..original_data.len()).step_by(chunk_size) {
                encoder
                    .compress(&original_data[i..i + chunk_size.min(original_data.len() - i)])
                    .await
                    .unwrap();
            }
            encoder.finish().await.unwrap();
        };
        let mut compressed_len = 0;
        let decompress_fut = async {
            while let Some(compressed_chunk) = rx.next().await {
                compressed_len += compressed_chunk.len();
                decoder.decompress(&compressed_chunk).await.unwrap();
            }
            decoder.finish().await.unwrap();
        };

        let mut decompressed_data = Vec::new();
        let collect_final_data = async {
            // Receive decompressed chunks
            while let Some(chunk) = drx.next().await {
                decompressed_data.extend_from_slice(&chunk);
            }
        };

        futures::join!(compress_fut, decompress_fut, collect_final_data);

        assert_ne!(compressed_len, original_data.len());
        assert_eq!(original_data, decompressed_data);
    }

    #[fuchsia::test]
    async fn test_invalid_input() {
        let (mut decoder, _drx) = Decoder::new();

        let invalid_data = vec![0xff; 1024];

        let result = decoder.decompress(&invalid_data).await;

        assert_matches!(result, Err(Error::Decompress(..)));
    }

    #[fuchsia::test]
    async fn test_send_error() {
        let (mut encoder, rx) = Encoder::new(0);

        let data = b"some_text";
        drop(rx);

        let result = encoder.compress(data).await;

        assert_matches!(result, Err(Error::Send(..)));

        let (mut encoder, mut rx) = Encoder::new(0);
        let (mut decoder, drx) = Decoder::new();
        encoder.compress(data).await.unwrap();
        encoder.finish().await.unwrap();
        drop(drx);

        let mut compressed_data = Vec::new();
        while let Some(chunk) = rx.next().await {
            compressed_data.extend_from_slice(&chunk);
        }

        let result = decoder.decompress(&compressed_data).await;
        assert_matches!(result, Err(Error::Send(..)));
    }
}
