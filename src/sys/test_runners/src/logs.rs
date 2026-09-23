// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Helpers for capturing logs from Fuchsia processes.

use fuchsia_async as fasync;
use futures::{AsyncReadExt as _, AsyncWriteExt as _, FutureExt as _, future};
use std::num::NonZeroUsize;
use thiserror::Error;

/// Buffer size for socket read calls to `LoggerStream::buffer_and_drain`.
const SOCKET_BUFFER_SIZE: usize = 2048;

/// Maximum length we will buffer for a single line. If a line is longer than this
/// length it will be split up into multiple messages.
const MAX_LINE_BUFFER_LENGTH: usize = 4096;

/// Error returned by this library.
#[derive(Debug, PartialEq, Eq, Error, Clone)]
pub enum LoggerError {
    #[error("cannot create socket: {:?}", _0)]
    CreateSocket(zx::Status),

    #[error("cannot duplicate socket: {:?}", _0)]
    DuplicateSocket(zx::Status),

    #[error("invalid socket: {:?}", _0)]
    InvalidSocket(zx::Status),
}

/// Error returned from draining LoggerStream or writing to LogWriter.
#[derive(Debug, Error)]
pub enum LogError {
    /// Error encountered when draining LoggerStream.
    #[error("can't get logs: {:?}", _0)]
    Read(std::io::Error),

    /// Error encountered when writing to LogWriter.
    #[error("can't write logs: {:?}", _0)]
    Write(std::io::Error),
}

/// Creates a combined socket handle for stdout and stderr and hooks them to same socket.
/// It also wraps the socket into stream and returns it back.
pub fn create_std_combined_log_stream()
-> Result<(LoggerStream, zx::NullableHandle, zx::NullableHandle), LoggerError> {
    let (client, log) = zx::Socket::create_stream();

    let stream = LoggerStream::new(client).map_err(LoggerError::InvalidSocket)?;
    let clone =
        log.duplicate_handle(zx::Rights::SAME_RIGHTS).map_err(LoggerError::DuplicateSocket)?;

    Ok((stream, log.into_handle(), clone.into_handle()))
}

/// Creates a socket handle for stdout/stderr and hooks it to a file handle.
/// It also wraps the socket into stream and returns it back.
pub fn create_log_stream() -> Result<(LoggerStream, zx::NullableHandle), LoggerError> {
    let (client, log) = zx::Socket::create_stream();

    let stream = LoggerStream::new(client).map_err(LoggerError::InvalidSocket)?;

    Ok((stream, log.into_handle()))
}
/// Collects logs in background and gives a way to collect those logs.
pub struct LogStreamReader {
    fut: future::RemoteHandle<Result<Vec<u8>, LogError>>,
}

impl LogStreamReader {
    pub fn new(logger: LoggerStream) -> Self {
        let (logger_handle, logger_fut) = logger.read_to_end().remote_handle();
        fasync::Task::spawn(logger_handle).detach();
        Self { fut: logger_fut }
    }

    /// Retrieve all logs.
    pub async fn get_logs(self) -> Result<Vec<u8>, LogError> {
        self.fut.await
    }
}

/// A stream bound to a socket where a source stream is captured.
/// For example, stdout and stderr streams can be redirected to the contained
/// socket and captured.
pub struct LoggerStream {
    socket: fasync::Socket,
}

impl Unpin for LoggerStream {}

impl LoggerStream {
    /// Create a LoggerStream from the provided zx::Socket. The `socket` object
    /// should be bound to its intended source stream (e.g. "stdout").
    pub fn new(socket: zx::Socket) -> Result<LoggerStream, zx::Status> {
        let l = LoggerStream { socket: fasync::Socket::from_socket(socket) };
        Ok(l)
    }

    /// Reads all bytes from socket.
    pub async fn read_to_end(mut self) -> Result<Vec<u8>, LogError> {
        let mut buffer: Vec<u8> = Vec::new();
        let _bytes_read = self.socket.read_to_end(&mut buffer).await.map_err(LogError::Read)?;
        Ok(buffer)
    }

