// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io;
use std::num::NonZeroUsize;

use crate::experimental::series::buffer::encoding;
use crate::experimental::series::buffer::simple8b_rle::{
    Simple8bRleBlock, Simple8bRleBufferMetadata, Simple8bRleRingBuffer,
};

#[derive(Debug)]
pub enum Encoding {}

impl<A> encoding::Encoding<A> for Encoding {
    type Compression = encoding::compression::Simple8bRle;

    const PAYLOAD: encoding::payload::Simple8bRle = encoding::payload::Simple8bRle::Signed;
}

/// ZigzagSimple8bRleRingBuffer is a ring buffer that allows logging an i64 into a
/// Simple8bRleRingBuffer. It does this by zigzag encoding an i64 before logging the
/// result into the latter.
#[derive(Clone, Debug)]
pub struct ZigzagSimple8bRleRingBuffer {
    buffer: Simple8bRleRingBuffer,
}

impl ZigzagSimple8bRleRingBuffer {
    /// Create a new ZigzagSimple8bRleRingBuffer that holds at least |min_samples|
    /// The buffer would continually grow and only evict data if it wouldn't
    /// cause the number of samples to fall below |min_samples|.
    pub const fn with_min_samples(min_samples: usize) -> Self {
        Self { buffer: Simple8bRleRingBuffer::with_min_samples(min_samples) }
    }

    /// Serialize the ZigzagSimple8bRleRingBuffer data into a bytes buffer.
    pub fn serialize(&self, buffer: &mut impl io::Write) -> io::Result<()> {
        self.buffer.serialize(buffer)
    }

    /// Push a new value onto the ZigzagSimple8bRleRingBuffer. This might evict
    /// one or more oldest values in the process.
    pub fn push(&mut self, value: i64) -> Vec<Simple8bRleBlock> {
        self.buffer.push(zigzag_encode(value))
    }

    /// Push |count| counts of the new value onto the ZigzagSimple8bRleRingBuffer.
    /// This might evict one or more oldest values in the process.
    pub fn fill(&mut self, value: i64, count: NonZeroUsize) -> Vec<Simple8bRleBlock> {
        self.buffer.fill(zigzag_encode(value), count)
    }

    pub(crate) fn metadata(&self) -> Simple8bRleBufferMetadata {
        self.buffer.metadata()
    }

    pub(crate) fn serialize_data(&self, buffer: &mut impl io::Write) -> io::Result<()> {
        self.buffer.serialize_data(buffer)
    }
}

/// Return `-2x-1` if `x` is negative. Otherwise return `2x`
const fn zigzag_encode(x: i64) -> u64 {
    // The below bitwise operation assumes that the number has two-complement representation.
    //
    // Note that `>>` is an arithmetic shift and not a logical shift
    // because `i64` is a signed integer type.
    // When `x` is positive, `x >> 63` is always 0, hence `0 ^ (x << 1) = 2x`
    // When `x` is negative, `x >> 63` is always -1, which is all 1 bits,
    // hence `-1 ^ (x << 1) = -2x-1`
    //
    // Example for -1 = 0b111...111:
    // ```
    // 0b111...111 >> 63 = 0b111...111 (which is -1)
    // 0b111...111 << 1 = 0b111...110 (which is -2)
    // 0b111...111 ^ 0b111...110 = 0b000...001 (which is 1)
    // ```
    ((x >> i64::BITS - 1) ^ (x << 1)) as u64
}

// Return `-x//2 - 1` if `x` is odd. Otherwise return `x//2`
pub const fn zigzag_decode(x: u64) -> i64 {
    // The below bitwise operation assumes that the number has two-complement representation.
    //
    // When `x` is even, `-(x & 1)` is always 0, hence `(x >> 1) & 0 = x//2`
    // When `x` is odd, `-(x & 1)` is always -1, which is all 1 bits,
    // hence `(x >> 1) ^ -1 = -x//2 - 1`
    (x >> 1) as i64 ^ -((x & 1) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN_SAMPLES: usize = 120;

    #[test]
    fn test_zigzag_simple8b_rle_ring_buffer() {
        let mut ring_buffer = ZigzagSimple8bRleRingBuffer::with_min_samples(MIN_SAMPLES);
        ring_buffer.push(1);
        ring_buffer.push(i64::MIN);
        ring_buffer.push(i64::MAX);
        ring_buffer.push(-1);

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            4, 0, // length
            0, 0,    // selector head index
            1,    // last block's # of values
            0xef, // first block: RLE selector, second block: 64-bit selector
            0xfe, // third block: 64-bit selector, fourth block: RLE selector
            2, 0, 0, 0, 0, 0, // first block: value (1 is encoded as 2)
            1, 0, // first block: length
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // second block
            0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // third block
            1, 0, 0, 0, 0, 0, // fourth block: value (-1 is encoded as 1)
            1, 0, // fourth block: length
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    #[test]
    fn test_zigzag_simple8b_rle_ring_buffer_fill() {
        let mut ring_buffer = ZigzagSimple8bRleRingBuffer::with_min_samples(MIN_SAMPLES);
        ring_buffer.fill(1, NonZeroUsize::new(10).unwrap());

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            1, 0, // length
            0, 0,    // selector head index
            10,   // last block's # of values
            0x0f, // first block: RLE selector
            2, 0, 0, 0, 0, 0, // first block: value (1 is encoded as 2)
            10, 0, // first block: length
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }
}
