// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Statistics and sample aggregation.

use num::{Num, NumCast, Zero};
use std::convert::Infallible;
use std::fmt::Debug;
use std::num::NonZeroUsize;
use std::ops::BitOr;
use std::{cmp, io};
use thiserror::Error;

use crate::experimental::clock::MonotonicityError;
use crate::experimental::series::buffer::{BufferStrategy, RingBuffer};
use crate::experimental::series::interpolation::InterpolationKind;
use crate::experimental::series::interval::SamplingInterval;
use crate::experimental::series::{Bitset, Counter, DataSemantic, Gauge, GaugeForceSimple8bRle};
use crate::experimental::vec1::Vec1;

pub mod recipe {
    //! Type definitions and respellings of common or interesting statistic types.

    use crate::experimental::series::statistic::{ArithmeticMean, Transform};

    pub type ArithmeticMeanTransform<T, A> = Transform<ArithmeticMean<T>, A>;
}

/// Sample folding error.
///
/// Describes errors that can occur when folding a sample into a [`Statistic`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FoldError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Monotonicity(#[from] MonotonicityError),
    #[error("overflow in statistic computation")]
    Overflow,
    // TODO(https://fxbug.dev/432323121): Provide the data that could not be sent into the channel
    //                                    here. This requires an input type parameter on
    //                                    `FoldError`, a `dyn Any` trait object, etc.
    #[error("timed sample dropped: channel is closed or buffer is exhausted")]
    Buffer,
    #[error("failed to fold one or more buffered samples")]
    Flush(Vec1<FoldError>),
}

impl From<Infallible> for FoldError {
    fn from(_: Infallible) -> Self {
        unreachable!()
    }
}

/// A statistical function type that folds samples into an aggregation.
pub trait Statistic: Clone {
    /// The type of data semantic associated with samples.
    type Semantic: DataSemantic;
    /// The type of samples.
    type Sample: Clone;
    /// The type of the statistical aggregation.
    type Aggregation: Clone;

    /// Folds a sample into the aggregation of the statistic.
    ///
    /// # Errors
    ///
    /// Returns an error if folding the sample causes an overflow.
    fn fold(&mut self, sample: Self::Sample) -> Result<(), FoldError>;

    /// Folds a sample into the aggregation of a statistic `n` (one or more) times.
    ///
    /// For some `Statistic`s, this function is significantly more efficient than multiple calls to
    /// [`fold`]. For example, [`Max`] need not fold more than once for any given sample regardless
    /// of `n`. Prefer this function when the same sample must be folded more than once.
    ///
    /// # Errors
    ///
    /// Returns an error if folding the sample causes an overflow.
    fn fill(&mut self, sample: Self::Sample, n: NonZeroUsize) -> Result<(), FoldError> {
        for _ in 0..n.get() {
            self.fold(sample.clone())?;
        }
        Ok(())
    }

    /// Resets the state (and aggregation) of the statistic.
    ///
    /// The state of a statistic after a reset is arbitrary, but most types reset to a reasonable
    /// initial state via `Default`. Some types do nothing, such as [`LatchMax`], which operates
    /// across [`SamplingInterval`]s.
    ///
    /// Statistics can be configured to reset to any given state via [`Reset`].
    ///
    /// [`LatchMax`] crate::experimental::series::statistic::LatchMax
    /// [`Reset`] crate::experimental::series::statistic::Reset
    fn reset(&mut self);

    /// Gets the statistical aggregation.
    ///
    /// Returns `None` if no aggregation is ready.
    fn aggregation(&self) -> Option<Self::Aggregation>;
}

/// Extension methods for `Statistic`s.
pub trait StatisticExt: Statistic {
    /// Gets the statistical aggregation and resets the statistic.
    fn get_aggregation_and_reset(&mut self) -> Option<Self::Aggregation> {
        let aggregation = self.aggregation();
        self.reset();
        aggregation
    }
}

impl<F> StatisticExt for F where F: Statistic {}

/// A [`Statistic`] for which an in-memory ring buffer encoding is defined.
///
/// A serial statistic can be used with a [`TimeMatrix`]. A series of aggregations are stored in an
/// instance of the defined buffer, which is typically optimized for the data semantic and data
/// type of the statistic.
///
/// [`TimeMatrix`]: crate::experimental::series::TimeMatrix
pub trait SerialStatistic<P>: Statistic
where
    P: InterpolationKind,
{
    type Buffer: Clone + RingBuffer<Self::Aggregation>;

    fn buffer(interval: &SamplingInterval) -> Self::Buffer;
}

