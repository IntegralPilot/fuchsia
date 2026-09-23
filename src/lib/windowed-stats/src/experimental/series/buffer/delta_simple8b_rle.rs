// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::bail;
use byteorder::{LittleEndian, WriteBytesExt as _};
use std::io;
use std::num::NonZeroUsize;

use crate::experimental::series::buffer::encoding;
use crate::experimental::series::buffer::simple8b_rle::Simple8bRleRingBuffer;

#[derive(Debug)]
pub enum Encoding {}

impl<A> encoding::Encoding<A> for Encoding {
    type Compression = encoding::compression::DeltaSimple8bRle;

    const PAYLOAD: encoding::payload::DeltaSimple8bRle =
        encoding::payload::DeltaSimple8bRle::Unsigned;
}

#[derive(Clone, Debug)]
pub struct DeltaSimple8bRleRingBuffer {
    base: Option<u64>,
    last: Option<u64>,
    buffer: Simple8bRleRingBuffer,
}

impl DeltaSimple8bRleRingBuffer {
    /// Create a new Simple8bRleRingBuffer that holds at least |min_samples|
    /// (including the base value).
    /// The buffer would continually grow and only evict data if it wouldn't
    /// cause the number of samples to fall below |min_samples|.
    pub const fn with_min_samples(min_samples: usize) -> Self {
        Self {
            base: None,
            last: None,
            buffer: Simple8bRleRingBuffer::with_min_samples(min_samples.saturating_sub(1)),
        }
    }

    /// Push |value| onto the ring buffer.
    /// |value| is expected to be equal or greater than the value that was
    /// passed into the previous call `push` or `fill`. If |value| is less than
    /// the previous value, return an `Err` and don't commit the value.
    pub fn push(&mut self, value: u64) -> Result<(), anyhow::Error> {
        let (base, last) = match (self.base.as_mut(), self.last.as_mut()) {
            (Some(base), Some(last)) => (base, last),
            _ => {
                self.base = Some(value);
                self.last = Some(value);
                return Ok(());
            }
        };

        if value < *last {
            bail!("Expect value to be non-decreasing: value {} last {}", value, *last);
        }
        let diff = value - *last;
        let evicted_blocks = self.buffer.push(diff);
        for evicted_block in evicted_blocks {
            *base = base.saturating_add(evicted_block.saturating_sum())
        }
        self.last = Some(value);
        Ok(())
    }

    /// Push |count| counts of the new value onto the ring buffer.
    /// |value| is expected to be equal or greater than the value that was
    /// passed into the previous call `push` or `fill`. If |value| is less than
    /// the previous value, return an `Err` and don't commit the value.
    pub fn fill(&mut self, value: u64, count: NonZeroUsize) -> Result<(), anyhow::Error> {
        self.push(value)?;
        if let Some(remaining_count) = NonZeroUsize::new(count.get() - 1) {
            // Diff is 0 because the value was already pushed once above, and
            // now we are pushing the same value again.
            const DIFF: u64 = 0;
            let evicted_blocks = self.buffer.fill(DIFF, remaining_count);
            for evicted_block in evicted_blocks {
                self.base =
                    self.base.map(|base| base.saturating_add(evicted_block.saturating_sum()));
            }
        }
        Ok(())
    }