    /// Drain the `stream` and write all of its contents to `writer`. Bytes are
    /// delimited by newline and each line will be passed to `writer.write` individually.
    /// An optional `peek_fn` may be specified which is passed a reference to each line before
    /// it is written.
    pub async fn buffer_drain_and_peek(
        mut self,
        writer: &mut SocketLogWriter,
        peek_fn: Option<impl Fn(&[u8])>,
    ) -> Result<(), LogError> {
        let mut line_buffer: Vec<u8> = Vec::with_capacity(MAX_LINE_BUFFER_LENGTH);
        let mut socket_buffer: Vec<u8> = vec![0; SOCKET_BUFFER_SIZE];

        while let Some(bytes_read) = NonZeroUsize::new(
            self.socket.read(&mut socket_buffer[..]).await.map_err(LogError::Read)?,
        ) {
            let bytes_read = bytes_read.get();

            let newline_iter = socket_buffer[..bytes_read]
                .iter()
                .enumerate()
                .filter_map(|(i, &b)| if b == b'\n' { Some(i) } else { None });

            let mut prev_offset = 0;
            for idx in newline_iter {
                let line = &socket_buffer[prev_offset..idx + 1];
                if !line_buffer.is_empty() {
                    writer.write(line_buffer.drain(..).as_slice()).await?;
                }
                if let Some(peek) = &peek_fn {
                    peek(line);
                }
                writer.write(line).await?;
                prev_offset = idx + 1;
            }
            if prev_offset != bytes_read {
                line_buffer.extend_from_slice(&socket_buffer[prev_offset..bytes_read]);
            }

            if line_buffer.len() > MAX_LINE_BUFFER_LENGTH {
                let bytes = &line_buffer[..MAX_LINE_BUFFER_LENGTH];
                if let Some(peek) = &peek_fn {
                    peek(bytes);
                }
                writer.write(bytes).await?;
                line_buffer.drain(..MAX_LINE_BUFFER_LENGTH);
            }
        }

        if !line_buffer.is_empty() {
            let bytes = &line_buffer[..];
            if let Some(peek) = &peek_fn {
                peek(bytes);
            }
            writer.write(bytes).await?;
        }

        Ok(())
    }

    /// Convenience function for buffer_drain_and_peek without a peek function.
    pub async fn buffer_and_drain(self, writer: &mut SocketLogWriter) -> Result<(), LogError> {
        self.buffer_drain_and_peek(writer, None::<fn(&[u8])>).await
    }

    /// Take the underlying socket of this object.
    pub fn take_socket(self) -> fasync::Socket {
        self.socket
    }
}

/// Utility struct to write to socket asynchrously.
pub struct SocketLogWriter {
    logger: fasync::Socket,
}

impl SocketLogWriter {
    pub fn new(logger: fasync::Socket) -> Self {
        Self { logger }
    }

    pub async fn write_str(&mut self, s: &str) -> Result<(), LogError> {
        self.write(s.as_bytes()).await
    }

    pub async fn write(&mut self, bytes: &[u8]) -> Result<(), LogError> {
        self.logger.write_all(bytes).await.map_err(LogError::Write)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context as _, Error, format_err};
    use assert_matches::assert_matches;
    use futures::{TryStreamExt as _, try_join};
    use rand::distr::{Alphanumeric, SampleString as _};
    use rand::rng;
    use std::sync::mpsc;
    use test_case::test_case;

    #[fuchsia::test]
    async fn log_writer_reader_work() {
        let (sock1, sock2) = zx::Socket::create_stream();
        let mut log_writer = SocketLogWriter::new(fasync::Socket::from_socket(sock1));

        let reader = LoggerStream::new(sock2).unwrap();
        let reader = LogStreamReader::new(reader);

        log_writer.write_str("this is string one.").await.unwrap();
        log_writer.write_str("this is string two.").await.unwrap();
        drop(log_writer);

        let actual = reader.get_logs().await.unwrap();
        let actual = std::str::from_utf8(&actual).unwrap();
        assert_eq!(actual, "this is string one.this is string two.".to_owned());
    }