// This blanket implementation essentially forwards the `BufferStrategy` implementation of the
// associated `DataSemantic`. `DataSemantic` types like `Gauge` provide the core implementation and
// the `SerialStatistic` trait relates the necessary types and provides a more brief way to express
// bounds.
impl<F, P> SerialStatistic<P> for F
where
    F: Statistic,
    // The data semantic must implement `BufferStrategy` for the `Statistic`'s aggregation type and
    // the given `Interpolation` type `P`.
    F::Semantic: BufferStrategy<F::Aggregation, P>,
    P: InterpolationKind,
{
    type Buffer = <F::Semantic as BufferStrategy<F::Aggregation, P>>::Buffer;

    fn buffer(interval: &SamplingInterval) -> Self::Buffer {
        <F::Semantic>::buffer(interval)
    }
}

/// The associated data semantic type of a `Statistic`.
pub type Semantic<F> = <F as Statistic>::Semantic;

/// The associated metadata type of a `Statistic`.
pub type Metadata<F> = <Semantic<F> as DataSemantic>::Metadata;

/// The associated sample type of a `Statistic`.
pub type Sample<F> = <F as Statistic>::Sample;

/// The associated aggregation type of a `Statistic`.
pub type Aggregation<F> = <F as Statistic>::Aggregation;

/// Arithmetic mean statistic.
///
/// The arithmetic mean sums samples within their domain and computes the mean as a real number
/// represented using floating-point. For floating-point samples, `NaN`s are discarded and the sum
/// saturates to infinity without error.
///
/// This statistic is sensitive to overflow in the count of samples.
#[derive(Clone, Debug)]
pub struct ArithmeticMean<T> {
    /// The sum of samples.
    sum: T,
    /// The count of samples.
    n: u64,
}

impl<T> ArithmeticMean<T> {
    pub fn with_sum(sum: T) -> Self {
        ArithmeticMean { sum, n: 0 }
    }

    fn fill(&mut self, sample: T, n: NonZeroUsize) -> Result<(), FoldError>
    where
        Self: Statistic<Sample = T>,
        T: Clone + Num + NumCast,
    {
        Ok(match num::cast::<_, T>(n.get()) {
            Some(m) => {
                self.fold(sample * m)?;
                self.increment((n.get() as u64) - 1)?;
            }
            _ => {
                for _ in 0..n.get() {
                    self.fold(sample.clone())?;
                }
            }
        })
    }

    fn increment(&mut self, m: u64) -> Result<(), FoldError> {
        // TODO(https://fxbug.dev/351848566): On overflow, either saturate `self.n` or leave both
        //                                    `self.sum` and `self.n` unchanged (here and in
        //                                    `Sampler::fold` and `Fill::fill` implementations; see
        //                                    below).
        self.n.checked_add(m).inspect(|sum| self.n = *sum).map(|_| ()).ok_or(FoldError::Overflow)
    }

    fn reset(&mut self)
    where
        T: Zero,
    {
        *self = Default::default()
    }
}

impl<T> Default for ArithmeticMean<T>
where
    T: Zero,
{
    fn default() -> Self {
        ArithmeticMean::with_sum(T::zero())
    }
}

impl Statistic for ArithmeticMean<f32> {
    type Semantic = Gauge;
    type Sample = f32;
    type Aggregation = f32;

    fn fold(&mut self, sample: Self::Sample) -> Result<(), FoldError> {
        // Discard `NaN` terms and avoid some floating-point exceptions.
        self.sum = match sample {
            _ if sample.is_nan() => self.sum,
            sample if sample.is_infinite() => sample,
            // Because neither term is `NaN` and `sample` is finite, this saturates at negative or
            // positive infinity.
            sample => self.sum + sample,
        };
        self.increment(1)
    }

    fn fill(&mut self, sample: Self::Sample, n: NonZeroUsize) -> Result<(), FoldError> {
        ArithmeticMean::fill(self, sample, n)
    }

    fn reset(&mut self) {
        ArithmeticMean::reset(self)
    }

    fn aggregation(&self) -> Option<Self::Aggregation> {
        // This is lossy and lossiness correlates to the magnitude of `n`. See details of
        // `u64 as f32` casts.
        (self.n > 0).then(|| self.sum / (self.n as f32))
    }
}

impl Statistic for ArithmeticMean<i64> {
    type Semantic = Gauge;
    type Sample = i64;
    type Aggregation = f32;

    fn fold(&mut self, sample: Self::Sample) -> Result<(), FoldError> {
        // TODO(https://fxbug.dev/351848566): On overflow, either saturate `self.n` or leave both
        //                                    `self.sum` and `self.n` unchanged (here and in
        //                                    `Sampler::fold` and `Fill::fill` implementations; see
        //                                    below).
        self.sum = self.sum.checked_add(sample).ok_or(FoldError::Overflow)?;
        self.increment(1)
    }

