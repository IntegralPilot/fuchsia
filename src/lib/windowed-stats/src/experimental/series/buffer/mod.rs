// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Ring buffers and compression.

mod delta_simple8b_rle;
mod delta_zigzag_simple8b_rle;
mod simple8b_rle;
mod uncompressed;
mod zigzag_simple8b_rle;

pub mod decode;
pub mod encoding;

use log::warn;
use std::fmt::{self, Debug, Display, Formatter};
use std::io::{self, Write};
use std::num::NonZeroUsize;

use crate::experimental::series::SamplingInterval;
use crate::experimental::series::buffer::encoding::Encoding;
use crate::experimental::series::interpolation::InterpolationKind;

use Capacity::MinSamples;

// The choice of ring buffer is determined by three types associated with a time matrix:
//
//   1. The data semantic.
//   2. The aggregation.
//   3. The interpolation.
//
// Because of this, this trait is implemented by `DataSemantic` types like `Gauge` for various
// aggregation and interpolation input types. For each supported triple, there is an associated
// ring buffer. In type bounds, this is typically expressed via the `SerialStatistic` trait, which
// is implemented for any and all `Statistic` types with an associated data semantic type that
// implements `BufferStrategy` for its associated aggregation type. See the blanket implementation
// for `SerialStatistic`.
/// A type that can construct a [`RingBuffer`] associated with an aggregation type and
/// interpolation.
///
/// [`RingBuffer`]: crate::experimental::series::buffer::RingBuffer
pub trait BufferStrategy<A, P>
where
    P: InterpolationKind,
{
    type Buffer: Clone + RingBuffer<A>;

    /// Constructs a ring buffer with the given fixed capacity.
    fn buffer(interval: &SamplingInterval) -> Self::Buffer {
        Self::Buffer::with_capacity(interval.capacity())
    }
}

/// A fixed-capacity circular ring buffer.
pub trait RingBuffer<A> {
    /// The compression and payload of the buffer.
    type Encoding: Encoding<A>;

    fn with_capacity(capacity: Capacity) -> Self
    where
        Self: Sized;

    fn push(&mut self, item: A);

    fn fill(&mut self, item: A, count: NonZeroUsize);

    fn serialize(&self, write: impl Write) -> io::Result<()>;

    // TODO(https://fxbug.dev/369886210): Implement a durability query. This is the duration of the
    //                                    sampling interval (`SamplingInterval::duration`)
    //                                    multiplied by the (approximate) number of samples for
    //                                    which the buffer has capacity.
    //
    // /// Gets the approximate durability of the buffer.
    // ///
    // /// Durability is the maximum period of time represented by the aggregations of a sampling
    // /// interval. This is the time period for which it represents historical data.
    // fn durability(&self) -> Duration;
}

/// Capacity bounds of a [`RingBuffer`].
///
/// Because buffers may be compressed, capacity is expressed in terms of bounds rather than exact
/// quantities.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Capacity {
    /// A lower bound on the number of samples that a buffer stores in memory.
    ///
    /// Given a bound `n`, a buffer allocates enough memory for at least `n` samples, but may store
    /// more.
    MinSamples(NonZeroUsize),
}

impl Capacity {
    /// Constructs a minimum samples capacity, clamping to `[1, usize::MAX]`.
    pub fn from_min_samples(n: usize) -> Self {
        MinSamples(NonZeroUsize::new(n).unwrap_or(NonZeroUsize::MIN))
    }
}

impl Display for Capacity {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            MinSamples(n) => write!(formatter, "{}", n),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Uncompressed<A>(uncompressed::UncompressedRingBuffer<A>);

impl RingBuffer<f32> for Uncompressed<f32> {
    type Encoding = uncompressed::Encoding<f32>;

    fn with_capacity(capacity: Capacity) -> Self {
        Uncompressed(match capacity {
            MinSamples(n) => uncompressed::UncompressedRingBuffer::with_min_samples(n.get()),
        })
    }

