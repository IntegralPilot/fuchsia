// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Round-robin multi-resolution time series.

mod interval;

pub(crate) mod buffer;

pub mod interpolation;
pub mod metadata;
pub mod statistic;

use derivative::Derivative;
use std::fmt::{Debug, Display};
use std::io;
use std::marker::PhantomData;
use std::num::NonZeroUsize;

use crate::experimental::Vec1;
use crate::experimental::clock::{ObservationTime, Tick, Timed, Timestamp, TimestampExt};
use crate::experimental::series::buffer::{
    BufferStrategy, DeltaSimple8bRle, DeltaZigzagSimple8bRle, RingBuffer, Simple8bRle,
    Uncompressed, ZigzagSimple8bRle, encoding,
};
use crate::experimental::series::interpolation::{
    ConstantSample, Interpolation, InterpolationKind, LastSample,
};
use crate::experimental::series::metadata::{BitsetIndex, Metadata};
use crate::experimental::series::statistic::{
    FoldError, PostAggregation, SerialStatistic, Statistic,
};

pub use crate::experimental::series::buffer::{Capacity, decode};
pub use crate::experimental::series::interval::{SamplingInterval, SamplingProfile};

/// A [`TimeMatrix`] type that can be advanced forward in time.
///
/// This trait provides tick operations for `[TimeMatrix`] types. Ticking a [`TimeMatrix`] causes
/// sample interpolation within and aggregation propagation across [`SamplingInterval`]s.
///
/// Importantly, this trait is `dyn` compatible and type erased; it can be used to tick a
/// [`TimeMatrix`] regardless of its input type parameters (sample type, interpolation type, etc.).
///
/// See also the [`TimeMatrixFold`] subtrait.
pub trait TimeMatrixTick {
    fn tick(&mut self, timestamp: Timestamp) -> Result<(), FoldError>;

    fn tick_and_get_buffers(&mut self, timestamp: Timestamp)
    -> Result<SerializedBuffer, FoldError>;
}

/// A [`TimeMatrix`] type that can sample data.
///
/// This trait provides fold operations for `TimeMatrix` types. Folding samples updates
/// aggregations and advances a [`TimeMatrix`] forward in time.
///
/// See also the [`TimeMatrixTick`] supertrait. This trait supports both ticking and sampling, but
/// is not completely type erased: the sample input type parameter `T` is needed.
pub trait TimeMatrixFold<T>: TimeMatrixTick {
    fn fold(&mut self, sample: Timed<T>) -> Result<(), FoldError>;
}

/// A type that describes the semantics of data folded by `Sampler`s.
///
/// Data semantics determine how statistics are interpreted and time series are aggregated and
/// buffered.
pub trait DataSemantic {
    type Metadata: Metadata;

    fn display() -> impl Display;
}

/// A continually increasing value.
///
/// Counters are analogous to an odometer in a vehicle.
#[derive(Debug)]
pub enum Counter {}

impl BufferStrategy<u64, LastSample> for Counter {
    type Buffer = DeltaSimple8bRle;
}

impl DataSemantic for Counter {
    type Metadata = ();

    fn display() -> impl Display {
        "counter"
    }
}

/// A fluctuating value.
///
/// Gauges are analogous to a speedometer in a vehicle.
#[derive(Debug)]
pub enum Gauge {}

impl<P> BufferStrategy<f32, P> for Gauge
where
    P: InterpolationKind,
{
    type Buffer = Uncompressed<f32>;
}

impl BufferStrategy<i64, ConstantSample> for Gauge {
    type Buffer = ZigzagSimple8bRle;
}

impl BufferStrategy<i64, LastSample> for Gauge {
    type Buffer = DeltaZigzagSimple8bRle<i64>;
}

impl BufferStrategy<u64, ConstantSample> for Gauge {
    type Buffer = Simple8bRle;
}

impl BufferStrategy<u64, LastSample> for Gauge {
    type Buffer = DeltaZigzagSimple8bRle<u64>;
}

impl DataSemantic for Gauge {
    type Metadata = ();

    fn display() -> impl Display {
        "gauge"
    }
}