    fn fill(&mut self, sample: Self::Sample, n: NonZeroUsize) -> Result<(), FoldError> {
        ArithmeticMean::fill(self, sample, n)
    }

    fn reset(&mut self) {
        ArithmeticMean::reset(self)
    }

    fn aggregation(&self) -> Option<Self::Aggregation> {
        // This is lossy and lossiness correlates to the magnitude of `n`. See details of
        // `i64 as f32` casts.
        (self.n > 0).then(|| self.sum as f32 / (self.n as f32))
    }
}

impl Statistic for ArithmeticMean<u64> {
    type Semantic = Gauge;
    type Sample = u64;
    type Aggregation = f32;

    fn fold(&mut self, sample: Self::Sample) -> Result<(), FoldError> {
        // TODO(https://fxbug.dev/351848566): On overflow, either saturate `self.n` or leave both
        //                                    `self.sum` and `self.n` unchanged (here and in
        //                                    `Sampler::fold` and `Fill::fill` implementations; see
        //                                    below).
        self.sum = self.sum.checked_add(sample).ok_or(FoldError::Overflow)?;
        self.increment(1)
    }

    fn fill(&mut self, sample: Self::Sample, n: NonZeroUsize) -> Result<(), FoldError> {
        ArithmeticMean::fill(self, sample, n)
    }

    fn reset(&mut self) {
        ArithmeticMean::reset(self)
    }

    fn aggregation(&self) -> Option<Self::Aggregation> {
        // This is lossy and lossiness correlates to the magnitude of `n`. See details of
        // `u64 as f32` casts.
        (self.n > 0).then(|| self.sum as f32 / (self.n as f32))
    }
}

/// Sum statistic.
///
/// The sum directly computes the aggregation in the domain of samples.
///
/// This statistic is sensitive to overflow in the sum of samples.
#[derive(Clone, Debug)]
pub struct Sum<T> {
    /// The sum of samples.
    sum: T,
}

impl<T> Sum<T> {
    pub fn with_sum(sum: T) -> Self {
        Sum { sum }
    }

    fn fill(&mut self, sample: T, n: NonZeroUsize) -> Result<(), FoldError>
    where
        Self: Statistic<Sample = T>,
        T: Clone + Num + NumCast,
    {
        if let Some(n) = num::cast::<_, T>(n.get()) {
            self.fold(sample * n)
        } else {
            Ok(for _ in 0..n.get() {
                self.fold(sample.clone())?;
            })
        }
    }

    fn reset(&mut self)
    where
        T: Zero,
    {
        *self = Default::default();
    }
}

impl<T> Default for Sum<T>
where
    T: Zero,
{
    fn default() -> Self {
        Sum::with_sum(T::zero())
    }
}

impl Statistic for Sum<u64> {
    type Semantic = Gauge;
    type Sample = u64;
    type Aggregation = u64;

    fn fold(&mut self, sample: Self::Sample) -> Result<(), FoldError> {
        // TODO(https://fxbug.dev/351848566): Saturate `self.sum` on overflow.
        self.sum
            .checked_add(sample)
            .inspect(|sum| self.sum = *sum)
            .map(|_| ())
            .ok_or(FoldError::Overflow)
    }

    fn fill(&mut self, sample: Self::Sample, n: NonZeroUsize) -> Result<(), FoldError> {
        Sum::fill(self, sample, n)
    }

    fn reset(&mut self) {
        Sum::reset(self)
    }

    fn aggregation(&self) -> Option<Self::Aggregation> {
        let sum = self.sum;
        Some(sum)
    }
}

impl Statistic for Sum<i64> {
    type Semantic = Gauge;
    type Sample = i64;
    type Aggregation = i64;

    fn fold(&mut self, sample: Self::Sample) -> Result<(), FoldError> {
        // TODO(https://fxbug.dev/351848566): Saturate `self.sum` on overflow.
        self.sum
            .checked_add(sample)
            .inspect(|sum| self.sum = *sum)
            .map(|_| ())
            .ok_or(FoldError::Overflow)
    }

    fn fill(&mut self, sample: Self::Sample, n: NonZeroUsize) -> Result<(), FoldError> {
        Sum::fill(self, sample, n)
    }

    fn reset(&mut self) {
        Sum::reset(self)
    }

    fn aggregation(&self) -> Option<Self::Aggregation> {
        let sum = self.sum;
        Some(sum)
    }
}

