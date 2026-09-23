// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use byteorder::{LittleEndian, WriteBytesExt};
use std::convert::Infallible;
use std::io;
use std::marker::PhantomData;
use std::num::NonZeroUsize;

use crate::experimental::series::buffer::encoding;
use crate::experimental::series::buffer::zigzag_simple8b_rle::ZigzagSimple8bRleRingBuffer;

#[derive(Debug)]
pub struct Encoding<A>(PhantomData<fn() -> A>, Infallible);
impl encoding::Encoding<i64> for Encoding<i64> {
    type Compression = encoding::compression::DeltaSimple8bRle;

    const PAYLOAD: encoding::payload::DeltaSimple8bRle =
        encoding::payload::DeltaSimple8bRle::Signed;
}

impl encoding::Encoding<u64> for Encoding<u64> {
    type Compression = encoding::compression::DeltaSimple8bRle;

    const PAYLOAD: encoding::payload::DeltaSimple8bRle =
        encoding::payload::DeltaSimple8bRle::UnsignedWithSignedDiff;
}

#[derive(Clone, Debug)]
pub struct DeltaZigzagSimple8bRleRingBuffer<A> {
    buffer: BaseDeltaZigzagSimple8bRleRingBuffer,
    phantom: PhantomData<fn() -> A>,
}

impl<A> DeltaZigzagSimple8bRleRingBuffer<A> {
    /// Create a new DeltaZigzagSimple8bRleRingBuffer that holds at least |min_samples|
    /// (including the base value).
    /// The buffer would continually grow and only evict data if it wouldn't
    /// cause the number of samples to fall below |min_samples|.
    pub const fn with_min_samples(min_samples: usize) -> Self {
        Self {
            buffer: BaseDeltaZigzagSimple8bRleRingBuffer::with_min_samples(min_samples),
            phantom: PhantomData,
        }
    }

    /// Serialize the DeltaZigzagSimple8bRleRingBuffer data into a bytes buffer.
    pub fn serialize(&self, buffer: &mut impl io::Write) -> io::Result<()> {
        self.buffer.serialize(buffer)
    }
}

impl DeltaZigzagSimple8bRleRingBuffer<i64> {
    /// Push |value| onto the ring buffer. Oldest values might be evicted by this call.
    /// Evicted values are applied to the base value.
    pub fn push(&mut self, value: i64) {
        self.buffer.push(value)
    }

    /// Push |count| counts of the new value onto the ring buffer.
    /// Oldest values might be evicted by this call. Evicted values are applied to base value
    pub fn fill(&mut self, value: i64, count: NonZeroUsize) {
        self.buffer.fill(value, count)
    }
}

impl DeltaZigzagSimple8bRleRingBuffer<u64> {
    /// Push |value| onto the ring buffer. Oldest values might be evicted by this call.
    /// Evicted values are applied to the base value.
    pub fn push(&mut self, value: u64) {
        self.buffer.push(value as i64)
    }

    /// Push |count| counts of the new value onto the ring buffer.
    /// Oldest values might be evicted by this call. Evicted values are applied to base value
    pub fn fill(&mut self, value: u64, count: NonZeroUsize) {
        self.buffer.fill(value as i64, count)
    }
}

#[derive(Clone, Debug)]
struct BaseDeltaZigzagSimple8bRleRingBuffer {
    base: Option<i64>,
    last: Option<i64>,
    buffer: ZigzagSimple8bRleRingBuffer,
}

impl BaseDeltaZigzagSimple8bRleRingBuffer {
    pub const fn with_min_samples(min_samples: usize) -> Self {
        Self {
            base: None,
            last: None,
            buffer: ZigzagSimple8bRleRingBuffer::with_min_samples(min_samples.saturating_sub(1)),
        }
    }

    pub fn push(&mut self, value: i64) {
        let (base, last) = match (self.base.as_mut(), self.last.as_mut()) {
            (Some(base), Some(last)) => (base, last),
            _ => {
                self.base = Some(value);
                self.last = Some(value);
                return;
            }
        };
        let diff = value - *last;
        let evicted_blocks = self.buffer.push(diff);
        for evicted_block in evicted_blocks {
            *base = base.saturating_add(evicted_block.saturating_sum_with_zigzag_decode());
        }
        self.last = Some(value);
    }

    pub fn fill(&mut self, value: i64, count: NonZeroUsize) {
        self.push(value);
        if let Some(remaining_count) = NonZeroUsize::new(count.get() - 1) {
            // Diff is 0 because the value was already pushed once above, and
            // now we are pushing the same value again.
            const DIFF: i64 = 0;
            let evicted_blocks = self.buffer.fill(DIFF, remaining_count);
            for evicted_block in evicted_blocks {
                self.base = self.base.map(|base| {
                    base.saturating_add(evicted_block.saturating_sum_with_zigzag_decode())
                });
            }
        }
    }

