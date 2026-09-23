// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async::{self as fasync, ReadableHandle, ReadableState};

use futures::Stream;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use thiserror::Error;

const NEWLINE: u8 = b'\n';

/// Splits the bytes from a streaming socket into newlines suitable for forwarding to LogSink.
/// Returned chunks may not be complete newlines if single lines are over the size limit for a log
/// message.
///
/// This implementation prioritizes standing memory usage over the number of copies or allocations
/// made. Log forwarding is not particularly throughput sensitive, but keeping around lots of large
/// buffers takes up memory.
pub struct NewlineChunker {
    socket: fasync::Socket,
    buffer: Vec<u8>,
    is_terminated: bool,
    max_message_size: usize,
    trim_newlines: bool,
}

impl NewlineChunker {
    /// Creates a `NewlineChunker` that does not include the trailing `\n` in each line.
    pub fn new(socket: fasync::Socket, max_message_size: usize) -> Self {
        Self { socket, buffer: vec![], is_terminated: false, max_message_size, trim_newlines: true }
    }

    /// Creates a `NewlineChunker` that includes the trailing `\n` in each line.
    pub fn new_with_newlines(socket: fasync::Socket, max_message_size: usize) -> Self {
        Self {
            socket,
            buffer: vec![],
            is_terminated: false,
            max_message_size,
            trim_newlines: false,
        }
    }

    /// Removes and returns the next line or maximum-size chunk from the head of the buffer if
    /// available.
    fn next_chunk_from_buffer(&mut self) -> Option<Vec<u8>> {
        let new_tail_start =
            if let Some(mut newline_pos) = self.buffer.iter().position(|&b| b == NEWLINE) {
                // start the tail 1 past the last newline encountered
                while let Some(&NEWLINE) = self.buffer.get(newline_pos + 1) {
                    newline_pos += 1;
                }
                newline_pos + 1
            } else if self.buffer.len() >= self.max_message_size {
                // we have to check the length *after* looking for newlines in case a single socket
                // read was larger than the max size but contained newlines in the first
                // self.max_message_size bytes
                self.max_message_size
            } else {
                // no newlines, and the bytes in the buffer are too few to force chunking
                return None;
            };

        // the tail becomes the head for the next chunk
        let new_tail = self.buffer.split_off(new_tail_start);
        let mut next_chunk = std::mem::replace(&mut self.buffer, new_tail);

        if self.trim_newlines {
            // remove the newlines from the end of the chunk we're returning
            while let Some(&NEWLINE) = next_chunk.last() {
                next_chunk.pop();
            }
        }

        Some(next_chunk)
    }

    fn end_of_stream(&mut self) -> Poll<Option<Vec<u8>>> {
        if !self.buffer.is_empty() {
            // the buffer is under the forced chunk size because the first return didn't happen
            Poll::Ready(Some(std::mem::replace(&mut self.buffer, vec![])))
        } else {
            // end the stream
            self.is_terminated = true;
            Poll::Ready(None)
        }
    }
}

impl Stream for NewlineChunker {
    type Item = Result<Vec<u8>, NewlineChunkerError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.is_terminated {
            return Poll::Ready(None);
        }

        // first check to see if previous socket reads have left us with lines in the buffer
        if let Some(chunk) = this.next_chunk_from_buffer() {
            return Poll::Ready(Some(Ok(chunk)));
        }