/// A semantic like `Gauge` that avoids [`DeltaZigzagSimple8bRle`] until we fix
/// some other issues.
///
/// TODO(https://fxbug.dev/436253782): Delete this type when the viewer can
/// decode `DeltaZigzagSimple8bRle`.
///
/// OR
///
/// TODO(https://fxbug.dev/457443158): Delete this type when
/// `ConstantAggregation` is introduced and netstack's time series is changed to
/// `TimeMatrix<Diff<u64>, ConstantAggregation>`.
///
/// Whichever happens first.
pub enum GaugeForceSimple8bRle {}

impl<T: Into<u64>> BufferStrategy<T, LastSample> for GaugeForceSimple8bRle {
    type Buffer = Simple8bRle;
}

impl DataSemantic for GaugeForceSimple8bRle {
    type Metadata = ();

    fn display() -> impl Display {
        "gauge"
    }
}

/// A set of Boolean values.
///
/// Bitsets are analogous to indicator lamps in a vehicle.
#[derive(Debug)]
pub enum Bitset {}

impl<A, P> BufferStrategy<A, P> for Bitset
where
    Simple8bRle: RingBuffer<A>,
    P: InterpolationKind,
{
    type Buffer = Simple8bRle;
}

impl DataSemantic for Bitset {
    type Metadata = BitsetIndex;

    fn display() -> impl Display {
        "bitset"
    }
}

/// A buffer of serialized data from a time series.
#[derive(Clone, Debug)]
struct SerializedTimeSeries {
    interval: SamplingInterval,
    data: Vec<u8>,
}

impl SerializedTimeSeries {
    /// Gets the sampling interval for the aggregations in the buffer.
    pub fn interval(&self) -> &SamplingInterval {
        &self.interval
    }
}

/// An unbuffered statistical time series specification.
///
/// This type samples and interpolates timed data and produces aggregations per its statistic and
/// sampling interval. It is a specification insofar that it does **not** buffer the series of
/// aggregations.
#[derive(Clone, Debug)]
struct TimeSeries<F>
where
    F: Statistic,
{
    interval: SamplingInterval,
    statistic: F,
}

impl<F> TimeSeries<F>
where
    F: Statistic,
{
    pub fn new(interval: SamplingInterval) -> Self
    where
        F: Default,
    {
        TimeSeries { interval, statistic: F::default() }
    }

    pub const fn with_statistic(interval: SamplingInterval, statistic: F) -> Self {
        TimeSeries { interval, statistic }
    }

    /// Folds interpolations for intervals intersected by the given [`Tick`] and gets the
    /// aggregations.
    ///
    /// The returned iterator performs the computation and so it must be consumed to change the
    /// state of the statistic.
    ///
    /// [`Tick`]: crate::experimental::clock::Tick
    #[must_use]
    fn interpolate_and_get_aggregations<'i, P>(
        &'i mut self,
        interpolation: &'i mut P,
        tick: Tick,
    ) -> impl 'i + Iterator<Item = Result<(NonZeroUsize, F::Aggregation), FoldError>>
    where
        P: Interpolation<F::Sample>,
    {
        self.interval.fold_and_get_expirations(tick, PhantomData::<F::Sample>).flat_map(
            move |expiration| {
                expiration
                    .interpolate_and_get_aggregation(&mut self.statistic, interpolation)
                    .transpose()
            },
        )
    }

    /// Folds the given sample and interpolations for intervals intersected by the given [`Tick`]
    /// and gets the aggregations.
    ///
    /// The returned iterator performs the computation and so it must be consumed to change the
    /// state of the statistic.
    ///
    /// [`Tick`]: crate::experimental::clock::Tick
    #[must_use]
    fn fold_and_get_aggregations<'i, P>(
        &'i mut self,
        interpolation: &'i mut P,
        tick: Tick,
        sample: F::Sample,
    ) -> impl 'i + Iterator<Item = Result<(NonZeroUsize, F::Aggregation), FoldError>>
    where
        P: Interpolation<F::Sample>,
    {
        self.interval.fold_and_get_expirations(tick, sample).flat_map(move |expiration| {
            expiration.fold_and_get_aggregation(&mut self.statistic, interpolation).transpose()
        })
    }

    /// Gets the sampling interval of the series.
    pub fn interval(&self) -> &SamplingInterval {
        &self.interval
    }
}