    pub fn serialize(&self, buffer: &mut impl io::Write) -> io::Result<()> {
        let metadata = self.buffer.metadata();
        // Add one count for the base value
        let num_blocks =
            metadata.num_blocks.checked_add(if self.base.is_some() { 1 } else { 0 }).ok_or_else(
                || io::Error::new(io::ErrorKind::InvalidData, "Metadata num_blocks overflow"),
            )?;
        buffer.write_u16::<LittleEndian>(num_blocks)?;
        buffer.write_u16::<LittleEndian>(metadata.selectors_head_index)?;
        buffer.write_u8(metadata.last_block_num_values)?;

        if let Some(base) = self.base {
            buffer.write_i64::<LittleEndian>(base)?;
        }
        self.buffer.serialize_data(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN_SAMPLES: usize = 120;

    #[test]
    fn test_ring_buffer_has_base_value_only() {
        let mut ring_buffer =
            DeltaZigzagSimple8bRleRingBuffer::<i64>::with_min_samples(MIN_SAMPLES);
        ring_buffer.push(-42);
        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            1, 0, // length
            0, 0, // selector head index
            0, // last block's # of values
            214, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // base value (-42)
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    #[test]
    fn test_ring_buffer_has_base_and_encoded_diff_value() {
        let mut ring_buffer =
            DeltaZigzagSimple8bRleRingBuffer::<i64>::with_min_samples(MIN_SAMPLES);
        ring_buffer.push(42);
        ring_buffer.push(0);
        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            2, 0, // length
            0, 0, // selector head index
            1, // last block's # of values
            42, 0, 0, 0, 0, 0, 0, 0,    // base value
            0x0f, // RLE selector
            83, 0, 0, 0, 0, 0, // first block: value (-42 is encoded as 83)
            1, 0, // first block: length
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    #[test]
    fn test_evicted_values_are_added_to_base_value() {
        let mut ring_buffer = DeltaZigzagSimple8bRleRingBuffer::<i64>::with_min_samples(10);
        let mut counter = 516i64;
        ring_buffer.push(counter);
        for _ in 0..8 {
            counter -= 128; // -128 will be encoded as 255, which takes 8 bits
            ring_buffer.push(counter);
            ring_buffer.push(counter);
        }

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            0, 0, // selector head index
            8, // last block's # of values
            0x04, 0x2, 0, 0, 0, 0, 0, 0,    // base value (0x204 = 516)
            0x77, // first block: 8-bit selector, second block: 8-bit selector
            255, 0, 255, 0, 255, 0, 255, 0, // first block (alternating encoded -128 and 0)
            255, 0, 255, 0, 255, 0, 255, 0, // second block (alternating encoded -128 and 0)
        ];
        assert_eq!(&buffer[..], expected_bytes);

        counter += 1;
        ring_buffer.push(counter);

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            1, 0, // selector head index
            1, // last block's # of values
            4, 0, 0, 0, 0, 0, 0, 0,    // base value (516 - 128*4 = 4)
            0x77, // first block: 8-bit selector
            // Note that because selector head index is 1, the first block selector is at
            // bits 4-7. Bits 0-3 above are ignored.
            0x0f, // second block: RLE selector
            255, 0, 255, 0, 255, 0, 255, 0, // first block
            2, 0, 0, 0, 0, 0, // second block: value (1 is encoded as 2)
            1, 0, // second block: length
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    #[test]
    fn test_u64_ring_buffer() {
        let mut ring_buffer =
            DeltaZigzagSimple8bRleRingBuffer::<u64>::with_min_samples(MIN_SAMPLES);
        ring_buffer.push(1);
        ring_buffer.push(u64::MAX);
        ring_buffer.push(3);
        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            2, 0, // length
            0, 0, // selector head index
            2, // last block's # of values
            1, 0, 0, 0, 0, 0, 0, 0,    // base value
            0x03, // 4-bit selector
            0x83, 0, 0, 0, 0, 0, 0, 0, // first block values (-2 and 4 encoded as 3 and 8)
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    // This test adopts `test_evicted_values_are_added_to_base_value` but modifies some
    // `push` calls to `fill`.
    #[test]
    fn test_fill() {
        let mut ring_buffer = DeltaZigzagSimple8bRleRingBuffer::<i64>::with_min_samples(10);
        let mut counter = 516i64;
        ring_buffer.push(counter);
        for _ in 0..8 {
            counter -= 128; // -128 will be encoded as 255, which takes 8 bits
            ring_buffer.fill(counter, NonZeroUsize::new(2).unwrap());
        }

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            0, 0, // selector head index
            8, // last block's # of values
            0x04, 0x2, 0, 0, 0, 0, 0, 0,    // base value (0x204 = 516)
            0x77, // first block: 8-bit selector, second block: 8-bit selector
            255, 0, 255, 0, 255, 0, 255, 0, // first block (alternating encoded -128 and 0)
            255, 0, 255, 0, 255, 0, 255, 0, // second block (alternating encoded -128 and 0)
        ];
        assert_eq!(&buffer[..], expected_bytes);

        counter += 1;
        ring_buffer.fill(counter, NonZeroUsize::new(1).unwrap());

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            1, 0, // selector head index
            1, // last block's # of values
            4, 0, 0, 0, 0, 0, 0, 0,    // base value (516 - 128*4 = 4)
            0x77, // first block: 8-bit selector
            // Note that because selector head index is 1, the first block selector is at
            // bits 4-7. Bits 0-3 above are ignored.
            0x0f, // second block: RLE selector
            255, 0, 255, 0, 255, 0, 255, 0, // first block
            2, 0, 0, 0, 0, 0, // second block: value (1 is encoded as 2)
            1, 0, // second block: length
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    // This test adopts `test_u64_ring_buffer` but modifies the last call.
    #[test]
    fn test_u64_ring_buffer_fill() {
        let mut ring_buffer =
            DeltaZigzagSimple8bRleRingBuffer::<u64>::with_min_samples(MIN_SAMPLES);
        ring_buffer.push(1);
        ring_buffer.push(u64::MAX);
        ring_buffer.fill(3, NonZeroUsize::new(40).unwrap());
        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            0, 0,  // selector head index
            25, // last block's # of values
            1, 0, 0, 0, 0, 0, 0, 0,    // base value
            0xf3, // first block: 4-bit selector, second block: RLE selector
            0x83, 0, 0, 0, 0, 0, 0,
            0, // first block values (-2, 4 encoded as 3, 8; then 14 0's)
            0, 0, 0, 0, 0, 0, // second block: value
            25, 0, // second block: length
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }
}
