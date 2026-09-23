// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::convert::Infallible;
use std::iter;
use std::marker::PhantomData;
use std::num::NonZeroU8;

use byteorder::{LittleEndian, ReadBytesExt as _};
use itertools::Itertools;
use thiserror::Error;

use crate::experimental::clock::{Duration, Timestamp, TimestampExt as _};
use crate::experimental::series::SerializedBuffer;
use crate::experimental::series::buffer::simple8b_rle::SIMPLE8B_SELECTOR_BIT_COUNTS;

const BLOCK_LENGTH: usize = std::mem::size_of::<u64>();
const BITS_PER_SELECTOR: NonZeroU8 = NonZeroU8::new(4).unwrap();

/// A struct for decoding serialized data by this crate.
#[derive(Debug)]
pub struct Decoder<'a> {
    pub semantic: &'a str,
    pub created_timestamp: Timestamp,
    pub end_timestamp: Timestamp,
    pub series_type: SeriesType,
    data: &'a [u8],
}

impl<'a> Decoder<'a> {
    pub fn from_serialized_buffer(buffer: &'a SerializedBuffer) -> Result<Self, DecodeError> {
        Self::new(buffer.data.as_slice(), &buffer.data_semantic)
    }

    pub fn new(mut data: &'a [u8], semantic: &'a str) -> Result<Self, DecodeError> {
        let version = data.read_u8()?;
        if version != 1 {
            return Err(DecodeError::UnknownVersion(version));
        }
        let created_timestamp =
            Timestamp::from_quanta(data.read_u32::<LittleEndian>()?.try_into()?);
        let end_timestamp = Timestamp::from_quanta(data.read_u32::<LittleEndian>()?.try_into()?);

        let series_type = SeriesType::decode(&mut data)?;
        Ok(Self { semantic, created_timestamp, end_timestamp, series_type, data })
    }

    pub fn iter_series(&self) -> TimeSeriesIterator<'a> {
        TimeSeriesIterator { data: self.data, series_type: self.series_type }
    }
}

pub struct TimeSeriesIterator<'a> {
    data: &'a [u8],
    series_type: SeriesType,
}

impl<'a> TimeSeriesIterator<'a> {
    fn try_next_series(&mut self) -> Result<TimeSeries<'a>, DecodeError> {
        let Self { data, series_type } = self;
        let len = usize::from(data.read_u16::<LittleEndian>()?);
        if data.len() < len {
            return Err(DecodeError::ShortBuffer);
        }
        let granularity = Duration::from_seconds(data.read_u16::<LittleEndian>()?.into());
        // Can't underflow given we checked against data length above.
        let len = len.checked_sub(std::mem::size_of::<u16>()).unwrap();
        let (mut time_series, rest) = data.split_at(len);
        *data = rest;
        let series_data = SeriesData::decode(&mut time_series, *series_type)?;
        Ok(TimeSeries { granularity, data: time_series, series_data })
    }
}

impl<'a> Iterator for TimeSeriesIterator<'a> {
    type Item = Result<TimeSeries<'a>, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.data.is_empty() {
            return None;
        }
        let r = self.try_next_series();
        if r.is_err() {
            // Return None from now on.
            self.data = &[];
        }
        Some(r)
    }
}

pub struct TimeSeries<'a> {
    /// Time series granularity.
    pub granularity: Duration,
    series_data: SeriesData<'a>,
    data: &'a [u8],
}

impl<'a> TimeSeries<'a> {
    pub fn data_points<D: DataPoint>(&self) -> impl Iterator<Item = Result<D, DecodeError>> + 'a {
        let Self { granularity: _, series_data, data } = self;
        match series_data {
            SeriesData::Simple8bRle(i) => {
                CompressedBlocksIterator::from_simple8b_rle_info(data, i).data_points()
            }
        }
    }

    pub fn data_points_64(&self) -> impl Iterator<Item = Result<u64, DecodeError>> + 'a {
        self.data_points::<u64>()
    }
}

pub trait DataPoint: Sized + 'static {
    fn from_u64(v: u64) -> Result<Self, DecodeError>;
}

impl DataPoint for u64 {
    fn from_u64(v: u64) -> Result<Self, DecodeError> {
        Ok(v)
    }
}

struct CompressedBlocksIterator<'a, D> {
    data: &'a [u8],
    last_block_num_values: u8,
    selectors: iter::Take<iter::Skip<BitsIterator<'a>>>,
    _marker: PhantomData<D>,
}