impl<F, R, A> TimeSeries<PostAggregation<F, R>>
where
    F: Default + Statistic,
    R: Clone + Fn(F::Aggregation) -> A,
    A: Clone,
{
    pub fn with_transform(interval: SamplingInterval, transform: R) -> Self {
        TimeSeries { interval, statistic: PostAggregation::from_transform(transform) }
    }
}

/// A buffered round-robin statistical time series.
///
/// This type composes a [`TimeSeries`] with a round-robin buffer of aggregations and interpolation
/// state. Aggregations produced by the time series when sampling or interpolating are pushed into
/// the buffer.
#[derive(Derivative)]
#[derivative(
    Clone(bound = "F: Clone, F::Buffer: Clone, P::Output<F::Sample>: Clone,"),
    Debug(bound = "F: Debug,
                   F::Buffer: Debug,
                   P::Output<F::Sample>: Debug,")
)]
struct BufferedTimeSeries<F, P>
where
    F: SerialStatistic<P>,
    P: InterpolationKind,
{
    buffer: F::Buffer,
    interpolation: P::Output<F::Sample>,
    series: TimeSeries<F>,
}

impl<F, P> BufferedTimeSeries<F, P>
where
    F: SerialStatistic<P>,
    P: InterpolationKind,
{
    pub fn new(interpolation: P::Output<F::Sample>, series: TimeSeries<F>) -> Self {
        let buffer = F::buffer(&series.interval);
        BufferedTimeSeries { buffer, interpolation, series }
    }

    /// Folds interpolations for intervals intersected by the given [`Tick`] and buffers the
    /// aggregations.
    ///
    /// # Errors
    ///
    /// Returns an error if sampling fails.
    ///
    /// [`Tick`]: crate::experimental::clock::Tick
    fn interpolate(&mut self, tick: Tick) -> Result<(), FoldError> {
        for aggregation in
            self.series.interpolate_and_get_aggregations(&mut self.interpolation, tick)
        {
            let (count, aggregation) = aggregation?;
            if count.get() == 1 {
                self.buffer.push(aggregation);
            } else {
                self.buffer.fill(aggregation, count);
            }
        }
        Ok(())
    }

    /// Folds the given sample and interpolations for intervals intersected by the given [`Tick`]
    /// and buffers the aggregations.
    ///
    /// # Errors
    ///
    /// Returns an error if sampling fails.
    ///
    /// [`Tick`]: crate::experimental::clock::Tick
    fn fold(&mut self, tick: Tick, sample: F::Sample) -> Result<(), FoldError> {
        for aggregation in
            self.series.fold_and_get_aggregations(&mut self.interpolation, tick, sample)
        {
            let (count, aggregation) = aggregation?;
            if count.get() == 1 {
                self.buffer.push(aggregation);
            } else {
                self.buffer.fill(aggregation, count);
            }
        }
        Ok(())
    }

    pub fn serialize_and_get_buffer(&self) -> io::Result<SerializedTimeSeries> {
        let mut data = vec![];
        self.buffer.serialize(&mut data)?;
        Ok(SerializedTimeSeries { interval: *self.series.interval(), data })
    }
}

/// A buffer of data from time matrix.
#[derive(Clone, Debug, PartialEq)]
pub struct SerializedBuffer {
    pub data_semantic: String,
    pub data: Vec<u8>,
}

impl SerializedBuffer {
    /// Records the current state of this `TimeMatrix` into `node`.
    pub fn write_to_inspect(self, node: &fuchsia_inspect::Node) {
        let Self { data_semantic, data } = self;
        node.record_string("type", data_semantic);
        node.record_bytes("data", data);
    }

    /// Records an attempt at retrieving a serialized buffer to inspect.
    pub fn write_to_inspect_or_error<E: Debug>(
        result: Result<Self, E>,
        node: &fuchsia_inspect::Node,
    ) {
        match result {
            Ok(b) => b.write_to_inspect(node),
            Err(e) => node.record_string("type", format!("error: {:?}", e)),
        }
    }
}

