// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::LogError;
use diagnostics_data::{ExtendedMoniker, LogsData};
use diagnostics_log_encoding::{Header, LOG_CONTROL_BIT, MONIKER, URL, Value};
use diagnostics_message::MonikerWithUrl;
use flyweights::FlyStr;
use futures_util::stream::FusedStream;
use futures_util::{AsyncReadExt, stream};
use std::collections::HashMap;
use zerocopy::FromBytes;

pub struct FxtStreamer {
    socket_reader: SocketReader,
    tags: HashMap<u32, MonikerWithUrl>,
}

impl FxtStreamer {
    pub fn new(socket: flex_client::AsyncSocket) -> Self {
        Self { socket_reader: SocketReader::new(socket), tags: HashMap::default() }
    }

    pub fn stream(self) -> impl FusedStream<Item = Result<LogsData, LogError>> {
        stream::unfold(Some(self), |maybe_this| async {
            let mut this = maybe_this?;
            let item = this.next_message().await.transpose()?;
            let maybe_this = item.is_ok().then_some(this);
            Some((item, maybe_this))
        })
    }

    async fn next_message(&mut self) -> Result<Option<LogsData>, LogError> {
        loop {
            // 1. Ensure we have at least 8 bytes for the header.
            let Some(header_bytes) = self.socket_reader.fill(8).await? else {
                return Ok(None);
            };

            // 2. Parse header to get record size.
            let header = Header::read_from_bytes(header_bytes)
                .map_err(|_| LogError::UnknownError(anyhow::anyhow!("Failed to parse header")))?;
            let record_len = header.size_words() as usize * 8;
            if record_len < 8 {
                Err(LogError::UnknownError(anyhow::anyhow!("Invalid record size in header")))?;
            }

            // 3. Ensure we have the full record.
            let record_bytes = self.socket_reader.fill(record_len).await?.unwrap();

            // 4. Parse the record.
            let (record, remaining) = diagnostics_log_encoding::parse::parse_record(record_bytes)
                .map_err(|e| LogError::UnknownError(e.into()))?;
            debug_assert!(remaining.is_empty());

            // 5. Handle Manifest vs Log
            let tag = header.tag();
            let is_manifest = (tag & LOG_CONTROL_BIT) != 0;
            let actual_tag = tag & !LOG_CONTROL_BIT;

            if is_manifest {
                let mut moniker = None;
                let mut url = None;
                for arg in &record.arguments {
                    if arg.name() == MONIKER
                        && let Value::Text(t) = arg.value()
                    {
                        moniker = Some(
                            ExtendedMoniker::parse_str(&t)
                                .map_err(|e| LogError::UnknownError(e.into()))?,
                        );
                    } else if arg.name() == URL
                        && let Value::Text(t) = arg.value()
                    {
                        url = Some(FlyStr::new(t));
                    }
                }
                if let (Some(moniker), Some(url)) = (moniker, url) {
                    self.tags.insert(actual_tag, MonikerWithUrl { moniker, url });
                }
                self.socket_reader.consume(record_len);
                continue;
            }

            let source = self.tags.get(&actual_tag).cloned().unwrap_or_else(|| MonikerWithUrl {
                moniker: ExtendedMoniker::parse_str("/UNKNOWN").unwrap(),
                url: FlyStr::new(""),
            });
            let logs_data = diagnostics_message::from_structured(source, record_bytes)
                .map_err(|e| LogError::UnknownError(e.into()))?;

            self.socket_reader.consume(record_len);
            return Ok(Some(logs_data));
        }
    }
}

struct SocketReader {
    socket: flex_client::AsyncSocket,
    buffer: Vec<u8>,
    head: usize,
}

impl SocketReader {
    fn new(socket: flex_client::AsyncSocket) -> Self {
        Self { socket, buffer: Vec::new(), head: 0 }
    }

    /// Ensures that `count` bytes are present in the buffer and returns them.  Returns
    /// None, if the socket is closed and there is no partial data.
    async fn fill(&mut self, count: usize) -> Result<Option<&[u8]>, LogError> {
        let remaining = self.buffer.len() - self.head;
        if remaining < count {
            if self.head > 0 {
                self.buffer.copy_within(self.head.., 0);
                self.buffer.truncate(remaining);
                self.head = 0;
            }
            while self.buffer.len() < count {
                let start_len = self.buffer.len();
                self.buffer.resize(std::cmp::max(start_len + 4096, count), 0);
                let n = self
                    .socket
                    .read(&mut self.buffer[start_len..])
                    .await
                    .map_err(LogError::IOError)?;
                self.buffer.truncate(start_len + n);
                if n == 0 {
                    if !self.buffer.is_empty() {
                        return Err(LogError::UnknownError(anyhow::anyhow!(
                            "Truncated record at end of stream"
                        )));
                    }
                    return Ok(None);
                }
            }
        }
        Ok(Some(&self.buffer[self.head..self.head + count]))
    }