impl<'a, D> CompressedBlocksIterator<'a, D> {
    fn from_simple8b_rle_info(data: &'a [u8], info: &Simple8bRleInfo<'a>) -> Self {
        let Simple8bRleInfo {
            selectors,
            num_selectors,
            index_to_head_selector,
            last_block_num_values,
        } = info;
        // Selectors are 4 bits each.
        let selectors = BitsIterator::new(*selectors, BITS_PER_SELECTOR)
            .skip(*index_to_head_selector)
            .take(*num_selectors);

        Self {
            data,
            last_block_num_values: *last_block_num_values,
            selectors,
            _marker: PhantomData,
        }
    }

    fn try_next_block(&mut self) -> Result<Option<CompressedBlock<'a, D>>, DecodeError> {
        let Self { data, last_block_num_values, selectors, _marker } = self;
        let Some(selector) = selectors.next() else {
            return Ok(None);
        };
        // The bad type here is an artifact of reading u64s from `BitsIterator`,
        // we know this can't overflow because we create the selector
        // BitsIterator with BITS_PER_SELECTOR.
        static_assertions::const_assert!(BITS_PER_SELECTOR.get() as u32 <= u8::BITS);
        let selector = u8::try_from(selector).expect("selector overflow");
        let reader = CompressedBlockDataReader::decode(data, selector)?;
        // Decoding the reader consumes from the data slice. If we get an empty
        // slice, it means we're yielding the last block and should perhaps
        // apply the last block limit.
        let take = (data.is_empty() && reader.should_limit_last_block())
            .then(|| (*last_block_num_values).into());
        Ok(Some(CompressedBlock { reader, take, _marker: PhantomData }))
    }
}

impl<'a, D: DataPoint> CompressedBlocksIterator<'a, D> {
    fn data_points(self) -> impl Iterator<Item = Result<D, DecodeError>> + 'a {
        self.flatten_ok().map(|r| r.and_then(|r| r))
    }
}

impl<'a, D> Iterator for CompressedBlocksIterator<'a, D> {
    type Item = Result<CompressedBlock<'a, D>, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.data.is_empty() {
            return None;
        }
        let r = self.try_next_block();
        if r.is_err() {
            // Return None from now on.
            self.data = &[];
        }
        r.transpose()
    }
}

struct AlignedReader<'a, T> {
    data: &'a [u8],
    _marker: PhantomData<T>,
}

impl<'a, T> AlignedReader<'a, T> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, _marker: PhantomData }
    }
}

impl<T: AlignedDataPoint> Iterator for AlignedReader<'_, T> {
    type Item = u64;

    fn next(&mut self) -> Option<Self::Item> {
        let Self { data, _marker } = self;
        T::read(data).ok()
    }
}

trait AlignedDataPoint {
    fn read(data: &mut &[u8]) -> Result<u64, DecodeError>;
}

impl AlignedDataPoint for u8 {
    fn read(data: &mut &[u8]) -> Result<u64, DecodeError> {
        Ok(data.read_u8().map(u64::from)?)
    }
}

impl AlignedDataPoint for u16 {
    fn read(data: &mut &[u8]) -> Result<u64, DecodeError> {
        Ok(data.read_u16::<LittleEndian>().map(u64::from)?)
    }
}

impl AlignedDataPoint for u32 {
    fn read(data: &mut &[u8]) -> Result<u64, DecodeError> {
        Ok(data.read_u32::<LittleEndian>().map(u64::from)?)
    }
}

impl AlignedDataPoint for u64 {
    fn read(data: &mut &[u8]) -> Result<u64, DecodeError> {
        Ok(data.read_u64::<LittleEndian>()?)
    }
}

struct RleReader {
    value: u64,
    repeat: u64,
}

impl RleReader {
    fn new(mut data: &[u8]) -> Result<Self, DecodeError> {
        let data = data.read_u64::<LittleEndian>()?;
        // RLE stores a 6-byte, little-endian integer followed by a 2-byte
        // length.
        let value = data & 0x0000ffffffffffff;
        let repeat = data >> (6 * 8);
        Ok(Self { value, repeat })
    }
}

impl Iterator for RleReader {
    type Item = u64;

    fn next(&mut self) -> Option<Self::Item> {
        let Self { value, repeat } = self;
        let n = repeat.checked_sub(1)?;
        *repeat = n;
        Some(*value)
    }
}

