// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(
    missing_docs,
    unreachable_patterns,
    clippy::useless_conversion,
    clippy::redundant_clone,
    clippy::precedence
)]

//! A chunked ring buffer implementation.
//!
//! The chunk size is defined at time of construction. When a write results
//! in a chunk that is greater than or equal to chunk size, the current
//! tail pointer is recorded to delimit chunks. When there is not enough room
//! for a write, the next chunk is discarded to make room for the write.

use std::collections::VecDeque;

use thiserror::Error;

/// Error returned when a write is larger than the buffer capacity.
#[derive(Error, Debug, PartialEq)]
#[error("write size is larger than buffer size")]
pub struct WriteTooLarge;

/// Chunked ring buffer.
///
/// A ring buffer which is logically split into chunks where each chunk is
/// at least `chunk_size`. A transaction API is provided if the caller needs
/// to make multiple writes that must not be split across chunks. Chunks are
/// discarded in their entirety when needed to make space.
///
/// A smaller chunk size has the advantage of higher utilization of the space
/// in the ring buffer in exchange for more memory used keeping track of
/// chunk boundaries and more work done when discarding chunks. Conversely a
/// larger chunk size means that utilization of the buffer space has higher
/// variance, but less memory is needed and discards are less often. Users
/// should choose a chunk size appropriate for their use case.
#[derive(Debug)]
pub struct RingBuffer {
    buf: Vec<u8>,
    chunk_size: usize,
    tail: usize,
    // Empty when the ring buffer is completely empty, otherwise the front
    // element is the head pointer. This fact is used to distinguish between
    // the buffer being completely full vs completely empty.
    boundary_indices: VecDeque<usize>,
}

impl RingBuffer {
    /// Create a new chunked ring buffer.
    ///
    /// The capacity of the buffer is set to the next multiple of `chunk_size`
    /// larger than or equal to `size`.
    pub fn new(size: usize, chunk_size: usize) -> Self {
        let size = if chunk_size == 0 { size } else { size.next_multiple_of(chunk_size) };
        Self { buf: vec![0u8; size], chunk_size, tail: 0, boundary_indices: VecDeque::new() }
    }

    /// Get the head/read pointer.
    fn head(&self) -> usize {
        *self.boundary_indices.front().unwrap_or(&self.tail)
    }

    /// Compute the total number of bytes stored in the buffer.
    pub fn len(&self) -> usize {
        let head = self.head();
        if self.tail == head && !self.boundary_indices.is_empty() {
            self.buf.len()
        } else {
            self.bytes_between(head, self.tail)
        }
    }

    fn bytes_between(&self, i: usize, j: usize) -> usize {
        if j >= i { j - i } else { self.buf.len() - i + j }
    }

    /// Return the data stored in the ring buffer as two byte slices.
    pub fn get_view(&self) -> (&[u8], &[u8]) {
        let head = self.head();
        if self.tail > head || self.tail == head && self.boundary_indices.is_empty() {
            (&self.buf[head..self.tail], &[])
        } else {
            (&self.buf[head..], &self.buf[..self.tail])
        }
    }

    /// Start a transaction that groups a series of writes.
    pub fn start_transaction(&mut self) -> Transaction<'_> {
        Transaction::new(self)
    }

    // NB: This function is a helper for the public write functions, and
    // assumes things like `slice` being non-empty and that it fits in
    // the buffer.
    fn write_inner(&mut self, slice: &[u8]) {
        while self.len() + slice.len() > self.buf.len() {
            let _ = self.boundary_indices.pop_front();
            if self.boundary_indices.is_empty() {
                break;
            }
        }
        if self.boundary_indices.is_empty() {
            self.boundary_indices.push_back(self.tail);
        }

        if self.tail + slice.len() >= self.buf.len() {
            let remaining = self.buf.len() - self.tail;
            assert!(remaining > 0);
            self.buf[self.tail..self.tail + remaining].copy_from_slice(&slice[..remaining]);
            let data_remaining = slice.len() - remaining;
            if data_remaining > 0 {
                self.buf[..data_remaining].copy_from_slice(&slice[remaining..]);
            }
            self.tail = data_remaining;
        } else {
            self.buf[self.tail..self.tail + slice.len()].copy_from_slice(&slice);
            self.tail += slice.len();
        }
    }

    fn maybe_chunk(&mut self) {
        let Some(penultimate) = self.boundary_indices.back() else {
            return;
        };
        // The buffer is full, there is no point in recording a boundary
        // because it will just duplicate the existing value in
        // `boundary_indices`.
        if *penultimate == self.tail {
            return;
        }
        if self.bytes_between(*penultimate, self.tail) >= self.chunk_size {
            self.boundary_indices.push_back(self.tail);
        }
    }

    /// Writes `slice` to the buffer and records a chunk if it is large enough.
    pub fn write(&mut self, slice: &[u8]) -> Result<(), WriteTooLarge> {
        let mut transaction = self.start_transaction();
        transaction.write(slice)?;
        transaction.commit();
        Ok(())
    }

    fn rollback(&mut self, start: usize) {
        if self.head() == start {
            self.boundary_indices.clear();
        }
        self.tail = start;
    }
}

/// A transaction which consists of a series of writes that can be committed
/// or rolled back.
pub struct Transaction<'a> {
    buffer: &'a mut RingBuffer,
    start: usize,
    written: usize,
    completed: bool,
}

