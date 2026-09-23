// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::Mutex;
use std::collections::VecDeque;
use zx::Peered;

const FIFO_SIZE: usize = 255;
const PTY_DEVICE_SIGNAL_READABLE: zx::Signals = zx::Signals::USER_0;

pub struct Fifo {
    event: zx::EventPair,
    buffer: Mutex<VecDeque<u8>>,
}

impl Fifo {
    pub fn new(event: zx::EventPair) -> Self {
        Self { event, buffer: Mutex::new(VecDeque::with_capacity(FIFO_SIZE + 1)) }
    }

    pub fn read(&self, count: usize) -> Result<Vec<u8>, zx::Status> {
        let mut buffer = self.buffer.lock();
        if buffer.is_empty() {
            self.event
                .signal_peer(PTY_DEVICE_SIGNAL_READABLE, zx::Signals::NONE)
                .inspect_err(|s| println!("console: failed to signal peer: {}", s))?;
            return Err(zx::Status::SHOULD_WAIT);
        }

        let len = std::cmp::min(count, buffer.len());
        let data: Vec<u8> = buffer.drain(0..len).collect();

        if buffer.is_empty() {
            self.event
                .signal_peer(PTY_DEVICE_SIGNAL_READABLE, zx::Signals::NONE)
                .inspect_err(|s| println!("console: failed to signal peer: {}", s))?;
        }
        Ok(data)
    }

    pub fn write(&self, data: &[u8]) -> Result<usize, zx::Status> {
        let mut buffer = self.buffer.lock();
        if buffer.len() == FIFO_SIZE {
            return Err(zx::Status::SHOULD_WAIT);
        }

        let was_empty = buffer.is_empty();
        let len = std::cmp::min(data.len(), FIFO_SIZE - buffer.len());
        let iter = data[..len].iter().cloned();
        buffer.extend(iter);

        if was_empty && !buffer.is_empty() {
            self.event
                .signal_peer(zx::Signals::NONE, PTY_DEVICE_SIGNAL_READABLE)
                .inspect_err(|s| println!("console: failed to signal peer: {}", s))?;
        }

        Ok(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    fn is_readable(event: &zx::EventPair) -> bool {
        event.wait_one(PTY_DEVICE_SIGNAL_READABLE, zx::MonotonicInstant::INFINITE_PAST).is_ok()
    }

    #[test]
    fn empty_read() {
        let (ev1, ev2) = zx::EventPair::create();
        let fifo = Fifo::new(ev2);
        assert!(!is_readable(&ev1));

        assert_matches!(fifo.read(16), Err(zx::Status::SHOULD_WAIT));
    }

    #[test]
    fn some_data() {
        let (ev1, ev2) = zx::EventPair::create();
        let fifo = Fifo::new(ev2);
        let data: Vec<u8> = (1..=16).collect();
        assert_matches!(fifo.write(&data), Ok(16));
        assert!(is_readable(&ev1));

        let buf = fifo.read(15).unwrap();
        assert_eq!(buf.len(), 15);
        assert_eq!(buf, &data[..15]);
        assert!(is_readable(&ev1));

        let buf = fifo.read(1).unwrap();
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0], 16);
        assert!(!is_readable(&ev1));
    }

    #[test]
    fn fill() {
        let (ev1, ev2) = zx::EventPair::create();
        let fifo = Fifo::new(ev2);

        for j in 0..2 {
            let data: Vec<u8> =
                (0..=FIFO_SIZE).map(|i| (j * FIFO_SIZE / 2 + i + 1) as u8).collect();
            assert_matches!(fifo.write(&data), Ok(FIFO_SIZE));
            assert!(is_readable(&ev1));

            let buf = fifo.read(FIFO_SIZE).unwrap();
            assert_eq!(buf.len(), FIFO_SIZE);
            assert_eq!(buf, &data[..FIFO_SIZE]);
            assert!(!is_readable(&ev1));
        }
    }

    #[test]
    fn wrapping() {
        let (ev1, ev2) = zx::EventPair::create();
        let fifo = Fifo::new(ev2);
        let data: Vec<u8> = (1..=FIFO_SIZE).map(|i| i as u8).collect();
        assert_matches!(fifo.write(&data), Ok(FIFO_SIZE));

        let buf = fifo.read(FIFO_SIZE / 2).unwrap();
        assert_eq!(buf.len(), FIFO_SIZE / 2);
        assert_eq!(buf, &data[..127]);

        let remaining = FIFO_SIZE - buf.len();
        let data2: Vec<u8> = (1..=buf.len()).map(|i| (3 * i + 1) as u8).collect();
        assert_matches!(fifo.write(&data2), Ok(127));

        let buf2 = fifo.read(FIFO_SIZE).unwrap();
        assert_eq!(buf2.len(), FIFO_SIZE);
        assert_eq!(&buf2[..remaining], &data[127..]);
        assert_eq!(&buf2[remaining..], &data2[..]);
        assert!(!is_readable(&ev1));
    }

    #[test]
    fn write_full_fifo_should_wait() {
        let (_ev1, ev2) = zx::EventPair::create();
        let fifo = Fifo::new(ev2);

        // Fill the FIFO to its maximum capacity
        let data: Vec<u8> = (0..FIFO_SIZE).map(|i| i as u8).collect();
        assert_matches!(fifo.write(&data), Ok(FIFO_SIZE));

        // Attempt to write one more byte, which should result in SHOULD_WAIT
        let one_byte_data = [0u8];
        assert_matches!(fifo.write(&one_byte_data), Err(zx::Status::SHOULD_WAIT));
    }
}