/// One or more statistical round-robin time series.
///
/// A time matrix is a round-robin multi-resolution time series that samples and interpolates timed
/// data, computes statistical aggregations for elapsed [sampling intervals][`SamplingInterval`],
/// and buffers those aggregations. The sample data, statistic, and interpolation of series in a
/// time matrix must be the same, but the sampling intervals can and should differ.
#[derive(Derivative)]
#[derivative(
    Clone(bound = "F: Clone, F::Buffer: Clone, P::Output<F::Sample>: Clone,"),
    Debug(bound = "F: Debug,
                   F::Buffer: Debug,
                   P::Output<F::Sample>: Debug,")
)]
pub struct TimeMatrix<F, P>
where
    F: SerialStatistic<P>,
    P: InterpolationKind,
{
    created: Timestamp,
    last: ObservationTime,
    buffers: Vec1<BufferedTimeSeries<F, P>>,
}

impl<F, P> TimeMatrix<F, P>
where
    F: SerialStatistic<P>,
    P: InterpolationKind,
{
    fn from_series_with<Q>(
        created: Timestamp,
        series: impl Into<Vec1<TimeSeries<F>>>,
        mut interpolation: Q,
    ) -> Self
    where
        Q: FnMut() -> P::Output<F::Sample>,
    {
        let buffers =
            series.into().map_into(|series| BufferedTimeSeries::new((interpolation)(), series));
        TimeMatrix { created, last: ObservationTime::at(created), buffers }
    }

    /// Constructs a time matrix with the given sampling profile and interpolation.
    ///
    /// Statistics are default initialized.
    pub fn new(profile: impl Into<SamplingProfile>, interpolation: P::Output<F::Sample>) -> Self
    where
        F: Default,
    {
        Self::new_at(Timestamp::now(), profile, interpolation)
    }

    pub(crate) fn new_at(
        timestamp: Timestamp,
        profile: impl Into<SamplingProfile>,
        interpolation: P::Output<F::Sample>,
    ) -> Self
    where
        F: Default,
    {
        let sampling_intervals = profile.into().into_sampling_intervals();
        TimeMatrix::from_series_with(
            timestamp,
            sampling_intervals.map_into(TimeSeries::new),
            || interpolation.clone(),
        )
    }

    /// Constructs a time matrix with the given statistic.
    pub fn with_statistic(
        profile: impl Into<SamplingProfile>,
        interpolation: P::Output<F::Sample>,
        statistic: F,
    ) -> Self {
        let sampling_intervals = profile.into().into_sampling_intervals();
        TimeMatrix::from_series_with(
            Timestamp::now(),
            sampling_intervals
                .map_into(|window| TimeSeries::with_statistic(window, statistic.clone())),
            || interpolation.clone(),
        )
    }

    /// Folds the given sample and interpolations and gets the aggregation buffers.
    ///
    /// To fold a sample without serializing buffers, use [`Sampler::fold`].
    ///
    /// [`Sampler::fold`]: crate::experimental::series::Sampler::fold
    pub fn fold_and_get_buffers(
        &mut self,
        sample: Timed<F::Sample>,
    ) -> Result<SerializedBuffer, FoldError> {
        self.fold(sample)?;
        let series_buffers = self
            .buffers
            .try_map_ref(BufferedTimeSeries::serialize_and_get_buffer)
            .map_err::<FoldError, _>(From::from)?;
        self.serialize(series_buffers).map_err(From::from)
    }

    fn serialize(
        &self,
        series_buffers: Vec1<SerializedTimeSeries>,
    ) -> io::Result<SerializedBuffer> {
        use crate::experimental::clock::DurationExt;
        use byteorder::{LittleEndian, WriteBytesExt};
        use std::io::Write;

        let created_timestamp = u32::try_from(self.created.quantize()).unwrap_or(u32::MAX);
        let end_timestamp =
            u32::try_from(self.last.last_update_timestamp.quantize()).unwrap_or(u32::MAX);

        let mut buffer = vec![];
        buffer.write_u8(1)?; // Version number.
        buffer.write_u32::<LittleEndian>(created_timestamp)?; // Matrix creation time.
        buffer.write_u32::<LittleEndian>(end_timestamp)?; // Last observed or interpolated sample
        // time.
        encoding::serialize_buffer_type_descriptors::<F, P>(&mut buffer)?; // Buffer descriptors.

        for series in series_buffers {
            const GRANULARITY_FIELD_LEN: usize = 2;
            let len = u16::try_from(series.data.len() + GRANULARITY_FIELD_LEN).unwrap_or(u16::MAX);
            let granularity =
                u16::try_from(series.interval().duration().into_quanta()).unwrap_or(u16::MAX);

            buffer.write_u16::<LittleEndian>(len)?;
            buffer.write_u16::<LittleEndian>(granularity)?;
            buffer.write_all(&series.data[..len as usize - GRANULARITY_FIELD_LEN])?;
        }
        Ok(SerializedBuffer {
            data_semantic: format!("{}", <F as Statistic>::Semantic::display()),
            data: buffer,
        })
    }
}