    /// Serialize the DeltaSimple8bRleRingBuffer data into a bytes buffer.
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
            buffer.write_u64::<LittleEndian>(base)?;
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
        let mut ring_buffer = DeltaSimple8bRleRingBuffer::with_min_samples(MIN_SAMPLES);
        ring_buffer.push(42).expect("push should succeed");
        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            1, 0, // length
            0, 0, // selector head index
            0, // last block's # of values
            42, 0, 0, 0, 0, 0, 0, 0, // base value
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    #[test]
    fn test_ring_buffer_has_base_and_encoded_diff_value() {
        let mut ring_buffer = DeltaSimple8bRleRingBuffer::with_min_samples(MIN_SAMPLES);
        ring_buffer.push(42).expect("push should succeed");
        ring_buffer.push(50).expect("push should succeed");
        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            2, 0, // length
            0, 0, // selector head index
            1, // last block's # of values
            42, 0, 0, 0, 0, 0, 0, 0,    // base value
            0x0f, // RLE selector
            8, 0, 0, 0, 0, 0, // first block: value
            1, 0, //first block: length
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    #[test]
    fn test_evicted_values_are_added_to_base_value() {
        let mut ring_buffer = DeltaSimple8bRleRingBuffer::with_min_samples(10);
        let mut counter = 4u64;
        ring_buffer.push(counter).expect("push should succeed");
        for _ in 0..8 {
            counter += 0x80; // 0x80 == 128 and takes 8 bits
            ring_buffer.push(counter).expect("push should succeed");
            ring_buffer.push(counter).expect("push should succeed");
        }

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            0, 0, // selector head index
            8, // last block's # of values
            4, 0, 0, 0, 0, 0, 0, 0,    // base value
            0x77, // first block: 8-bit selector, second block: 8-bit selector
            0x80, 0, 0x80, 0, 0x80, 0, 0x80, 0, // first block
            0x80, 0, 0x80, 0, 0x80, 0, 0x80, 0, // second block
        ];
        assert_eq!(&buffer[..], expected_bytes);

        counter += 1;
        ring_buffer.push(counter).expect("push should succeed");

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            1, 0, // selector head index
            1, // last block's # of values
            0x04, 0x2, 0, 0, 0, 0, 0, 0,    // base value (4 + 128*4 = 516 = 0x204)
            0x77, // first block: 8-bit selector
            // Note that because selector head index is 1, the first block selector is at
            // bits 4-7. Bits 0-3 above are ignored.
            0x0f, // second block: RLE selector
            0x80, 0, 0x80, 0, 0x80, 0, 0x80, 0, // first block
            1, 0, 0, 0, 0, 0, // second block: value
            1, 0, // second block: length
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    #[test]
    fn test_multiple_evicted_blocks_are_added_to_base_value() {
        let mut ring_buffer = DeltaSimple8bRleRingBuffer::with_min_samples(17);
        let mut counter = 4u64;
        ring_buffer.push(counter).expect("push should succeed");
        for _ in 0..8 {
            counter += 0x80; // 0x80 == 128 and takes 8 bits
            ring_buffer.push(counter).expect("push should succeed");
            ring_buffer.push(counter).expect("push should succeed");
        }

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            0, 0, // selector head index
            8, // last block's # of values
            4, 0, 0, 0, 0, 0, 0, 0,    // base value
            0x77, // first block: 8-bit selector, second block: 8-bit selector
            0x80, 0, 0x80, 0, 0x80, 0, 0x80, 0, // first block
            0x80, 0, 0x80, 0, 0x80, 0, 0x80, 0, // second block
        ];
        assert_eq!(&buffer[..], expected_bytes);

        for _ in 0..8 {
            counter += 8;
            ring_buffer.push(counter).expect("push should succeed");
            ring_buffer.push(counter).expect("push should succeed");
        }
        // This one call will cause both the oldest blocks to be evicted.
        // We want to verify that both of them will be applied to the base value.
        counter += 1;
        ring_buffer.push(counter).expect("push should succeed");

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            0, 0, // selector head index
            1, // last block's # of values
            0x04, 0x4, 0, 0, 0, 0, 0, 0,    // base value (4 + 128*8 = 1028 = 0x404)
            0xf3, // first block: 4-bit selector, second block: RLE selector
            0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, // first block
            1, 0, 0, 0, 0, 0, // second block: value
            1, 0, // second block: length
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    #[test]
    fn test_error_on_decreasing_value() {
        let mut ring_buffer = DeltaSimple8bRleRingBuffer::with_min_samples(10);
        ring_buffer.push(42).expect("push should succeed");
        ring_buffer.push(41).expect_err("push should fail because of decreasing value");
    }

    #[test]
    fn test_fill() {
        let mut ring_buffer = DeltaSimple8bRleRingBuffer::with_min_samples(10);
        let mut counter = 4u64;
        ring_buffer.push(counter).expect("push should succeed");
        for _ in 0..8 {
            counter += 0x80; // 0x80 == 128 and takes 8 bits
            ring_buffer.fill(counter, NonZeroUsize::new(2).unwrap()).expect("fill should succeed");
        }

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            0, 0, // selector head index
            8, // last block's # of values
            4, 0, 0, 0, 0, 0, 0, 0,    // base value
            0x77, // first block: 8-bit selector, second block: 8-bit selector
            0x80, 0, 0x80, 0, 0x80, 0, 0x80, 0, // first block
            0x80, 0, 0x80, 0, 0x80, 0, 0x80, 0, // second block
        ];
        assert_eq!(&buffer[..], expected_bytes);

        counter += 1u64 << 32;
        ring_buffer.fill(counter, NonZeroUsize::new(9).unwrap()).expect("fill should succeed");

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            0, 0, // selector head index
            8, // last block's # of values
            0x04, 0x4, 0, 0, 0, 0, 0, 0,    // base value (4 + 128*8 = 1028 = 0x404)
            0xff, // first block: RLE selector, second block: RLE selector
            0, 0, 0, 0, 1, 0, // first block: value (1 << 32)
            1, 0, // first block: length
            0, 0, 0, 0, 0, 0, // second block: value
            8, 0, // second block: length
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    #[test]
    fn test_fill_error_on_decreasing_value() {
        let mut ring_buffer = DeltaSimple8bRleRingBuffer::with_min_samples(10);
        ring_buffer.push(42).expect("push should succeed");
        ring_buffer
            .fill(41, NonZeroUsize::new(5).unwrap())
            .expect_err("fill should fail because of decreasing value");
    }
}