    #[test_case(String::from("Hello World!") ; "consumes_simple_msg")]
    #[test_case(get_random_string(10000) ; "consumes_large_msg")]
    #[fuchsia::test]
    async fn logger_stream_read_to_end(msg: String) -> Result<(), Error> {
        let (stream, tx) = create_logger_stream()?;

        let () = take_and_write_to_socket(tx, &msg)?;
        let result = stream.read_to_end().await.context("Failed to read from socket")?;
        let actual = std::str::from_utf8(&result).context("Failed to parse bytes")?.to_owned();

        assert_eq!(actual, msg);
        Ok(())
    }

    #[fuchsia::test]
    async fn logger_stream_read_to_end_consumes_concat_msgs() -> Result<(), Error> {
        let (stream, tx) = create_logger_stream()?;
        let msgs =
            vec!["Hello World!".to_owned(), "Hola Mundo!".to_owned(), "你好，世界!".to_owned()];

        for msg in msgs.iter() {
            let () = write_to_socket(&tx, &msg)?;
        }
        std::mem::drop(tx);
        let result = stream.read_to_end().await.context("Failed to read from socket")?;
        let actual = std::str::from_utf8(&result).context("Failed to parse bytes")?.to_owned();

        assert_eq!(actual, msgs.join(""));
        Ok(())
    }

    #[fuchsia::test]
    async fn buffer_and_drain_reads_each_line_as_a_new_message() -> Result<(), Error> {
        let (stream, tx) = create_logger_stream()?;
        let (mut logger, rx) = create_datagram_logger()?;
        let msg = "Hello World\nHola Mundo!\n你好，世界!";

        let (tx_peeks, rx_peeks) = mpsc::channel();

        let () = take_and_write_to_socket(tx, msg)?;
        let (actual, ()) = try_join!(read_all_messages(rx), async move {
            stream
                .buffer_drain_and_peek(
                    &mut logger,
                    Some(move |line: &[u8]| tx_peeks.send(line.len()).unwrap()),
                )
                .await
                .context("Failed to drain stream")
        },)?;

        let expected = vec![
            "Hello World\n".to_string(),
            "Hola Mundo!\n".to_string(),
            "你好，世界!".to_string(),
        ];
        assert_eq!(actual, expected);

        let lengths = rx_peeks.iter().collect::<Vec<_>>();

        assert_eq!(lengths, expected.iter().map(|v| v.len()).collect::<Vec<_>>());

        Ok(())
    }

    #[fuchsia::test]
    async fn buffer_and_drain_does_not_buffer_past_maximum_size() -> Result<(), Error> {
        let msg = get_random_string(MAX_LINE_BUFFER_LENGTH + 10);
        let (stream, tx) = create_logger_stream()?;
        let (mut logger, rx) = create_datagram_logger()?;

        let (tx_peeks, rx_peeks) = mpsc::channel();

        let () = take_and_write_to_socket(tx, &msg)?;
        let (actual, ()) = try_join!(read_all_messages(rx), async move {
            stream
                .buffer_drain_and_peek(
                    &mut logger,
                    Some(move |line: &[u8]| {
                        tx_peeks.send(line.len()).unwrap();
                    }),
                )
                .await
                .context("Failed to drain stream")
        },)?;

        let lengths = rx_peeks.iter().collect::<Vec<_>>();

        assert_eq!(actual.len(), 2);
        assert_eq!(actual[0], msg[0..MAX_LINE_BUFFER_LENGTH]);
        assert_eq!(actual[1], msg[MAX_LINE_BUFFER_LENGTH..]);

        assert_eq!(lengths, vec![MAX_LINE_BUFFER_LENGTH, 10]);

        Ok(())
    }