impl<F, R, P, A> TimeMatrix<PostAggregation<F, R>, P>
where
    PostAggregation<F, R>: SerialStatistic<P, Aggregation = A>,
    F: Default + SerialStatistic<P>,
    R: Clone + Fn(F::Aggregation) -> A,
    P: InterpolationKind,
    A: Clone,
{
    /// Constructs a time matrix with the default statistic and given transform for
    /// post-aggregation.
    pub fn with_transform(
        profile: impl Into<SamplingProfile>,
        interpolation: P::Output<<PostAggregation<F, R> as Statistic>::Sample>,
        transform: R,
    ) -> Self
    where
        R: Clone,
    {
        let sampling_intervals = profile.into().into_sampling_intervals();
        TimeMatrix::from_series_with(
            Timestamp::now(),
            sampling_intervals
                .map_into(|window| TimeSeries::with_transform(window, transform.clone())),
            || interpolation.clone(),
        )
    }
}

impl<F, P> Default for TimeMatrix<F, P>
where
    F: Default + SerialStatistic<P>,
    P: InterpolationKind,
    P::Output<F::Sample>: Default,
{
    fn default() -> Self {
        TimeMatrix::new(SamplingProfile::default(), P::Output::default())
    }
}

impl<F, P> TimeMatrixFold<F::Sample> for TimeMatrix<F, P>
where
    F: SerialStatistic<P>,
    P: InterpolationKind,
{
    fn fold(&mut self, sample: Timed<F::Sample>) -> Result<(), FoldError> {
        let (timestamp, sample) = sample.into();
        let tick = self.last.tick(timestamp, true)?;
        Ok(for buffer in self.buffers.iter_mut() {
            buffer.fold(tick, sample.clone())?;
        })
    }
}

impl<F, P> TimeMatrixTick for TimeMatrix<F, P>
where
    F: SerialStatistic<P>,
    P: InterpolationKind,
{
    fn tick(&mut self, timestamp: Timestamp) -> Result<(), FoldError> {
        let tick = self.last.tick(timestamp.into(), false)?;
        Ok(for buffer in self.buffers.iter_mut() {
            buffer.interpolate(tick)?;
        })
    }

    fn tick_and_get_buffers(
        &mut self,
        timestamp: Timestamp,
    ) -> Result<SerializedBuffer, FoldError> {
        self.tick(timestamp)?;
        let series_buffers = self
            .buffers
            .try_map_ref(BufferedTimeSeries::serialize_and_get_buffer)
            .map_err::<FoldError, _>(From::from)?;
        self.serialize(series_buffers).map_err(From::from)
    }
}

#[cfg(test)]
mod tests {
    use fuchsia_async as fasync;

    use crate::experimental::clock::{Timed, Timestamp};
    use crate::experimental::series::interpolation::{ConstantSample, LastSample};
    use crate::experimental::series::statistic::{
        ArithmeticMean, LatchMax, Max, PostAggregation, Sum, Transform, Union,
    };
    use crate::experimental::series::{
        SamplingProfile, TimeMatrix, TimeMatrixFold, TimeMatrixTick,
    };

    fn fold_and_interpolate_f32(matrix: &mut impl TimeMatrixFold<f32>) {
        matrix.fold(Timed::now(0.0)).unwrap();
        matrix.fold(Timed::now(1.0)).unwrap();
        matrix.fold(Timed::now(2.0)).unwrap();
        matrix.tick(Timestamp::now()).unwrap();
    }