enum CompressedBlockDataReader<'a> {
    U8(AlignedReader<'a, u8>),
    U16(AlignedReader<'a, u16>),
    U32(AlignedReader<'a, u32>),
    U64(AlignedReader<'a, u64>),
    Bits(BitsIterator<'a>),
    Rle(RleReader),
}

impl<'a> CompressedBlockDataReader<'a> {
    fn decode(data: &mut &'a [u8], selector: u8) -> Result<Self, DecodeError> {
        // The following table describes the meaning of each selector for
        // Simple8bRle:
        //
        // ```
        // Selector value:  0  1  2  3  4  5  6  7  8  9 10 11 12 13 14 | 15 (RLE)
        // Integers coded: 64 32 21 16 12 10  9  8  7  6  5  4  3  2  1 | up to 2^16
        // ```

        if data.len() < BLOCK_LENGTH {
            return Err(DecodeError::ShortBuffer);
        }
        let (block, rest) = data.split_at(BLOCK_LENGTH);
        *data = rest;

        if selector == 15 {
            return Ok(Self::Rle(RleReader::new(block)?));
        }
        let bits_per_int = *SIMPLE8B_SELECTOR_BIT_COUNTS
            .get(usize::from(selector))
            .ok_or_else(|| DecodeError::UnsupportedSelector(selector))?;

        match bits_per_int {
            // Selector 7 (0x07): 8 x 8-bit integers.
            u8::BITS => Ok(Self::U8(AlignedReader::new(block))),
            // Selector 11 (0x0b): 4 x 16-bit integers.
            u16::BITS => Ok(Self::U16(AlignedReader::new(block))),
            // Selector 13 (0x0d): 2 x 32-bit integers.
            u32::BITS => Ok(Self::U32(AlignedReader::new(block))),
            // Selector 14 (0x0e): 1 x 64-bit integers.
            u64::BITS => Ok(Self::U64(AlignedReader::new(block))),
            b => Ok(Self::Bits(BitsIterator::new(
                block,
                NonZeroU8::new(b.try_into().expect("bits per int overflow"))
                    .expect("zero bits iterator"),
            ))),
        }
    }

    fn should_limit_last_block(&self) -> bool {
        match self {
            // Rle carries the length with it we don't need to limit.
            Self::Rle(_) => false,
            _ => true,
        }
    }
}

impl<'a> Iterator for CompressedBlockDataReader<'a> {
    type Item = u64;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::U8(r) => r.next(),
            Self::U16(r) => r.next(),
            Self::U32(r) => r.next(),
            Self::U64(r) => r.next(),
            Self::Bits(r) => r.next(),
            Self::Rle(r) => r.next(),
        }
    }
}

struct CompressedBlock<'a, D> {
    reader: CompressedBlockDataReader<'a>,
    take: Option<usize>,
    _marker: PhantomData<D>,
}

impl<'a, D: DataPoint> Iterator for CompressedBlock<'a, D> {
    type Item = Result<D, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        let Self { reader, take, _marker } = self;
        if let Some(take) = take {
            *take = take.checked_sub(1)?;
        }
        reader.next().map(D::from_u64)
    }
}

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum SeriesType {
    Simple8bRle,
}

impl SeriesType {
    fn decode(data: &mut &[u8]) -> Result<Self, DecodeError> {
        let series_type = data.read_u8()?;
        let series_subtype = data.read_u8()?;
        match (series_type, series_subtype) {
            (1, 0) => Ok(SeriesType::Simple8bRle),
            // TODO(https://fxbug.dev/460161247): Support remaining series
            // types.
            (t, st) => Err(DecodeError::UnsupportedSeriesType(t, st)),
        }
    }
}

/// Supported decoding compression series types.
#[derive(Debug, Copy, Clone)]
enum SeriesData<'a> {
    Simple8bRle(Simple8bRleInfo<'a>),
}

impl<'a> SeriesData<'a> {
    fn decode(data: &mut &'a [u8], series_type: SeriesType) -> Result<Self, DecodeError> {
        match series_type {
            SeriesType::Simple8bRle => Simple8bRleInfo::decode(data).map(SeriesData::Simple8bRle),
        }
    }
}

#[derive(Copy, Clone, Debug)]
struct Simple8bRleInfo<'a> {
    selectors: &'a [u8],
    num_selectors: usize,
    index_to_head_selector: usize,
    last_block_num_values: u8,
}

impl<'a> Simple8bRleInfo<'a> {
    fn decode(data: &mut &'a [u8]) -> Result<Self, DecodeError> {
        let num_selectors = usize::from(data.read_u16::<LittleEndian>()?);
        let index_to_head_selector = usize::from(data.read_u16::<LittleEndian>()?);
        let last_block_num_values = data.read_u8()?;

        // Each selector is 4 bits. When `numSelectors + indexToHeadSelector` is
        // odd, the last half-byte will be filled with 4 empty bits.
        let selector_bytes = (num_selectors + index_to_head_selector + 1) / 2;
        let (selectors, rest) = data.split_at(selector_bytes);
        *data = rest;

        let want_len = num_selectors * BLOCK_LENGTH;
        if data.len() != want_len {
            return Err(DecodeError::UnexpectedBlocksLength { want: want_len, got: data.len() });
        }

        Ok(Self { selectors, num_selectors, index_to_head_selector, last_block_num_values })
    }
}