    fn push(&mut self, item: f32) {
        self.0.push(item);
    }

    fn fill(&mut self, item: f32, count: NonZeroUsize) {
        self.0.fill(item, count);
    }

    fn serialize(&self, mut write: impl Write) -> io::Result<()> {
        self.0.serialize(&mut write)
    }
}

#[derive(Clone, Debug)]
pub struct Simple8bRle(simple8b_rle::Simple8bRleRingBuffer);

impl<A> RingBuffer<A> for Simple8bRle
where
    A: Into<u64>,
{
    type Encoding = simple8b_rle::Encoding;

    fn with_capacity(capacity: Capacity) -> Self {
        Simple8bRle(match capacity {
            MinSamples(n) => simple8b_rle::Simple8bRleRingBuffer::with_min_samples(n.get()),
        })
    }

    fn push(&mut self, item: A) {
        self.0.push(item.into());
    }

    fn fill(&mut self, item: A, count: NonZeroUsize) {
        self.0.fill(item.into(), count);
    }

    fn serialize(&self, mut write: impl Write) -> io::Result<()> {
        self.0.serialize(&mut write)
    }
}

/// A ring buffer that encodes signed integer items using Zigzag, Simple8B, and RLE compression.
#[derive(Clone, Debug)]
pub struct ZigzagSimple8bRle(zigzag_simple8b_rle::ZigzagSimple8bRleRingBuffer);

impl<A> RingBuffer<A> for ZigzagSimple8bRle
where
    A: Into<i64>,
{
    type Encoding = zigzag_simple8b_rle::Encoding;

    fn with_capacity(capacity: Capacity) -> Self {
        ZigzagSimple8bRle(match capacity {
            MinSamples(n) => {
                zigzag_simple8b_rle::ZigzagSimple8bRleRingBuffer::with_min_samples(n.get())
            }
        })
    }

    fn push(&mut self, item: A) {
        self.0.push(item.into());
    }

    fn fill(&mut self, item: A, count: NonZeroUsize) {
        self.0.fill(item.into(), count);
    }

    fn serialize(&self, mut write: impl Write) -> io::Result<()> {
        self.0.serialize(&mut write)
    }
}

#[derive(Clone, Debug)]
pub struct DeltaSimple8bRle(delta_simple8b_rle::DeltaSimple8bRleRingBuffer);

impl<A> RingBuffer<A> for DeltaSimple8bRle
where
    A: Into<u64>,
{
    type Encoding = delta_simple8b_rle::Encoding;

    fn with_capacity(capacity: Capacity) -> Self {
        let ring_buffer = match capacity {
            MinSamples(n) => {
                delta_simple8b_rle::DeltaSimple8bRleRingBuffer::with_min_samples(n.get())
            }
        };
        DeltaSimple8bRle(ring_buffer)
    }

    fn push(&mut self, item: A) {
        if let Err(e) = self.0.push(item.into()) {
            warn!("DeltaSimple8bRleRingBuffer::push error: {}", e);
        }
    }

    fn fill(&mut self, item: A, count: NonZeroUsize) {
        if let Err(e) = self.0.fill(item.into(), count) {
            warn!("DeltaSimple8bRleRingBuffer::fill error: {}", e);
        }
    }

    fn serialize(&self, mut write: impl Write) -> io::Result<()> {
        self.0.serialize(&mut write)
    }
}

#[derive(Clone, Debug)]
pub struct DeltaZigzagSimple8bRle<A>(
    delta_zigzag_simple8b_rle::DeltaZigzagSimple8bRleRingBuffer<A>,
);

impl RingBuffer<i64> for DeltaZigzagSimple8bRle<i64> {
    type Encoding = delta_zigzag_simple8b_rle::Encoding<i64>;

    fn with_capacity(capacity: Capacity) -> Self {
        let ring_buffer = match capacity {
            MinSamples(n) => {
                delta_zigzag_simple8b_rle::DeltaZigzagSimple8bRleRingBuffer::with_min_samples(
                    n.get(),
                )
            }
        };
        DeltaZigzagSimple8bRle(ring_buffer)
    }