/// Minimum statistic.
#[derive(Clone, Debug)]
pub struct Min<T> {
    /// The minimum of samples.
    min: Option<T>,
}

impl<T> Min<T> {
    pub fn with_min(min: T) -> Self {
        Min { min: Some(min) }
    }
}

impl<T> Default for Min<T> {
    fn default() -> Self {
        Min { min: Default::default() }
    }
}

impl<T> Statistic for Min<T>
where
    T: Ord + Copy + Zero + Num + NumCast,
{
    type Semantic = Gauge;
    type Sample = T;
    type Aggregation = T;

    fn fold(&mut self, sample: Self::Sample) -> Result<(), FoldError> {
        self.min = Some(match self.min {
            Some(min) => cmp::min(min, sample),
            _ => sample,
        });
        Ok(())
    }

    fn fill(&mut self, sample: Self::Sample, _n: NonZeroUsize) -> Result<(), FoldError> {
        self.fold(sample)
    }

    fn reset(&mut self) {
        *self = Default::default();
    }

    fn aggregation(&self) -> Option<Self::Aggregation> {
        self.min
    }
}

/// Maximum statistic.
#[derive(Clone, Debug)]
pub struct Max<T> {
    /// The maximum of samples.
    max: Option<T>,
}

impl<T> Max<T> {
    pub fn with_max(max: T) -> Self {
        Max { max: Some(max) }
    }
}

impl<T> Default for Max<T> {
    fn default() -> Self {
        Max { max: Default::default() }
    }
}

impl<T> Statistic for Max<T>
where
    T: Ord + Copy + Zero + Num + NumCast,
{
    type Semantic = Gauge;
    type Sample = T;
    type Aggregation = T;

    fn fold(&mut self, sample: Self::Sample) -> Result<(), FoldError> {
        self.max = Some(match self.max {
            Some(max) => cmp::max(max, sample),
            _ => sample,
        });
        Ok(())
    }

    fn fill(&mut self, sample: Self::Sample, _n: NonZeroUsize) -> Result<(), FoldError> {
        self.fold(sample)
    }

    fn reset(&mut self) {
        *self = Default::default();
    }

    fn aggregation(&self) -> Option<Self::Aggregation> {
        self.max
    }
}

/// Statistic for keeping the most recent sample.
#[derive(Clone, Debug)]
pub struct Last<T> {
    /// The most recent sample.
    last: Option<T>,
}

impl<T> Last<T> {
    pub fn with_sample(sample: T) -> Self {
        Last { last: Some(sample) }
    }
}

impl<T> Default for Last<T> {
    fn default() -> Self {
        Last { last: Default::default() }
    }
}

impl<T> Statistic for Last<T>
where
    T: Copy + Zero + Num + NumCast,
{
    type Semantic = Gauge;
    type Sample = T;
    type Aggregation = T;

    fn fold(&mut self, sample: Self::Sample) -> Result<(), FoldError> {
        self.last = Some(sample);
        Ok(())
    }

    fn fill(&mut self, sample: Self::Sample, _n: NonZeroUsize) -> Result<(), FoldError> {
        self.fold(sample)
    }

    fn reset(&mut self) {
        *self = Default::default();
    }

    fn aggregation(&self) -> Option<Self::Aggregation> {
        self.last
    }
}

#[derive(Default, Copy, Clone, Debug)]
pub struct Union<T> {
    bits: T,
}

impl<T> Union<T> {
    pub fn with_bits(bits: T) -> Self {
        Union { bits }
    }
}

impl<T> Statistic for Union<T>
where
    T: Into<u64> + BitOr<Output = T> + Copy + Default,
{
    type Semantic = Bitset;
    type Sample = T;
    type Aggregation = T;

    fn fold(&mut self, sample: Self::Sample) -> Result<(), FoldError> {
        self.bits = self.bits | sample;
        Ok(())
    }

    fn fill(&mut self, sample: Self::Sample, _n: NonZeroUsize) -> Result<(), FoldError> {
        self.fold(sample)
    }

    fn reset(&mut self) {
        *self = Default::default();
    }

    fn aggregation(&self) -> Option<Self::Aggregation> {
        Some(self.bits)
    }
}

/// Maximum statistic that sums non-monotonic samples into the maximum.
///
/// This statistic is sensitive to overflow in the sum of samples with the non-monotonic sum.
#[derive(Clone, Debug)]
pub struct LatchMax<T> {
    /// The last observed sample.
    last: Option<T>,
    /// The maximum of samples and the non-monotonic sum.
    max: Option<T>,
    /// The sum of non-monotonic samples.
    sum: T,
}

