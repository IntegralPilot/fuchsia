// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::append::{Append, BufferTooSmall, TrackedAppend};
use zerocopy::SplitByteSliceMut;

pub struct BufferWriter<B> {
    buffer: B,
    written: usize,
}

impl<B: SplitByteSliceMut> BufferWriter<B> {
    pub fn new(buffer: B) -> Self {
        Self { buffer, written: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buffer.len() - self.written
    }

    pub fn written(&self) -> usize {
        self.written
    }

    pub fn into_written(self) -> B {
        let (written, _remaining) = self.buffer.split_at(self.written).ok().unwrap();
        written
    }

    fn next_mut_slice(&mut self, len: usize) -> Result<&mut [u8], BufferTooSmall> {
        if self.written + len > self.buffer.len() {
            return Err(BufferTooSmall);
        }
        let ret = &mut self.buffer[self.written..self.written + len];
        self.written += len;
        Ok(ret)
    }
}

impl<B: SplitByteSliceMut> Append for BufferWriter<B> {
    fn append_bytes(&mut self, bytes: &[u8]) -> Result<(), BufferTooSmall> {
        self.next_mut_slice(bytes.len())?.copy_from_slice(bytes);
        Ok(())
    }

    fn append_bytes_zeroed(&mut self, len: usize) -> Result<&mut [u8], BufferTooSmall> {
        let ret = self.next_mut_slice(len)?;
        for x in &mut ret[..] {
            *x = 0;
        }
        Ok(ret)
    }

    fn can_append(&self, bytes: usize) -> bool {
        self.remaining() >= bytes
    }
}

impl<B: SplitByteSliceMut> TrackedAppend for BufferWriter<B> {
    fn bytes_appended(&self) -> usize {
        self.written
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_too_short() {
        assert!(BufferWriter::new(&mut [0; 0][..]).append_byte(1).is_err());
        assert!(BufferWriter::new(&mut [0; 5][..]).append_bytes(&[0; 6]).is_err());
        assert!(BufferWriter::new(&mut [0; 5][..]).append_value_zeroed::<[u8; 6]>().is_err());
    }

    #[test]
    fn append_value_zeroed() {
        let mut buffer = [1u8; 5];
        let mut w = BufferWriter::new(&mut buffer[..]);
        let mut data = w.append_value_zeroed::<[u8; 3]>().expect("failed writing buffer");
        data[0] = 42;
        // Don't write `data[1]`: BufferWriter should zero this byte.
        data[2] = 43;

        assert_eq!(3, w.written());
        assert_eq!(2, w.remaining());
        assert_eq!([42, 0, 43, 1, 1], buffer);
    }

    #[test]
    fn append_bytes() {
        let mut buffer = [1u8; 5];
        let mut w = BufferWriter::new(&mut buffer[..]);
        w.append_byte(42).expect("failed writing buffer");
        w.append_byte(43).expect("failed writing buffer");
        w.append_bytes(&[2, 3]).expect("failed writing buffer");

        assert_eq!(4, w.written());
        assert_eq!(1, w.remaining());
        assert_eq!(&[42, 43, 2, 3], w.into_written());
        assert_eq!([42, 43, 2, 3, 1], buffer);
    }

    #[test]
    fn can_append() {
        let mut buffer = [1u8; 5];
        let mut w = BufferWriter::new(&mut buffer[..]);
        assert!(w.can_append(0));
        assert!(w.can_append(4));
        assert!(w.can_append(5));
        assert!(!w.can_append(6));

        w.append_byte(42).unwrap();
        assert!(w.can_append(4));
        assert!(!w.can_append(5));
    }
}