    fn push(&mut self, item: i64) {
        self.0.push(item)
    }

    fn fill(&mut self, item: i64, count: NonZeroUsize) {
        self.0.fill(item, count);
    }

    fn serialize(&self, mut write: impl Write) -> io::Result<()> {
        self.0.serialize(&mut write)
    }
}

impl RingBuffer<u64> for DeltaZigzagSimple8bRle<u64> {
    type Encoding = delta_zigzag_simple8b_rle::Encoding<u64>;

    fn with_capacity(capacity: Capacity) -> Self {
        let ring_buffer = match capacity {
            MinSamples(n) => {
                delta_zigzag_simple8b_rle::DeltaZigzagSimple8bRleRingBuffer::with_min_samples(
                    n.get(),
                )
            }
        };
        DeltaZigzagSimple8bRle(ring_buffer)
    }

    fn push(&mut self, item: u64) {
        self.0.push(item)
    }

    fn fill(&mut self, item: u64, count: NonZeroUsize) {
        self.0.fill(item, count);
    }

    fn serialize(&self, mut write: impl Write) -> io::Result<()> {
        self.0.serialize(&mut write)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncompressed_buffer() {
        let mut buffer =
            <Uncompressed<f32> as RingBuffer<f32>>::with_capacity(Capacity::from_min_samples(2));
        buffer.push(22f32);
        let mut data = vec![];
        let result = RingBuffer::<f32>::serialize(&buffer, &mut data);
        assert!(result.is_ok());
        assert!(!data.is_empty());
    }

    #[test]
    fn simple8b_rle_buffer() {
        let mut buffer =
            <Simple8bRle as RingBuffer<u64>>::with_capacity(Capacity::from_min_samples(2));
        buffer.push(22u64);
        let mut data = vec![];
        let result = RingBuffer::<u64>::serialize(&buffer, &mut data);
        assert!(result.is_ok());
        assert!(!data.is_empty());
    }

    #[test]
    fn zigzag_simple8b_rle_buffer() {
        let mut buffer =
            <ZigzagSimple8bRle as RingBuffer<i64>>::with_capacity(Capacity::from_min_samples(2));
        buffer.push(22i64);
        let mut data = vec![];
        let result = RingBuffer::<i64>::serialize(&buffer, &mut data);
        assert!(result.is_ok());
        assert!(!data.is_empty());
    }

    #[test]
    fn delta_simple8b_rle_buffer() {
        let mut buffer =
            <DeltaSimple8bRle as RingBuffer<u64>>::with_capacity(Capacity::from_min_samples(2));
        buffer.push(22u64);
        buffer.push(30u64);
        let mut data = vec![];
        let result = RingBuffer::<u64>::serialize(&buffer, &mut data);
        assert!(result.is_ok());
        assert!(!data.is_empty());
    }

    #[test]
    fn delta_zigzag_simple8b_rle_buffer_i64() {
        let mut buffer = <DeltaZigzagSimple8bRle<i64> as RingBuffer<i64>>::with_capacity(
            Capacity::from_min_samples(2),
        );
        buffer.push(22i64);
        buffer.push(-1i64);
        let mut data = vec![];
        let result = RingBuffer::<i64>::serialize(&buffer, &mut data);
        assert!(result.is_ok());
        assert!(!data.is_empty());
    }

    #[test]
    fn delta_zigzag_simple8b_rle_buffer_u64() {
        let mut buffer = <DeltaZigzagSimple8bRle<u64> as RingBuffer<u64>>::with_capacity(
            Capacity::from_min_samples(2),
        );
        buffer.push(22u64);
        buffer.push(u64::MAX);
        let mut data = vec![];
        let result = RingBuffer::<u64>::serialize(&buffer, &mut data);
        assert!(result.is_ok());
        assert!(!data.is_empty());
    }
}