impl<T> LatchMax<T> {
    pub fn with_max(max: T) -> Self
    where
        T: Zero,
    {
        LatchMax::with_max_and_sum(max, T::zero())
    }

    pub fn with_max_and_sum(max: T, sum: T) -> Self {
        LatchMax { last: None, max: Some(max), sum }
    }
}

impl<T> Default for LatchMax<T>
where
    T: Zero,
{
    fn default() -> Self {
        LatchMax { last: None, max: None, sum: T::zero() }
    }
}

impl Statistic for LatchMax<u64> {
    type Semantic = Counter;
    type Sample = u64;
    type Aggregation = u64;

    fn fold(&mut self, sample: Self::Sample) -> Result<(), FoldError> {
        match self.last {
            Some(last) if sample < last => {
                // TODO(https://fxbug.dev/351848566): Saturate `self.sum` on overflow.
                self.sum = self.sum.checked_add(last).ok_or(FoldError::Overflow)?;
            }
            _ => {}
        }
        self.last = Some(sample);

        let sum = sample.checked_add(self.sum).ok_or(FoldError::Overflow)?;
        self.max = Some(match self.max {
            Some(max) => cmp::max(max, sum),
            _ => sum,
        });
        Ok(())
    }

    fn fill(&mut self, sample: Self::Sample, _n: NonZeroUsize) -> Result<(), FoldError> {
        self.fold(sample)
    }

    fn reset(&mut self) {}

    fn aggregation(&self) -> Option<Self::Aggregation> {
        self.max
    }
}

/// Post-aggregation transform.
///
/// A post-aggregation composes a [`Statistic`] and arbitrarily maps its aggregation. For example,
/// a transform may discretize the continuous aggregation of a statistic.
///
/// [`Statistic`]: crate::experimental::series::statistic::Statistic
#[derive(Clone, Copy, Debug)]
pub struct PostAggregation<F, R> {
    statistic: F,
    transform: R,
}

/// A post-aggregation of a function pointer.
///
/// This type definition describes a stateless and pure transform with a type name that can be
/// spelled in any context (i.e., no closure or other unnameable types occur).
pub type Transform<F, A> = PostAggregation<F, fn(Aggregation<F>) -> A>;

impl<F, R> PostAggregation<F, R> {
    pub fn with(statistic: F, transform: R) -> Self {
        PostAggregation { statistic, transform }
    }

    /// Constructs a `PostAggregation` from a transform function.
    ///
    /// The default statistic is composed.
    pub fn from_transform(transform: R) -> Self
    where
        F: Default,
    {
        PostAggregation { statistic: F::default(), transform }
    }
}

impl<A, F, R> Statistic for PostAggregation<F, R>
where
    F: Statistic,
    R: Clone + Fn(F::Aggregation) -> A,
    A: Clone,
{
    type Semantic = F::Semantic;
    type Sample = F::Sample;
    type Aggregation = A;

    fn fold(&mut self, sample: F::Sample) -> Result<(), FoldError> {
        self.statistic.fold(sample)
    }

    fn fill(&mut self, sample: F::Sample, n: NonZeroUsize) -> Result<(), FoldError> {
        self.statistic.fill(sample, n)
    }

    fn reset(&mut self) {
        self.statistic.reset()
    }

    fn aggregation(&self) -> Option<Self::Aggregation> {
        self.statistic.aggregation().map(|aggregation| (self.transform)(aggregation))
    }
}

/// Applies an arbitrary [reset function][`reset`] to a [`Statistic`].
///
/// [`reset`]: crate::experimental::series::statistic::Statistic::reset
/// [`Statistic`]: crate::experimental::series::statistic::Statistic
#[derive(Clone, Copy, Debug)]
pub struct Reset<F, R> {
    statistic: F,
    reset: R,
}

impl<F, R> Reset<F, R>
where
    F: Statistic,
    R: FnMut() -> F,
{
    pub fn with(statistic: F, reset: R) -> Self {
        Reset { statistic, reset }
    }
}

impl<F, R> Statistic for Reset<F, R>
where
    F: Statistic,
    R: Clone + FnMut() -> F,
{
    type Semantic = F::Semantic;
    type Sample = F::Sample;
    type Aggregation = F::Aggregation;

    fn fold(&mut self, sample: F::Sample) -> Result<(), FoldError> {
        self.statistic.fold(sample)
    }

    fn fill(&mut self, sample: F::Sample, n: NonZeroUsize) -> Result<(), FoldError> {
        self.statistic.fill(sample, n)
    }

    fn reset(&mut self) {
        self.statistic = (self.reset)();
    }

    fn aggregation(&self) -> Option<Self::Aggregation> {
        self.statistic.aggregation()
    }
}

