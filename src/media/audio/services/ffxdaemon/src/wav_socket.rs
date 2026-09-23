// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Helper newtype for reading and writing a wav file to and from a socket.

use anyhow::{Error, anyhow};
use byteorder::ByteOrder;
use fuchsia_audio::Format;
use futures::{AsyncReadExt, AsyncWriteExt};
use std::io::Cursor;

pub struct WavSocket(pub fidl::AsyncSocket);

impl WavSocket {
    pub async fn write_header(
        &mut self,
        duration: Option<std::time::Duration>,
        format: Format,
    ) -> Result<(), Error> {
        let spec = hound::WavSpec::from(format);
        let header = match duration {
            Some(d) => format.wav_header_for_duration(d)?,
            None => spec.into_header_for_infinite_file(),
        };

        self.0
            .write_all(&header)
            .await
            .map_err(|e| anyhow!("Failed to write header to socket: {}", e))
    }

    pub async fn read_header(&mut self) -> Result<hound::WavSpec, Error> {
        // 12 bytes for the RIFF chunk descriptor.
        let mut riff_chunk_descriptor = vec![0u8; 12];
        self.0.read_exact(&mut riff_chunk_descriptor).await?;

        // fmt chunk ID and size are 4 bytes each.
        let mut fmt_subchunk_info = vec![0u8; 8];
        self.0.read_exact(&mut fmt_subchunk_info).await?;

        // Chunk size field is Little endian.
        let fmt_chunk_size = byteorder::LittleEndian::read_u32(&fmt_subchunk_info[4..]) as usize;

        // fmt subchunk can differ in length depending on the file format.
        let mut fmt_chunk = vec![0u8; fmt_chunk_size];
        self.0.read_exact(&mut fmt_chunk).await?;

        // data sub chunk ID and size are 4 bytes each.
        let mut data_subchunk_info = vec![0u8; 8];
        self.0.read_exact(&mut data_subchunk_info).await?;

        let wav_header =
            [riff_chunk_descriptor, fmt_subchunk_info, fmt_chunk, data_subchunk_info].concat();

        let cursor_header = Cursor::new(wav_header);
        let reader = hound::WavReader::new(cursor_header)?;

        Ok(reader.spec())
    }

    // Reads up to buffer size bytes from the socket. Similiar to `std::io::Read::read_exact()`,
    // except that if EOF is encountered before filling the buffer, we still preserve the partially
    // filled buffer and return how many bytes were read from the socket instead of returning an
    // error and leaving the buffer in an unspecified state.
    pub async fn read_until_full(&mut self, buffer: &mut Vec<u8>) -> Result<u64, Error> {
        let mut bytes_read_so_far = 0;

        loop {
            let bytes_read = self.0.read(&mut buffer[bytes_read_so_far..]).await?;
            bytes_read_so_far += bytes_read;

            if bytes_read == 0 || bytes_read_so_far == buffer.len() as usize {
                break;
            }
        }

        Ok(bytes_read_so_far as u64)
    }
}
