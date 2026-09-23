// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use byteorder::{LittleEndian, WriteBytesExt};
use std::collections::VecDeque;
use std::convert::Infallible;
use std::io;
use std::marker::PhantomData;
use std::num::NonZeroUsize;

use crate::experimental::series::buffer::encoding;

#[derive(Debug)]
pub struct Encoding<A>(PhantomData<fn() -> A>, Infallible);

impl encoding::Encoding<f32> for Encoding<f32> {
    type Compression = encoding::compression::Uncompressed;

    const PAYLOAD: encoding::payload::Uncompressed = encoding::payload::Uncompressed::Float32;
}

#[derive(Clone, Debug)]
pub struct UncompressedRingBuffer<T> {
    buffer: VecDeque<T>,
    min_samples: usize,
}

impl<T> UncompressedRingBuffer<T> {
    /// Create a RingBuffer that holds at least |min_samples|
    /// The buffer would continually grow and only evict data if it wouldn't
    /// cause the number of samples to fall below |min_samples|.
    pub const fn with_min_samples(min_samples: usize) -> Self {
        Self { buffer: VecDeque::new(), min_samples }
    }

    /// Push |value| to the ring buffer, evicting and returning the oldest
    /// value if there are more than the required number of samples.
    pub fn push(&mut self, value: T) -> Option<T> {
        let mut popped = None;
        if self.buffer.len() >= self.min_samples {
            popped = self.buffer.pop_front();
        }
        self.buffer.push_back(value);
        popped
    }
}

impl<T: Clone> UncompressedRingBuffer<T> {
    /// Push |value| to the ring buffer |count| times, evicting the oldest
    /// values if there are more than the required number of samples.
    ///
    /// Internally, the ring buffer performs batch allocation rather than
    /// pushing the value individually. Additionally, for |count| larger
    /// than the buffer size, the ring buffer would only create enough
    /// values to fill the buffer as an optimization.
    pub fn fill(&mut self, value: T, count: NonZeroUsize) {
        let num_samples_to_push = std::cmp::min(self.min_samples, count.get());
        let total_before_pop = self.buffer.len() + num_samples_to_push;
        if total_before_pop > self.min_samples {
            let remove_count = total_before_pop - self.min_samples;
            self.buffer.drain(..remove_count);
        }
        self.buffer.append(&mut vec![value; num_samples_to_push].into());
    }
}

impl UncompressedRingBuffer<f32> {
    pub fn serialize(&self, buffer: &mut impl io::Write) -> io::Result<()> {
        buffer.write_u16::<LittleEndian>(self.buffer.len() as u16)?;
        for value in self.buffer.iter() {
            buffer.write_f32::<LittleEndian>(*value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_empty() {
        let ring_buffer = UncompressedRingBuffer::<f32>::with_min_samples(3);
        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            0, 0, // length
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    #[test]
    fn test_ring_buffer_one_item() {
        let mut ring_buffer: UncompressedRingBuffer<f32> =
            UncompressedRingBuffer::<f32>::with_min_samples(3);
        ring_buffer.push(f32::from_bits(0xefcdab89u32));

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            1, 0, // length
            0x89, 0xab, 0xcd, 0xef, // first item
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    #[test]
    fn test_ring_buffer_full() {
        let mut ring_buffer: UncompressedRingBuffer<f32> =
            UncompressedRingBuffer::<f32>::with_min_samples(3);
        ring_buffer.push(f32::from_bits(1u32));
        ring_buffer.push(f32::from_bits(13u32));
        ring_buffer.push(f32::from_bits(37u32));

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            1, 0, 0, 0, // first item
            13, 0, 0, 0, // second item
            37, 0, 0, 0, // third item
        ];
        assert_eq!(&buffer[..], expected_bytes);

        ring_buffer.push(f32::from_bits(42u32));

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            13, 0, 0, 0, // first item
            37, 0, 0, 0, // second item
            42, 0, 0, 0, // third item
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    #[test]
    fn test_ring_buffer_fill() {
        let mut ring_buffer: UncompressedRingBuffer<f32> =
            UncompressedRingBuffer::<f32>::with_min_samples(3);
        ring_buffer.push(f32::from_bits(1u32));
        ring_buffer.push(f32::from_bits(13u32));
        ring_buffer.fill(f32::from_bits(37u32), NonZeroUsize::new(2).unwrap());

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            13, 0, 0, 0, // first item
            37, 0, 0, 0, // second item
            37, 0, 0, 0, // third item
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }

    #[test]
    fn test_ring_buffer_fill_evicts_all_old_values() {
        let mut ring_buffer: UncompressedRingBuffer<f32> =
            UncompressedRingBuffer::<f32>::with_min_samples(3);
        ring_buffer.push(f32::from_bits(1u32));
        ring_buffer.push(f32::from_bits(13u32));
        ring_buffer.fill(f32::from_bits(37u32), NonZeroUsize::new(99999).unwrap());

        let mut buffer = vec![];
        ring_buffer.serialize(&mut buffer).expect("serialize should succeed");
        let expected_bytes = &[
            3, 0, // length
            37, 0, 0, 0, // first item
            37, 0, 0, 0, // second item
            37, 0, 0, 0, // third item
        ];
        assert_eq!(&buffer[..], expected_bytes);
    }
}