impl<'a> Transaction<'a> {
    /// Create a new transaction.
    pub fn new(buffer: &'a mut RingBuffer) -> Self {
        let start = buffer.tail;
        Self { buffer, start, written: 0, completed: false }
    }

    /// Perform a write.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), WriteTooLarge> {
        if bytes.len() == 0 {
            return Ok(());
        }

        if self.written + bytes.len() > self.buffer.buf.len() {
            return Err(WriteTooLarge);
        }

        self.buffer.write_inner(bytes);
        self.written += bytes.len();
        Ok(())
    }

    /// Commit the transaction by recording a chunk.
    pub fn commit(mut self) {
        self.buffer.maybe_chunk();
        self.completed = true;
    }
}

impl<'a> Drop for Transaction<'a> {
    fn drop(&mut self) {
        if !self.completed {
            self.buffer.rollback(self.start);
        }
    }
}

impl<'a> std::io::Write for Transaction<'a> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.write(buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use test_case::test_case;

    #[test]
    fn test_write_no_wrap() {
        let mut cb = RingBuffer::new(8, 4);
        const DATA: &[u8] = b"hello";
        cb.write(DATA).unwrap();

        assert_eq!(cb.boundary_indices, VecDeque::from([0, 5]));
        let (v1, v2) = cb.get_view();
        assert_eq!(v1, DATA);
        assert_eq!(v2, &[]);
    }

    #[test]
    fn test_write_wrap() {
        let mut cb = RingBuffer::new(8, 4);
        let mut tx = cb.start_transaction();
        tx.write(b"foo").unwrap();
        tx.write(b"bar").unwrap();
        tx.commit();
        assert_eq!(cb.boundary_indices, VecDeque::from([0, 6]));

        let mut tx = cb.start_transaction();
        tx.write(b"baz").unwrap();
        tx.write(b"qux").unwrap();
        tx.commit();
        assert_eq!(cb.boundary_indices, VecDeque::from([6, 4]));

        let (v1, v2) = cb.get_view();
        assert_eq!(v1, b"ba");
        assert_eq!(v2, b"zqux");
    }

    #[test_case(8; "chunk_size_equals_buffer_size")]
    #[test_case(4; "chunk_size_half_of_buffer_size")]
    fn test_write_exact_fill(chunk_size: usize) {
        let mut cb = RingBuffer::new(8, chunk_size);
        cb.write(b"12345678").unwrap();
        assert_eq!(cb.boundary_indices, VecDeque::from([0]));
        assert_eq!(cb.tail, 0);

        let (v1, v2) = cb.get_view();
        assert_eq!(v1, b"12345678");
        assert_eq!(v2, &[]);

        cb.write(b"9").unwrap();
        let (v1, v2) = cb.get_view();
        assert_eq!(v1, b"9");
        assert_eq!(v2, &[]);
    }

    #[test]
    fn test_write_smaller_than_chunk_size() {
        const CHUNK_SIZE: usize = 4;
        let mut cb = RingBuffer::new(8, CHUNK_SIZE);
        for i in 1u8..4 {
            cb.write(&[i]).unwrap();
            assert_eq!(cb.boundary_indices, VecDeque::from([0]));
        }
        cb.write(&[4]).unwrap();
        assert_eq!(cb.boundary_indices, VecDeque::from([0, CHUNK_SIZE]));

        let (v1, v2) = cb.get_view();
        assert_eq!(v1, &[1, 2, 3, 4]);
        assert_eq!(v2, &[]);
    }

    #[test]
    fn test_zero_chunk_size_zero_writes() {
        let mut cb = RingBuffer::new(8, 0);
        cb.write(b"").unwrap();
        assert_eq!(cb.boundary_indices, VecDeque::new());
        let (v1, v2) = cb.get_view();
        assert_eq!(v1, &[]);
        assert_eq!(v2, &[]);
    }

    #[test]
    fn test_zero_chunk_size_fill_and_overwrite() {
        const N: usize = 4;
        let mut cb = RingBuffer::new(N, 0);

        for i in 1u8..=4 {
            cb.write(&[i]).unwrap();
        }

        cb.write(&[5]).unwrap();
        let (v1, v2) = cb.get_view();
        assert_eq!([v1, v2].concat(), vec![2, 3, 4, 5]);

        cb.write(&[6]).unwrap();
        let (v1, v2) = cb.get_view();
        assert_eq!([v1, v2].concat(), vec![3, 4, 5, 6]);
    }

    #[test]
    fn test_transaction_rollback_on_drop() {
        let mut cb = RingBuffer::new(8, 4);
        cb.write(b"ab").unwrap();
        {
            let mut tx = cb.start_transaction();
            tx.write(b"cd").unwrap();
            tx.write(b"ef").unwrap();
            // Transaction is dropped here.
        }

        let (v1, v2) = cb.get_view();
        assert_eq!(v1, b"ab");
        assert_eq!(v2, &[]);
    }

    #[test]
    fn test_transaction_too_large() {
        let mut cb = RingBuffer::new(8, 4);
        let mut tx = cb.start_transaction();
        tx.write(b"12345678").unwrap();
        assert_eq!(tx.write(b"9"), Err(WriteTooLarge));
    }
}