/// A [`Statistic`] that produces an aggregation that is the difference between
/// the first and last sample of the aggregation interval.
#[derive(Default, Copy, Clone, Debug)]
pub struct Diff<T> {
    left: Option<T>,
    right: Option<T>,
}

impl<T: num::CheckedSub + Copy> Statistic for Diff<T> {
    type Semantic = GaugeForceSimple8bRle;
    type Sample = T;
    type Aggregation = T;

    fn fold(&mut self, sample: Self::Sample) -> Result<(), FoldError> {
        let Self { left, right } = self;
        let _: &mut T = left.get_or_insert(sample);
        *right = Some(sample);
        Ok(())
    }

    fn reset(&mut self) {
        let Self { left, right } = self;
        *left = right.take();
    }

    fn aggregation(&self) -> Option<Self::Aggregation> {
        let Self { left, right } = self;
        left.as_ref().zip(right.as_ref()).and_then(|(l, r)| r.checked_sub(l))
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use std::num::NonZeroUsize;

    use crate::experimental::series::statistic::{
        ArithmeticMean, Diff, FoldError, Last, LatchMax, Max, Min, PostAggregation, Reset,
        Statistic, Sum, Union,
    };

    #[test]
    fn arithmetic_mean_aggregation() {
        let mut mean = ArithmeticMean::<f32>::default();
        mean.fold(1.0).unwrap();
        mean.fold(1.0).unwrap();
        mean.fold(1.0).unwrap();
        let aggregation = mean.aggregation().unwrap();
        assert!(aggregation > 0.99 && aggregation < 1.01); // ~ 1.0

        let mut mean = ArithmeticMean::<f32>::default();
        mean.fold(0.0).unwrap();
        mean.fold(1.0).unwrap();
        mean.fold(2.0).unwrap();
        let aggregation = mean.aggregation().unwrap();
        assert!(aggregation > 0.99 && aggregation < 1.01); // ~ 1.0
    }

    #[test]
    fn arithmetic_mean_aggregation_fill() {
        let mut mean = ArithmeticMean::<f32>::default();
        mean.fill(1.0, NonZeroUsize::new(1000).unwrap()).unwrap();
        let aggregation = mean.aggregation().unwrap();
        assert!(aggregation > 0.99 && aggregation < 1.01); // ~ 1.0
    }

    #[test]
    fn arithmetic_mean_count_overflow() {
        let mut mean = ArithmeticMean::<f32> { sum: 1.0, n: u64::MAX };
        let result = mean.fold(1.0);
        assert_matches!(result, Err(FoldError::Overflow));
    }

    #[test]
    fn arithmetic_mean_i64_aggregation() {
        let mut mean = ArithmeticMean::<i64>::default();
        mean.fold(1).unwrap();
        mean.fold(1).unwrap();
        mean.fold(1).unwrap();
        let aggregation = mean.aggregation().unwrap();
        assert!(aggregation > 0.99 && aggregation < 1.01); // ~ 1.0

        let mut mean = ArithmeticMean::<i64>::default();
        mean.fold(0).unwrap();
        mean.fold(1).unwrap();
        mean.fold(2).unwrap();
        let aggregation = mean.aggregation().unwrap();
        assert!(aggregation > 0.99 && aggregation < 1.01); // ~ 1.0
    }

    #[test]
    fn arithmetic_mean_i64_aggregation_fill() {
        let mut mean = ArithmeticMean::<i64>::default();
        mean.fill(1, NonZeroUsize::new(1000).unwrap()).unwrap();
        let aggregation = mean.aggregation().unwrap();
        assert!(aggregation > 0.99 && aggregation < 1.01); // ~ 1.0
    }

    #[test]
    fn arithmetic_mean_i64_sum_overflow() {
        let mut mean = ArithmeticMean::<i64> { sum: i64::MAX, n: 1 };
        let result = mean.fold(1);
        assert_matches!(result, Err(FoldError::Overflow));
    }

    #[test]
    fn arithmetic_mean_i64_count_overflow() {
        let mut mean = ArithmeticMean::<i64> { sum: 1, n: u64::MAX };
        let result = mean.fold(1);
        assert_matches!(result, Err(FoldError::Overflow));
    }

    #[test]
    fn arithmetic_mean_u64_aggregation() {
        let mut mean = ArithmeticMean::<u64>::default();
        mean.fold(1).unwrap();
        mean.fold(1).unwrap();
        mean.fold(1).unwrap();
        let aggregation = mean.aggregation().unwrap();
        assert!(aggregation > 0.99 && aggregation < 1.01); // ~ 1.0

        let mut mean = ArithmeticMean::<u64>::default();
        mean.fold(0).unwrap();
        mean.fold(1).unwrap();
        mean.fold(2).unwrap();
        let aggregation = mean.aggregation().unwrap();
        assert!(aggregation > 0.99 && aggregation < 1.01); // ~ 1.0
    }

    #[test]
    fn arithmetic_mean_u64_aggregation_fill() {
        let mut mean = ArithmeticMean::<u64>::default();
        mean.fill(1, NonZeroUsize::new(1000).unwrap()).unwrap();
        let aggregation = mean.aggregation().unwrap();
        assert!(aggregation > 0.99 && aggregation < 1.01); // ~ 1.0
    }

    #[test]
    fn arithmetic_mean_u64_sum_overflow() {
        let mut mean = ArithmeticMean::<u64> { sum: u64::MAX, n: 1 };
        let result = mean.fold(1);
        assert_matches!(result, Err(FoldError::Overflow));
    }

    #[test]
    fn arithmetic_mean_u64_count_overflow() {
        let mut mean = ArithmeticMean::<u64> { sum: 1, n: u64::MAX };
        let result = mean.fold(1);
        assert_matches!(result, Err(FoldError::Overflow));
    }

    #[test]
    fn sum_aggregation() {
        let mut sum = Sum::<u64>::default();
        sum.fold(1).unwrap();
        sum.fold(1).unwrap();
        sum.fold(1).unwrap();
        let aggregation = sum.aggregation().unwrap();
        assert_eq!(aggregation, 3);

        let mut sum = Sum::<u64>::default();
        sum.fold(0).unwrap();
        sum.fold(1).unwrap();
        sum.fold(2).unwrap();
        let aggregation = sum.aggregation().unwrap();
        assert_eq!(aggregation, 3);
    }

    #[test]
    fn sum_aggregation_fill() {
        let mut sum = Sum::<u64>::default();
        sum.fill(10, NonZeroUsize::new(1000).unwrap()).unwrap();
        let aggregation = sum.aggregation().unwrap();
        assert_eq!(aggregation, 10_000);
    }

    #[test]
    fn sum_overflow() {
        let mut sum = Sum::<u64> { sum: u64::MAX };
        let result = sum.fold(1);
        assert_matches!(result, Err(FoldError::Overflow));
    }

    #[test]
    fn sum_i64_aggregation() {
        let mut sum = Sum::<i64>::default();
        sum.fold(1).unwrap();
        sum.fold(1).unwrap();
        sum.fold(1).unwrap();
        let aggregation = sum.aggregation().unwrap();
        assert_eq!(aggregation, 3);

        let mut sum = Sum::<i64>::default();
        sum.fold(0).unwrap();
        sum.fold(1).unwrap();
        sum.fold(2).unwrap();
        let aggregation = sum.aggregation().unwrap();
        assert_eq!(aggregation, 3);
    }

    #[test]
    fn sum_i64_aggregation_fill() {
        let mut sum = Sum::<i64>::default();
        sum.fill(10, NonZeroUsize::new(1000).unwrap()).unwrap();
        let aggregation = sum.aggregation().unwrap();
        assert_eq!(aggregation, 10_000);
    }

    #[test]
    fn sum_i64_overflow() {
        let mut sum = Sum::<i64> { sum: i64::MAX };
        let result = sum.fold(1);
        assert_matches!(result, Err(FoldError::Overflow));
    }

    #[test]
    fn min_aggregation() {
        let mut min = Min::<u64>::default();
        min.fold(10).unwrap();
        min.fold(1337).unwrap();
        min.fold(42).unwrap();
        let aggregation = min.aggregation().unwrap();
        assert_eq!(aggregation, 10);
    }

    #[test]
    fn min_aggregation_fill() {
        let mut min = Min::<u64>::default();
        min.fill(42, NonZeroUsize::new(1000).unwrap()).unwrap();
        let aggregation = min.aggregation().unwrap();
        assert_eq!(aggregation, 42);
    }

    #[test]
    fn max_aggregation() {
        let mut max = Max::<u64>::default();
        max.fold(0).unwrap();
        max.fold(1337).unwrap();
        max.fold(42).unwrap();
        let aggregation = max.aggregation().unwrap();
        assert_eq!(aggregation, 1337);
    }

    #[test]
    fn max_aggregation_fill() {
        let mut max = Max::<u64>::default();
        max.fill(42, NonZeroUsize::new(1000).unwrap()).unwrap();
        let aggregation = max.aggregation().unwrap();
        assert_eq!(aggregation, 42);
    }

    #[test]
    fn last_aggregation() {
        let mut last = Last::<u64>::default();
        last.fold(0).unwrap();
        last.fold(1337).unwrap();
        last.fold(42).unwrap();
        let aggregation = last.aggregation().unwrap();
        assert_eq!(aggregation, 42);
    }

    #[test]
    fn last_aggregation_fill() {
        let mut last = Last::<u64>::default();
        last.fill(42, NonZeroUsize::new(1000).unwrap()).unwrap();
        let aggregation = last.aggregation().unwrap();
        assert_eq!(aggregation, 42);
    }

    #[test]
    fn union_aggregation() {
        let mut value = Union::<u64>::default();
        value.fold(1 << 1).unwrap();
        value.fold(1 << 3).unwrap();
        value.fold(1 << 5).unwrap();
        let aggregation = value.aggregation().unwrap();
        assert_eq!(aggregation, 0b101010);
    }

    #[test]
    fn union_aggregation_fill() {
        let mut value = Union::<u64>::default();
        value.fill(1 << 2, NonZeroUsize::new(1000).unwrap()).unwrap();
        let aggregation = value.aggregation().unwrap();
        assert_eq!(aggregation, 0b100);
    }

    #[test]
    fn latch_max_aggregation() {
        let mut max = LatchMax::<u64>::default();
        max.fold(1).unwrap();
        max.fold(1).unwrap();
        max.fold(1).unwrap();
        let aggregation = max.aggregation().unwrap();
        assert_eq!(aggregation, 1);

        let mut max = LatchMax::<u64>::default();
        max.fold(0).unwrap();
        max.fold(1).unwrap();
        max.fold(2).unwrap();
        let aggregation = max.aggregation().unwrap();
        assert_eq!(aggregation, 2);

        let mut max = LatchMax::<u64>::default();
        max.fold(1).unwrap();
        max.fold(5).unwrap();
        max.fold(6).unwrap();
        max.fold(2).unwrap(); // Non-monotonic sum is six.
        max.fold(9).unwrap();
        max.fold(3).unwrap(); // Non-monotonic sum is 15 (6 + 9).
        max.fold(5).unwrap();
        let aggregation = max.aggregation().unwrap();
        assert_eq!(aggregation, 20);
    }

    #[test]
    fn latch_max_aggregation_fill() {
        let mut max = LatchMax::<u64>::default();
        max.fill(10, NonZeroUsize::new(1000).unwrap()).unwrap();
        let aggregation = max.aggregation().unwrap();
        assert_eq!(aggregation, 10);
    }

    #[test]
    fn latch_max_overflow_max() {
        let mut max = LatchMax::<u64>::default();
        max.fold(1).unwrap();
        max.fold(0).unwrap(); // Non-monotonic sum is one.
        let result = max.fold(u64::MAX); // Non-monotonic sum of one is added to `u64::MAX`.
        assert_matches!(result, Err(FoldError::Overflow));
    }

    #[test]
    fn post_sum_aggregation() {
        let mut sum = PostAggregation::<Sum<u64>, _>::from_transform(|sum| sum + 1);
        sum.fold(0).unwrap();
        sum.fold(1).unwrap();
        sum.fold(2).unwrap();
        let aggregation = sum.aggregation().unwrap();
        assert_eq!(aggregation, 4);
    }

    #[test]
    fn reset_with_function() {
        let mut sum = Reset::with(Sum::<u64>::default(), || Sum::with_sum(3));

        assert_eq!(sum.aggregation().unwrap(), 0);
        sum.fold(1).unwrap();
        assert_eq!(sum.aggregation().unwrap(), 1);

        sum.reset();
        assert_eq!(sum.aggregation().unwrap(), 3);
        sum.fold(1).unwrap();
        assert_eq!(sum.aggregation().unwrap(), 4);
    }

    #[test]
    fn diff_aggregation() {
        let mut diff = Diff::<u64>::default();
        assert_eq!(diff.aggregation(), None);
        diff.fold(1).unwrap();
        assert_eq!(diff.aggregation(), Some(0));
        diff.fold(3).unwrap();
        assert_eq!(diff.aggregation(), Some(2));
        diff.fold(5).unwrap();
        assert_eq!(diff.aggregation(), Some(4));
        diff.reset();
        assert_eq!(diff.aggregation(), None);
        diff.fold(7).unwrap();
        assert_eq!(diff.aggregation(), Some(2));
        diff.fold(7).unwrap();
        assert_eq!(diff.aggregation(), Some(2));
        diff.fold(4).unwrap();
        assert_eq!(diff.aggregation(), None);
    }
}