    /// Consumes `count` bytes from the buffer.
    fn consume(&mut self, count: usize) {
        self.head += count;
        if self.head == self.buffer.len() {
            self.buffer.clear();
            self.head = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use diagnostics_log_encoding::encode::{Encoder, EncoderOpts, ResizableBuffer};
    use diagnostics_log_encoding::{Argument, Record};
    use fuchsia_async as fasync;
    use futures_util::StreamExt;
    use zerocopy::IntoBytes;

    fn fn_encode(record: Record<'_>, tag: u32) -> Vec<u8> {
        let mut encoder = Encoder::new(
            std::io::Cursor::new(ResizableBuffer::from(Vec::new())),
            EncoderOpts::default(),
        );
        encoder.write_record(record).unwrap();
        let mut bytes = encoder.take().into_inner().into_inner();
        let mut header = Header::read_from_bytes(&bytes[0..8]).unwrap();
        header.set_tag(tag);
        bytes[0..8].copy_from_slice(header.as_bytes());
        bytes
    }

    // --- 1. Exercise SocketReader directly ---
    #[fuchsia::test]
    async fn test_socket_reader_clean_eof() {
        let (sender, receiver) = zx::Socket::create_stream();
        let mut reader = SocketReader::new(flex_client::socket_to_async(receiver));
        drop(sender);
        assert!(reader.fill(8).await.unwrap().is_none());
    }

    #[fuchsia::test]
    async fn test_socket_reader_truncated() {
        let (sender, receiver) = zx::Socket::create_stream();
        let mut reader = SocketReader::new(flex_client::socket_to_async(receiver));
        sender.write(b"1234").unwrap();
        drop(sender);
        assert_matches!(
            reader.fill(8).await,
            Err(LogError::UnknownError(e))
                if e.to_string().contains("Truncated record at end of stream")
        );
    }

    #[fuchsia::test]
    async fn test_socket_reader_shift_and_consume() {
        let (sender, receiver) = zx::Socket::create_stream();
        let mut reader = SocketReader::new(flex_client::socket_to_async(receiver));

        // Write 16 bytes
        sender.write(b"abcdefghijklmnop").unwrap();
        drop(sender);

        // Fill 8 bytes
        let slice1 = reader.fill(8).await.unwrap().unwrap();
        assert_eq!(slice1, b"abcdefgh");
        reader.consume(4); // head becomes 4, remaining is 12

        // Fill 8 bytes starting from head 4 ("efghijkl")
        // Since remaining 12 is >= 8, this returns directly without shifting!
        let slice2 = reader.fill(8).await.unwrap().unwrap();
        assert_eq!(slice2, b"efghijkl");
        reader.consume(6); // head becomes 10, remaining is 6

        // Fill 8 bytes starting from head 10 ("klmnop")
        // Remaining is 6, which is < 8! This triggers copy_within shifting to head 0!
        // But since sender is dropped, reading more returns EOF, so we get Truncated error!
        assert_matches!(
            reader.fill(8).await,
            Err(LogError::UnknownError(e))
                if e.to_string().contains("Truncated record at end of stream") => {}
        );
    }

    #[fuchsia::test]
    async fn test_socket_reader_partial_reads_loop() {
        let (sender, receiver) = zx::Socket::create_stream();
        let mut reader = SocketReader::new(flex_client::socket_to_async(receiver));

        // Write 4 bytes
        sender.write(b"1234").unwrap();

        // Use a separate task to push more bytes so reader.fill can loop asynchronously
        let push_task = fasync::Task::spawn(async move {
            fasync::Timer::new(std::time::Duration::from_millis(10)).await;
            sender.write(b"5678").unwrap();
        });

        let slice = reader.fill(8).await.unwrap().unwrap();
        assert_eq!(slice, b"12345678");
        push_task.await;
    }

    // --- 2. Exercise FxtStreamer error branches ---
    #[fuchsia::test]
    async fn test_fxt_streamer_invalid_record_size() {
        let (sender, receiver) = zx::Socket::create_stream();
        let streamer = FxtStreamer::new(flex_client::socket_to_async(receiver));
        let mut stream = std::pin::pin!(streamer.stream());

        // Write 8 bytes of zeroes. Header size_words will be 0. record_len = 0 < 8.
        sender.write(&[0; 8]).unwrap();
        drop(sender);

        assert_matches!(
            stream.next().await,
            Some(Err(LogError::UnknownError(e)))
                if e.to_string().contains("Invalid record size in header")
        );
    }

    #[fuchsia::test]
    async fn test_fxt_streamer_truncated_record() {
        let (sender, receiver) = zx::Socket::create_stream();
        let streamer = FxtStreamer::new(flex_client::socket_to_async(receiver));
        let mut stream = std::pin::pin!(streamer.stream());

        // Build a valid 16-byte record
        let valid_16_byte_record = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(0),
                severity: 0x30,
                arguments: vec![Argument::message("1234")],
            },
            1,
        );

        // Write only the first 8 bytes!
        sender.write(&valid_16_byte_record[0..8]).unwrap();
        drop(sender);

        assert_matches!(
            stream.next().await,
            Some(Err(LogError::UnknownError(e)))
                if e.to_string().contains("Truncated record at end of stream")
        );
    }