        loop {
            // we don't have a chunk to return, poll for reading the socket
            let readable_state = futures::ready!(this.socket.poll_readable(cx))
                .map_err(NewlineChunkerError::PollReadable)?;

            // find out how much buffer we should make available
            let bytes_in_socket = this
                .socket
                .as_ref()
                .outstanding_read_bytes()
                .map_err(NewlineChunkerError::OutstandingReadBytes)?;
            if bytes_in_socket == 0 {
                if readable_state == ReadableState::MaybeReadableAndClosed {
                    return this.end_of_stream().map(|buf| buf.map(Ok));
                }
                // if there are no bytes available this socket should not be considered readable
                ready!(this.socket.need_readable(cx).map_err(NewlineChunkerError::NeedReadable)?);
                continue;
            }

            // don't make the buffer bigger than necessary to get a chunk out
            let bytes_to_read = std::cmp::min(bytes_in_socket, this.max_message_size);
            let prev_len = this.buffer.len();

            // grow the size of the buffer to make space for the pending read, if it fails we'll
            // need to shrink it back down before any subsequent calls to poll_next
            this.buffer.resize(prev_len + bytes_to_read, 0);

            let bytes_read = match this.socket.as_ref().read(&mut this.buffer[prev_len..]) {
                Ok(b) => b,
                Err(zx::Status::PEER_CLOSED) => return this.end_of_stream().map(|buf| buf.map(Ok)),
                Err(zx::Status::SHOULD_WAIT) => {
                    // reset the size of the buffer to exclude the 0's we wrote above
                    this.buffer.truncate(prev_len);
                    return Poll::Ready(Some(Err(NewlineChunkerError::ShouldWait)));
                }
                Err(status) => {
                    // reset the size of the buffer to exclude the 0's we wrote above
                    this.buffer.truncate(prev_len);
                    return Poll::Ready(Some(Err(NewlineChunkerError::ReadSocket(status))));
                }
            };

            // handle possible short reads
            this.buffer.truncate(prev_len + bytes_read);

            // we got something out of the socket
            if let Some(chunk) = this.next_chunk_from_buffer() {
                // and its enough for a chunk
                return Poll::Ready(Some(Ok(chunk)));
            } else {
                // it is not enough for a chunk, request notification when there's more
                ready!(this.socket.need_readable(cx).map_err(NewlineChunkerError::NeedReadable)?);
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum NewlineChunkerError {
    #[error("got SHOULD_WAIT from socket read after confirming outstanding_read_bytes > 0")]
    ShouldWait,

    #[error("failed to read from socket")]
    ReadSocket(#[source] zx::Status),

    #[error("failed to get readable state for socket")]
    PollReadable(#[source] zx::Status),

    #[error("failed to register readable signal for socket")]
    NeedReadable(#[source] zx::Status),

    #[error("failed to get number of outstanding readable bytes in socket")]
    OutstandingReadBytes(#[source] zx::Status),
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[fuchsia::test]
    async fn parse_bytes_with_newline() {
        let (s1, s2) = zx::Socket::create_stream();
        let s1 = fasync::Socket::from_socket(s1);
        let mut chunker = NewlineChunker::new(s1, 100);
        s2.write(b"test\n").expect("Failed to write");
        assert_eq!(chunker.next().await.unwrap().unwrap(), b"test".to_vec());
    }

    #[fuchsia::test]
    async fn parse_bytes_with_many_newlines() {
        let (s1, s2) = zx::Socket::create_stream();
        let s1 = fasync::Socket::from_socket(s1);
        let mut chunker = NewlineChunker::new(s1, 100);
        s2.write(b"test1\ntest2\ntest3\n").expect("Failed to write");
        assert_eq!(chunker.next().await.unwrap().unwrap(), b"test1".to_vec());
        assert_eq!(chunker.next().await.unwrap().unwrap(), b"test2".to_vec());
        assert_eq!(chunker.next().await.unwrap().unwrap(), b"test3".to_vec());
        std::mem::drop(s2);
        assert!(chunker.next().await.is_none());
    }

    #[fuchsia::test]
    async fn parse_bytes_with_newlines_included() {
        let (s1, s2) = zx::Socket::create_stream();
        let s1 = fasync::Socket::from_socket(s1);
        let mut chunker = NewlineChunker::new_with_newlines(s1, 100);
        s2.write(b"test1\ntest2\ntest3\n").expect("Failed to write");
        assert_eq!(chunker.next().await.unwrap().unwrap(), b"test1\n".to_vec());
        assert_eq!(chunker.next().await.unwrap().unwrap(), b"test2\n".to_vec());
        assert_eq!(chunker.next().await.unwrap().unwrap(), b"test3\n".to_vec());
    }

    #[fuchsia::test]
    async fn max_message_size() {
        let (s1, s2) = zx::Socket::create_stream();
        let s1 = fasync::Socket::from_socket(s1);
        let mut chunker = NewlineChunker::new(s1, 2);
        s2.write(b"test\n").expect("Failed to write");
        assert_eq!(chunker.next().await.unwrap().unwrap(), b"te".to_vec());
        assert_eq!(chunker.next().await.unwrap().unwrap(), b"st".to_vec());
    }
}