    // TODO(https://fxbug.dev/356218503): Replace this with meaningful unit tests that assert the
    //                                    outputs of a `TimeMatrix`.
    // This "test" is considered successful as long as it builds.
    #[test]
    fn static_test_define_time_matrix() {
        type Mean<T> = ArithmeticMean<T>;
        type MeanTransform<T, F> = Transform<Mean<T>, F>;

        let _exec = fasync::TestExecutor::new_with_fake_time();

        // Arithmetic mean time matrices.
        let _ = TimeMatrix::<Mean<f32>, ConstantSample>::default();
        let _ = TimeMatrix::<Mean<f32>, LastSample>::new(
            SamplingProfile::balanced(),
            LastSample::or(0.0f32),
        );
        let _ = TimeMatrix::<_, ConstantSample>::with_statistic(
            SamplingProfile::granular(),
            ConstantSample::default(),
            Mean::<f32>::default(),
        );

        // Discrete arithmetic mean time matrices.
        let mut matrix = TimeMatrix::<MeanTransform<f32, i64>, LastSample>::with_transform(
            SamplingProfile::highly_granular(),
            LastSample::or(0.0f32),
            |aggregation| aggregation.ceil() as i64,
        );
        fold_and_interpolate_f32(&mut matrix);
        // This time matrix is constructed verbosely with no ad-hoc type definitions nor ergonomic
        // constructors. This is as raw as it gets.
        let mut matrix = TimeMatrix::<_, ConstantSample>::with_statistic(
            SamplingProfile::default(),
            ConstantSample::default(),
            PostAggregation::<ArithmeticMean<f32>, _>::from_transform(|aggregation: f32| {
                aggregation.ceil() as i64
            }),
        );
        fold_and_interpolate_f32(&mut matrix);
    }

    // TODO(https://fxbug.dev/356218503): Replace this with meaningful unit tests that assert the
    //                                    outputs of a `TimeMatrix`.
    // This "test" is considered successful as long as it builds.
    #[test]
    fn static_test_supported_statistic_and_interpolation_combinations() {
        let _exec = fasync::TestExecutor::new_with_fake_time();

        let _ = TimeMatrix::<ArithmeticMean<f32>, ConstantSample>::default();
        let _ = TimeMatrix::<ArithmeticMean<f32>, LastSample>::default();
        let _ = TimeMatrix::<LatchMax<u64>, LastSample>::default();
        let _ = TimeMatrix::<Max<u64>, ConstantSample>::default();
        let _ = TimeMatrix::<Max<u64>, LastSample>::default();
        let _ = TimeMatrix::<Sum<u64>, ConstantSample>::default();
        let _ = TimeMatrix::<Sum<u64>, LastSample>::default();
        let _ = TimeMatrix::<Union<u64>, ConstantSample>::default();
        let _ = TimeMatrix::<Union<u64>, LastSample>::default();
    }