    #[fuchsia::test]
    async fn buffer_and_drain_dumps_full_buffer_if_no_newline_seen() -> Result<(), Error> {
        let (stream, tx) = create_logger_stream()?;
        let (mut logger, rx) = create_datagram_logger()?;

        let ((), ()) = try_join!(
            async move {
                let msg = get_random_string(SOCKET_BUFFER_SIZE);
                // First write up to (SOCKET_BUFFER_SIZE - 1) so that we can
                // assert that buffer isn't drained prematurely.
                let () = write_to_socket(&tx, &msg[..SOCKET_BUFFER_SIZE - 1])?;

                // Temporarily convert fasync::Socket back to zx::Socket so that
                // we can use non-blocking `read` call.
                let rx = rx.into_zx_socket();
                let mut buffer = vec![0u8; SOCKET_BUFFER_SIZE];
                let maybe_bytes_read = rx.read(&mut buffer);
                assert_eq!(maybe_bytes_read, Err(zx::Status::SHOULD_WAIT));

                // Write last byte
                let () = write_to_socket(&tx, &msg[SOCKET_BUFFER_SIZE - 1..SOCKET_BUFFER_SIZE])?;

                // Confirm we still didn't write, waiting for newline.
                let maybe_bytes_read = rx.read(&mut buffer);
                assert_eq!(maybe_bytes_read, Err(zx::Status::SHOULD_WAIT));

                // Drop socket to unblock the read routine.
                std::mem::drop(tx);

                // Convert zx::Socket back to fasync::Socket.
                let mut rx = fasync::Socket::from_socket(rx);
                let bytes_read =
                    rx.read(&mut buffer).await.context("Failed to read from socket")?;
                let msg_written = std::str::from_utf8(&buffer).context("Failed to parse bytes")?;

                assert_eq!(bytes_read, SOCKET_BUFFER_SIZE);
                assert_eq!(msg_written, msg);

                Ok(())
            },
            async move { stream.buffer_and_drain(&mut logger).await.context("Failed to drain stream") },
        )?;

        Ok(())
    }

    #[fuchsia::test]
    async fn buffer_and_drain_return_error_if_stream_polls_err() -> Result<(), Error> {
        let (_tx, rx) = zx::Socket::create_stream();
        // If we don't have read rights the socket returns an error when polled.
        let rx = rx.duplicate_handle(zx::Rights::BASIC).expect("duplicate");
        let stream = LoggerStream::new(rx).context("Failed to create LoggerStream")?;
        let (mut logger, _rx) = create_datagram_logger()?;

        let result = stream.buffer_and_drain(&mut logger).await;

        assert_matches!(result, Err(LogError::Read(_)));
        Ok(())
    }

    async fn read_all_messages(socket: fasync::Socket) -> Result<Vec<String>, Error> {
        let mut results = Vec::new();
        let mut stream = socket.into_datagram_stream();
        while let Some(bytes) = stream.try_next().await.context("Failed to read socket stream")? {
            results.push(
                std::str::from_utf8(&bytes).context("Failed to parse bytes into utf8")?.to_owned(),
            );
        }

        Ok(results)
    }

    fn take_and_write_to_socket(socket: zx::Socket, message: &str) -> Result<(), Error> {
        write_to_socket(&socket, &message)
    }

    fn write_to_socket(socket: &zx::Socket, message: &str) -> Result<(), Error> {
        let bytes_written =
            socket.write(message.as_bytes()).context("Failed to write to socket")?;
        match bytes_written == message.len() {
            true => Ok(()),
            false => Err(format_err!(
                "Bytes written to socket doesn't match len of message. Message len = {}. Bytes written = {}",
                message.len(),
                bytes_written
            )),
        }
    }

    fn create_datagram_logger() -> Result<(SocketLogWriter, fasync::Socket), Error> {
        let (tx, rx) = zx::Socket::create_datagram();
        let logger = SocketLogWriter::new(fasync::Socket::from_socket(tx));
        let rx = fasync::Socket::from_socket(rx);
        Ok((logger, rx))
    }

    fn create_logger_stream() -> Result<(LoggerStream, zx::Socket), Error> {
        let (tx, rx) = zx::Socket::create_stream();
        let stream = LoggerStream::new(rx).context("Failed to create LoggerStream")?;
        Ok((stream, tx))
    }

    fn get_random_string(size: usize) -> String {
        Alphanumeric.sample_string(&mut rng(), size)
    }
}