    #[fuchsia::test]
    async fn test_fxt_streamer_parse_error() {
        let (sender, receiver) = zx::Socket::create_stream();
        let streamer = FxtStreamer::new(flex_client::socket_to_async(receiver));
        let mut stream = std::pin::pin!(streamer.stream());

        // Build a valid 16-byte record
        let valid_16_byte_record = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(0),
                severity: 0x30,
                arguments: vec![Argument::message("1234")],
            },
            1,
        );

        // Write the 8 byte header claiming 16 bytes, then provide 8 bytes of garbage payload.
        sender.write(&valid_16_byte_record[0..8]).unwrap();
        sender.write(&[0xff; 8]).unwrap(); // Invalid record encoding
        drop(sender);

        assert_matches!(stream.next().await, Some(Err(LogError::UnknownError(_))));
    }

    // --- 3. Exercise FxtStreamer non-error branches & continuous processing ---
    #[fuchsia::test]
    async fn test_fxt_streamer_unknown_tag() {
        let (sender, receiver) = zx::Socket::create_stream();
        let streamer = FxtStreamer::new(flex_client::socket_to_async(receiver));
        let mut stream = std::pin::pin!(streamer.stream());

        let log_bytes = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(123),
                severity: 0x30,
                arguments: vec![Argument::message("hello")],
            },
            1,
        );
        sender.write(&log_bytes).unwrap();
        drop(sender);

        let item = stream.next().await.unwrap().unwrap();
        assert_eq!(item.moniker.to_string(), "UNKNOWN");
        assert_eq!(item.metadata.component_url, Some(FlyStr::new("")));
        assert_eq!(item.msg().unwrap(), "hello");
    }

    #[fuchsia::test]
    async fn test_fxt_streamer_partial_manifest_ignored() {
        let (sender, receiver) = zx::Socket::create_stream();
        let streamer = FxtStreamer::new(flex_client::socket_to_async(receiver));
        let mut stream = std::pin::pin!(streamer.stream());

        // Write a manifest missing the URL argument
        let manifest_bytes = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(0),
                severity: 0x30,
                arguments: vec![Argument::other(MONIKER, "core/partial")],
            },
            1 | LOG_CONTROL_BIT,
        );

        let log_bytes = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(123),
                severity: 0x30,
                arguments: vec![Argument::message("hello")],
            },
            1,
        );

        sender.write(&manifest_bytes).unwrap();
        sender.write(&log_bytes).unwrap();
        drop(sender);

        // Manifest should be cleanly consumed and ignored. Log record falls back to /UNKNOWN.
        let item = stream.next().await.unwrap().unwrap();
        assert_eq!(item.moniker.to_string(), "UNKNOWN");
        assert_eq!(item.metadata.component_url, Some(FlyStr::new("")));
        assert_eq!(item.msg().unwrap(), "hello");
    }

    #[fuchsia::test]
    async fn test_fxt_streamer_multiple_messages() {
        let (sender, receiver) = zx::Socket::create_stream();
        let streamer = FxtStreamer::new(flex_client::socket_to_async(receiver));
        let mut stream = std::pin::pin!(streamer.stream());

        let manifest_a = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(0),
                severity: 0x30,
                arguments: vec![Argument::other(MONIKER, "core/a"), Argument::other(URL, "url_a")],
            },
            1 | LOG_CONTROL_BIT,
        );

        let log_a1 = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(1),
                severity: 0x30,
                arguments: vec![Argument::message("msg a1")],
            },
            1,
        );

        let log_a2 = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(2),
                severity: 0x30,
                arguments: vec![Argument::message("msg a2")],
            },
            1,
        );

        let log_unknown = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(3),
                severity: 0x30,
                arguments: vec![Argument::message("msg unknown")],
            },
            99, // Unmapped tag
        );

        sender.write(&manifest_a).unwrap();
        sender.write(&log_a1).unwrap();
        sender.write(&log_a2).unwrap();
        sender.write(&log_unknown).unwrap();
        drop(sender);

        let item1 = stream.next().await.unwrap().unwrap();
        assert_eq!(item1.moniker.to_string(), "core/a");
        assert_eq!(item1.msg().unwrap(), "msg a1");

        let item2 = stream.next().await.unwrap().unwrap();
        assert_eq!(item2.moniker.to_string(), "core/a");
        assert_eq!(item2.msg().unwrap(), "msg a2");

        let item3 = stream.next().await.unwrap().unwrap();
        assert_eq!(item3.moniker.to_string(), "UNKNOWN");
        assert_eq!(item3.msg().unwrap(), "msg unknown");

        assert!(stream.next().await.is_none());
    }
}