    #[test]
    fn time_matrix_with_uncompressed_buffer() {
        let exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(3_000_000_000));
        let mut time_matrix = TimeMatrix::<ArithmeticMean<f32>, ConstantSample>::new(
            SamplingProfile::highly_granular(),
            ConstantSample::default(),
        );
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                3, 0, 0, 0, // last timestamp
                0, 0, // type: uncompressed; subtype: f32
                4, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                0, 0, // number of elements
                4, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of elements
            ]
        );

        time_matrix.fold(Timed::now(f32::from_bits(42u32))).unwrap();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                10, 0, 0, 0, // last timestamp
                0, 0, // type: uncompressed; subtype: f32
                8, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                1, 0, // number of elements
                42, 0, 0, 0, // item 1
                4, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of elements
            ]
        );

        // Advance several time steps to test ring buffer's `fill`
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(50_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                50, 0, 0, 0, // last timestamp
                0, 0, // type: uncompressed; subtype: f32
                24, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                5, 0, // number of elements
                42, 0, 0, 0, // item 1
                0, 0, 0, 0, // item 2
                0, 0, 0, 0, // item 3
                0, 0, 0, 0, // item 4
                0, 0, 0, 0, // item 5
                4, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of elements
            ]
        );
    }

    #[test]
    fn time_matrix_with_simple8b_rle_buffer() {
        let exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(3_000_000_000));
        let mut time_matrix = TimeMatrix::<Max<u64>, ConstantSample>::new(
            SamplingProfile::highly_granular(),
            ConstantSample::default(),
        );
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                3, 0, 0, 0, // last timestamp
                1, 0, // type: simple8b RLE; subtype: unsigned
                7, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                0, 0, // number of selector elements and value blocks
                0, 0, // head selector index
                0, // number of values in last block
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of selector elements and value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );

        time_matrix.fold(Timed::now(15)).unwrap();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                10, 0, 0, 0, // last timestamp
                1, 0, // type: simple8b RLE; subtype: unsigned
                16, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                1, 0, // number of selector elements and value blocks
                0, 0,    // head selector index
                1,    // number of values in last block
                0x0f, // RLE selector
                15, 0, 0, 0, 0, 0, 1, 0, // value 15 appears 1 time
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of selector elements and value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );

        // Advance several time steps to test ring buffer's `fill`
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(50_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                50, 0, 0, 0, // last timestamp
                1, 0, // type: simple8b RLE; subtype: unsigned
                16, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                1, 0, // number of selector elements and value blocks
                0, 0,    // head selector index
                5,    // number of values in last block
                0x03, // 4-bit selector
                0x0f, 0, 0, 0, 0, 0, 0, 0, // values 15, 0, 0, 0, 0
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of selector elements and value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );
    }

    #[test]
    fn time_matrix_with_zigzag_simple8b_rle_buffer() {
        let exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(3_000_000_000));
        let mut time_matrix = TimeMatrix::<Max<i64>, ConstantSample>::new(
            SamplingProfile::highly_granular(),
            ConstantSample::default(),
        );
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                3, 0, 0, 0, // last timestamp
                1, 1, // type: simple8b RLE; subtype: signed (zigzag encoded)
                7, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                0, 0, // number of selector elements and value blocks
                0, 0, // head selector index
                0, // number of values in last block
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of selector elements and value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );

        time_matrix.fold(Timed::now(-8)).unwrap();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                10, 0, 0, 0, // last timestamp
                1, 1, // type: simple8b RLE; subtype: signed (zigzag encoded)
                16, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                1, 0, // number of selector elements and value blocks
                0, 0,    // head selector index
                1,    // number of values in last block
                0x0f, // RLE selector
                15, 0, 0, 0, 0, 0, 1, 0, // value -8 (encoded as 15) appears 1 time
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of selector elements and value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );

        // Advance several time steps to test ring buffer's `fill`
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(50_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                50, 0, 0, 0, // last timestamp
                1, 1, // type: simple8b RLE; subtype: signed (zigzag encoded)
                16, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                1, 0, // number of selector elements and value blocks
                0, 0,    // head selector index
                5,    // number of values in last block
                0x03, // 4-bit selector
                0x0f, 0, 0, 0, 0, 0, 0, 0, // values -8 (encoded as 15), 0, 0, 0, 0
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of selector elements and value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );
    }

    #[test]
    fn time_matrix_with_delta_simple8b_rle_buffer() {
        let exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(3_000_000_000));
        let mut time_matrix = TimeMatrix::<LatchMax<u64>, LastSample>::new(
            SamplingProfile::highly_granular(),
            LastSample::or(0),
        );
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                3, 0, 0, 0, // last timestamp
                2, 0, // type: delta simple8b RLE; subtype: unsigned
                7, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );

        time_matrix.fold(Timed::now(42)).unwrap();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                10, 0, 0, 0, // last timestamp
                2, 0, // type: delta simple8b RLE; subtype: unsigned
                15, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                1, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
                42, 0, 0, 0, 0, 0, 0, 0, // base value
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );

        time_matrix.fold(Timed::now(57)).unwrap();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(20_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                20, 0, 0, 0, // last timestamp
                2, 0, // type: delta simple8b RLE; subtype: unsigned
                24, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                2, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                1, // number of values in last block
                42, 0, 0, 0, 0, 0, 0, 0,    // base value
                0x0f, // RLE selector
                15, 0, 0, 0, 0, 0, 1, 0, // value 15 (delta) appears 1 time
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );

        // Advance several time steps to test ring buffer's `fill`
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(50_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                50, 0, 0, 0, // last timestamp
                2, 0, // type: delta simple8b RLE; subtype: unsigned
                24, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                2, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                4, // number of values in last block
                42, 0, 0, 0, 0, 0, 0, 0,    // base value
                0x03, // 4-bit selector
                0x0f, 0, 0, 0, 0, 0, 0, 0, // values 15, 0, 0, 0
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );
    }

    #[test]
    fn time_matrix_with_delta_zigzag_simple8b_rle_buffer_i64() {
        let exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(3_000_000_000));
        let mut time_matrix = TimeMatrix::<Max<i64>, LastSample>::new(
            SamplingProfile::highly_granular(),
            LastSample::or(0),
        );
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                3, 0, 0, 0, // last timestamp
                2, 1, // type: delta simple8b RLE; subtype: signed
                7, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );

        time_matrix.fold(Timed::now(42)).unwrap();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                10, 0, 0, 0, // last timestamp
                2, 1, // type: delta simple8b RLE; subtype: signed
                15, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                1, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
                42, 0, 0, 0, 0, 0, 0, 0, // base value
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );

        time_matrix.fold(Timed::now(34)).unwrap();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(20_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                20, 0, 0, 0, // last timestamp
                2, 1, // type: delta simple8b RLE; subtype: signed
                24, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                2, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                1, // number of values in last block
                42, 0, 0, 0, 0, 0, 0, 0,    // base value
                0x0f, // RLE selector
                15, 0, 0, 0, 0, 0, 1, 0, // value -8 (delta) encoded as 15, appearing 1 time
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );

        // Advance several time steps to test ring buffer's `fill`
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(50_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                50, 0, 0, 0, // last timestamp
                2, 1, // type: delta simple8b RLE; subtype: signed
                24, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                2, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                4, // number of values in last block
                42, 0, 0, 0, 0, 0, 0, 0,    // base value
                0x03, // 4-bit selector
                0x0f, 0, 0, 0, 0, 0, 0, 0, // diff values -8 (encoded as 15), 0, 0, 0
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );
    }

    #[test]
    fn time_matrix_with_delta_zigzag_simple8b_rle_buffer_u64() {
        let exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(3_000_000_000));
        let mut time_matrix = TimeMatrix::<Max<u64>, LastSample>::new(
            SamplingProfile::highly_granular(),
            LastSample::or(0),
        );
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                3, 0, 0, 0, // last timestamp
                2, 2, // type: delta simple8b RLE; subtype: unsigned with signed diff
                7, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );

        time_matrix.fold(Timed::now(1)).unwrap();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                10, 0, 0, 0, // last timestamp
                2, 2, // type: delta simple8b RLE; subtype: unsigned with signed diff
                15, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                1, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
                1, 0, 0, 0, 0, 0, 0, 0, // base value
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );

        time_matrix.fold(Timed::now(u64::MAX)).unwrap();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(20_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                20, 0, 0, 0, // last timestamp
                2, 2, // type: delta simple8b RLE; subtype: unsigned with signed diff
                24, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                2, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                1, // number of values in last block
                1, 0, 0, 0, 0, 0, 0, 0,    // base value
                0x0f, // RLE selector
                3, 0, 0, 0, 0, 0, 1, 0, // value -2 (delta) encoded as 3, appearing 1 time
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );

        // Advance several time steps to test ring buffer's `fill`
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(50_000_000_000));
        let buffer = time_matrix.tick_and_get_buffers(Timestamp::now()).unwrap();
        assert_eq!(
            buffer.data,
            vec![
                1, // version number
                3, 0, 0, 0, // created timestamp
                50, 0, 0, 0, // last timestamp
                2, 2, // type: delta simple8b RLE; subtype: unsigned with signed diff
                24, 0, // series 1: length in bytes
                10, 0, // series 1 granularity: 10s
                2, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                4, // number of values in last block
                1, 0, 0, 0, 0, 0, 0, 0,    // base value
                0x01, // 2-bit selector
                3, 0, 0, 0, 0, 0, 0, 0, // diff values -2 (encoded as 3), 0, 0, 0
                7, 0, // series 2: length in bytes
                60, 0, // series 2 granularity: 60s
                0, 0, // number of base value + selector elements or value blocks
                0, 0, // head selector index
                0, // number of values in last block
            ]
        );
    }
}
