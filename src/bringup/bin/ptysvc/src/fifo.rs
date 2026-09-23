// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::VecDeque;
use zx::Peered;

// 4KB buffer
pub const FIFO_SIZE: usize = 4096;
const PTY_DEVICE_SIGNAL_READABLE: zx::Signals = zx::Signals::USER_0;

pub struct Fifo {
    event: zx::EventPair,
    buffer: VecDeque<u8>,
}

impl Fifo {
    pub fn new(event: zx::EventPair) -> Self {
        Self { event, buffer: VecDeque::with_capacity(FIFO_SIZE) }
    }

    pub fn read(&mut self, count: usize) -> Result<Vec<u8>, zx::Status> {
        if count == 0 {
            return Ok(Vec::new());
        }

        if self.buffer.is_empty() {
            self.event
                .signal_peer(PTY_DEVICE_SIGNAL_READABLE, zx::Signals::NONE)
                .inspect_err(|s| eprintln!("ptysvc: failed to signal peer: {}", s))?;
            return Err(zx::Status::SHOULD_WAIT);
        }

        let len = std::cmp::min(count, self.buffer.len());
        let data: Vec<u8> = self.buffer.drain(0..len).collect();

        if self.buffer.is_empty() {
            self.event
                .signal_peer(PTY_DEVICE_SIGNAL_READABLE, zx::Signals::NONE)
                .inspect_err(|s| eprintln!("ptysvc: failed to signal peer: {}", s))?;
        }
        Ok(data)
    }

    // Returns number of bytes written.
    // If atomic is true, it writes all or nothing.
    pub fn write(&mut self, data: &[u8], atomic: bool) -> Result<usize, zx::Status> {
        if data.is_empty() {
            return Ok(0);
        }

        let avail = FIFO_SIZE - self.buffer.len();

        let len = if atomic {
            if avail < data.len() {
                return Ok(0);
            }
            data.len()
        } else {
            if avail == 0 {
                return Err(zx::Status::SHOULD_WAIT);
            }
            std::cmp::min(data.len(), avail)
        };

        let was_empty = self.buffer.is_empty();
        self.buffer.extend(data[..len].iter().cloned());

        if was_empty && !self.buffer.is_empty() {
            self.event
                .signal_peer(zx::Signals::NONE, PTY_DEVICE_SIGNAL_READABLE)
                .inspect_err(|s| eprintln!("ptysvc: failed to signal peer: {}", s))?;
        }

        Ok(len)
    }

    pub fn is_full(&self) -> bool {
        self.buffer.len() == FIFO_SIZE
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn test_fifo_read_write() {
        let (local, remote) = zx::EventPair::create();
        let mut fifo = Fifo::new(local);

        // Initially empty.
        assert!(fifo.is_empty());

        // Reading empty fifo should return SHOULD_WAIT and clear signal (if it was set).
        assert_eq!(fifo.read(10), Err(zx::Status::SHOULD_WAIT));

        // Verify signal is not set on peer.
        assert_matches!(
            remote.wait_one(PTY_DEVICE_SIGNAL_READABLE, zx::MonotonicInstant::INFINITE_PAST),
            zx::WaitResult::TimedOut(_)
        );

        // Write data.
        let data = vec![1u8, 2, 3, 4, 5];
        assert_eq!(fifo.write(&data, false), Ok(5));
        assert!(!fifo.is_empty());

        // Verify signal is set on peer.
        assert_matches!(
            remote.wait_one(PTY_DEVICE_SIGNAL_READABLE, zx::MonotonicInstant::INFINITE_PAST),
            zx::WaitResult::Ok(_)
        );

        // Read partial.
        assert_eq!(fifo.read(2), Ok(vec![1, 2]));
        assert!(!fifo.is_empty());
        // Signal should still be set.
        assert_matches!(
            remote.wait_one(PTY_DEVICE_SIGNAL_READABLE, zx::MonotonicInstant::INFINITE_PAST),
            zx::WaitResult::Ok(_)
        );

        // Read rest.
        assert_eq!(fifo.read(10), Ok(vec![3, 4, 5]));
        assert!(fifo.is_empty());

        // Verify signal is cleared on peer.
        assert_matches!(
            remote.wait_one(PTY_DEVICE_SIGNAL_READABLE, zx::MonotonicInstant::INFINITE_PAST),
            zx::WaitResult::TimedOut(_)
        );
    }

    #[test]
    fn test_fifo_atomic_write() {
        let (local, _remote) = zx::EventPair::create();
        let mut fifo = Fifo::new(local);

        // Fill up close to full.
        let fill = vec![0u8; FIFO_SIZE - 5];
        assert_eq!(fifo.write(&fill, false), Ok(FIFO_SIZE - 5));

        // Try to write 6 bytes atomically (should fail/return 0).
        let data = vec![1u8; 6];
        assert_eq!(fifo.write(&data, true), Ok(0));

        // Try to write 5 bytes atomically (should succeed).
        let data_fits = vec![1u8; 5];
        assert_eq!(fifo.write(&data_fits, true), Ok(5));
        assert!(fifo.is_full());
    }

    #[test]
    fn test_fifo_non_atomic_write() {
        let (local, _remote) = zx::EventPair::create();
        let mut fifo = Fifo::new(local);

        // Fill up close to full.
        let fill = vec![0u8; FIFO_SIZE - 5];
        assert_eq!(fifo.write(&fill, false), Ok(FIFO_SIZE - 5));

        // Try to write 10 bytes non-atomically (should write 5).
        let data = vec![1u8; 10];
        assert_eq!(fifo.write(&data, false), Ok(5));
        assert!(fifo.is_full());

        // Try to write more (should return SHOULD_WAIT).
        assert_eq!(fifo.write(&data, false), Err(zx::Status::SHOULD_WAIT));
    }
}