#[derive(Error, Debug)]
pub enum DecodeError {
    #[error("unknown version {0}")]
    UnknownVersion(u8),
    #[error("unsupported series type {0} {1}")]
    UnsupportedSeriesType(u8, u8),
    #[error("unsupported selector {0}")]
    UnsupportedSelector(u8),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("data buffer cut off")]
    ShortBuffer,
    #[error("unexpected block length {got}, want: {want}")]
    UnexpectedBlocksLength { want: usize, got: usize },
    #[error(transparent)]
    Other(anyhow::Error),
}

impl From<Infallible> for DecodeError {
    fn from(v: Infallible) -> Self {
        match v {}
    }
}

#[derive(Copy, Clone)]
struct BitsIterator<'a> {
    data: &'a [u8],
    bits_per_int: NonZeroU8,
    current_bit: u8,
}

impl<'a> BitsIterator<'a> {
    fn new(data: &'a [u8], bits_per_int: NonZeroU8) -> Self {
        assert!(u32::from(bits_per_int.get()) <= u64::BITS);
        Self { data, bits_per_int, current_bit: 0 }
    }
}

impl<'a> Iterator for BitsIterator<'a> {
    type Item = u64;

    fn next(&mut self) -> Option<Self::Item> {
        let Self { data, bits_per_int, current_bit } = self;

        // The current integer being reconstructed.
        let mut value: u64 = 0;
        // Bits remaining to complete the current integer.
        let mut bits_remaining = bits_per_int.get();

        loop {
            let current_byte = data.first()?;

            // Calculate bits available in the current byte.
            let bits_available = 8 - *current_bit;
            let bits_to_read = std::cmp::min(bits_remaining, bits_available);

            // Calculate the mask for the current byte.
            let mask = (0xff >> (8 - bits_to_read - *current_bit)) & (0xff << *current_bit);

            // 1. Mask off new bits.
            // 2. Shift right to align them to 0.
            // 3. Cast to u64 BEFORE shifting left to their final position in
            //    'value'.
            let extracted_bits = (*current_byte & mask) >> *current_bit;
            value |= u64::from(extracted_bits) << (bits_per_int.get() - bits_remaining);

            // Update accounting.
            bits_remaining -= bits_to_read;
            *current_bit += bits_to_read;

            if *current_bit == 8 {
                *current_bit = 0;
                *data = data.split_at(1).1;
            }

            if bits_remaining == 0 {
                return Some(value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use super::*;

    use proptest::prop_assert_eq;
    use proptest::strategy::Strategy;
    use proptest::test_runner::TestCaseError;

    use crate::experimental::clock::{Timed, Timestamp};
    use crate::experimental::series::buffer::simple8b_rle::{
        Simple8bRleBlock, Simple8bRleRingBuffer, simple8b_block_from_values,
    };
    use crate::experimental::series::interpolation::ConstantSample;
    use crate::experimental::series::statistic::Last;
    use crate::experimental::series::{
        Capacity, SamplingProfile, TimeMatrix, TimeMatrixFold, TimeMatrixTick,
    };

    #[test]
    fn decode_simple8b_rle_time_series() {
        let profile = SamplingProfile::balanced();

        let intervals = profile.clone().into_sampling_intervals();
        let granular_profile = intervals.iter().min_by_key(|x| x.max_sampling_period()).unwrap();
        let granularity = granular_profile.max_sampling_period();
        let samples = match granular_profile.capacity() {
            Capacity::MinSamples(n) => n.get(),
        };

        let start = Timestamp::from_nanos(0);
        let mut time = start;

        let mut series =
            TimeMatrix::<Last<u64>, ConstantSample>::new_at(time, profile, Default::default());

        let mut value = 1;
        let mut values = Vec::new();
        for _ in 0..samples {
            series.fold(Timed::at(value, time)).expect("fold sample");
            values.push(value);
            time += granularity;
            value += 1
        }
        let end = time;

        let buffer = series.tick_and_get_buffers(end).expect("tick");
        let decoder = Decoder::from_serialized_buffer(&buffer).expect("decode");
        let Decoder { semantic, created_timestamp, end_timestamp, series_type, data: _ } = &decoder;
        assert_eq!(semantic, &buffer.data_semantic);
        assert_eq!(created_timestamp, &start);
        assert_eq!(end_timestamp, &end);
        assert_eq!(series_type, &SeriesType::Simple8bRle);

        let duration = end - start;

        // zip_eq panics if the iterators are not the same size.
        let series = decoder.iter_series().zip_eq(intervals);
        for (series, interval) in series {
            let series = series.expect("error decoding series");
            assert_eq!(series.granularity, interval.max_sampling_period());
            // Now for checking the data.
            let data_points =
                series.data_points_64().collect::<Result<Vec<_>, _>>().expect("decode time series");
            let expect_samples = usize::try_from(
                duration.into_seconds() / interval.max_sampling_period().into_seconds(),
            )
            .unwrap();
            assert_eq!(data_points.len(), expect_samples);
        }

        // Verify the data points from the first series.
        let first_series = decoder
            .iter_series()
            .next()
            .expect("has first series")
            .expect("can decode first series");
        let samples = first_series
            .data_points_64()
            .collect::<Result<Vec<_>, _>>()
            .expect("decode time series");
        assert_eq!(samples, values);
    }

    const TEST_MIN_SAMPLES: usize = 500;

    fn decode_simple8b_rle_inner(samples: Vec<u64>) -> Result<(), TestCaseError> {
        let mut buffer = Simple8bRleRingBuffer::with_min_samples(TEST_MIN_SAMPLES);
        for i in samples.iter() {
            let evicted = buffer.push(*i);
            // Don't lose any data for the test.
            prop_assert_eq!(evicted, vec![]);
        }

        let mut serialized = Vec::new();
        buffer.serialize(&mut serialized).prop_context("serialize")?;
        let mut serialized = &serialized[..];

        let info = Simple8bRleInfo::decode(&mut serialized).prop_context("decode info")?;
        let deserialized =
            CompressedBlocksIterator::<'_, u64>::from_simple8b_rle_info(serialized, &info)
                .data_points()
                .collect::<Result<Vec<_>, _>>()
                .prop_context("data points")?;
        prop_assert_eq!(deserialized, samples);
        Ok(())
    }

    fn u64_samples_strategy() -> impl Strategy<Value = Vec<u64>> {
        (0..TEST_MIN_SAMPLES, 0..=u64::BITS).prop_flat_map(|(len, bits)| {
            let max = if bits == u64::BITS { u64::MAX } else { (1 << bits) - 1 };
            proptest::collection::vec(0..=max, len)
        })
    }

    trait ResultExt<T> {
        fn prop_context(self, context: &str) -> Result<T, TestCaseError>;
    }

    impl<T, E: Debug> ResultExt<T> for Result<T, E> {
        fn prop_context(self, context: &str) -> Result<T, TestCaseError> {
            self.map_err(|e| TestCaseError::fail(format!("{context}: {e:?}")))
        }
    }

    fn bits_iterator_inner(
        mut offset: u64,
        simple8b_rle_selector: u8,
    ) -> Result<(), TestCaseError> {
        let bits = SIMPLE8B_SELECTOR_BIT_COUNTS[usize::from(simple8b_rle_selector)];
        let mask = if bits == u64::BITS { u64::MAX } else { (1u64 << bits) - 1 };

        let generator = std::iter::repeat_with(move || {
            let nxt = offset;
            offset += 1;
            nxt & mask
        });

        let Simple8bRleBlock { selector: _, data } =
            simple8b_block_from_values(simple8b_rle_selector, &mut generator.clone());
        let num_values = u64::BITS / bits;

        let expect = generator.take(num_values.try_into().unwrap()).collect::<Vec<_>>();
        let data = data.to_le_bytes();

        let values =
            BitsIterator::new(&data[..], NonZeroU8::new(bits.try_into().unwrap()).unwrap())
                .collect::<Vec<_>>();
        prop_assert_eq!(values, expect);
        Ok(())
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            // Add all failed seeds here.
            failure_persistence: proptest_support::failed_seeds!(
                "cc b37d7d02c5bf8547fef78e979a7e64b86ed03eb9b77e2e16770bd96b1deed5d7"
            ),
            ..proptest::test_runner::Config::default()
        })]

        #[test]
        fn decode_simple8b_rle(x in u64_samples_strategy()) {
            decode_simple8b_rle_inner(x)?;
        }

        #[test]
        fn bits_iterator((offset, selector) in (0u64..100, 0..(SIMPLE8B_SELECTOR_BIT_COUNTS.len()) )) {
            bits_iterator_inner(offset, selector.try_into().unwrap())?;
        }

    }
}
