// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Sample and aggregation interpolation.

use std::fmt::{self, Debug, Formatter};
use std::marker::PhantomData;
use std::num::NonZeroUsize;

use crate::experimental::series::statistic::{FoldError, Statistic};

// The distinction between `InterpolationKind` and `Interpolation` provides an important feature:
// the interpolation for a `TimeMatrix` is specified nominally and any and all type parameters are
// implicitly and consistently forwarded from the `Statistic` type parameter. Consider this type:
//
// ```
// TimeMatrix<Sum<u64>, LastSample>
// ```
//
// Notice that `LastSample` is **not** parameterized. Instead, this parameter is lifted into the
// `InterpolationKind::Output` GAT. Without this separation of type constructor and closed output
// type, this type would require redundant and decoupled input parameters:
//
// ```
// TimeMatrix<Sum<u64>, LastSample<u64>>
// ```
//
// This loses the necessary relationship between the sample types (`u64`): they must be the same!

/// A kind of [`Interpolation`].
///
/// Interpolation kinds are nominal type constructors. The `Output` GAT provides a mapping from a
/// kind (with no type parameters) to a generic type constructor for a parameterized type. For
/// example, the [`LastSample`] kind has [`LastSampleOutput`] as its associated output type.
pub trait InterpolationKind {
    // TODO(https://fxbug.dev/372328823): The mapping from `InterpolationKind` types to their
    //                                    associated `Output` types prevents type inference in many
    //                                    APIs that construct a `TimeMatrix`. This means that
    //                                    `InterpolationKind` type parameters must often be
    //                                    annotated explicitly, even when the type appears in an
    //                                    expression of a function argument.
    //
    //                                    Refactor interpolation so that these type annotations are
    //                                    less commonly needed (or are not needed at all).
    /// The parameterized [`Interpolation`] associated with this kind.
    type Output<T>: Interpolation<T>
    where
        T: Clone;
}

/// A type that observes data in order to synthesize and fold interpolated samples into a
/// [`Statistic`].
///
/// `Interpolation`s mediate the aggregation of a [`Statistic`] by folding synthetic samples for
/// [sampling periods][`SamplingInterval`] in which no data has been observed.
///
/// [`SamplingInterval`]: crate::experimental::series::interval::SamplingInterval
pub trait Interpolation<T>: Clone {
    /// Folds an interpolated sample `n` times into the given [`Statistic`].
    fn interpolate<F>(&self, statistic: &mut F, n: NonZeroUsize) -> Result<(), FoldError>
    where
        F: Statistic<Sample = T>;

    /// Observes a (real) sample and potentially updates the state of the interpolation.
    fn observe(&mut self, _sample: T) {}
}

/// An [`Interpolation`] kind over a constant sample.
#[derive(Debug)]
pub enum ConstantSample {}

impl ConstantSample {
    pub fn default<T>() -> ConstantSampleOutput<T>
    where
        T: Default,
    {
        ConstantSampleOutput::default()
    }

    pub fn new<T>(sample: T) -> ConstantSampleOutput<T> {
        ConstantSampleOutput(sample)
    }
}

impl InterpolationKind for ConstantSample {
    type Output<T>
        = ConstantSampleOutput<T>
    where
        T: Clone;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ConstantSampleOutput<T>(pub T);

impl<T> Interpolation<T> for ConstantSampleOutput<T>
where
    T: Clone,
{
    fn interpolate<F>(&self, statistic: &mut F, n: NonZeroUsize) -> Result<(), FoldError>
    where
        F: Statistic<Sample = T>,
    {
        statistic.fill(self.0.clone(), n)
    }
}

/// An [`Interpolation`] kind over the last observed sample.
#[derive(Debug)]
pub enum LastSample {}

impl LastSample {
    pub fn or<T>(sample: T) -> LastSampleOutput<T> {
        LastSampleOutput::or(sample)
    }
}

impl InterpolationKind for LastSample {
    type Output<T>
        = LastSampleOutput<T>
    where
        T: Clone;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LastSampleOutput<T>(T);

impl<T> LastSampleOutput<T> {
    pub fn or(sample: T) -> Self {
        LastSampleOutput(sample)
    }
}

impl<T> Interpolation<T> for LastSampleOutput<T>
where
    T: Clone,
{
    fn interpolate<F>(&self, statistic: &mut F, n: NonZeroUsize) -> Result<(), FoldError>
    where
        F: Statistic<Sample = T>,
    {
        statistic.fill(self.0.clone(), n)
    }

    fn observe(&mut self, sample: T) {
        self.0 = sample;
    }
}

/// An [`Interpolation`] kind that samples nothing in periods with no data (i.e., does nothing).
#[derive(Debug)]
pub enum NoSample {}

impl NoSample {
    pub fn new<T>() -> NoSampleOutput<T> {
        NoSampleOutput::default()
    }
}

impl InterpolationKind for NoSample {
    type Output<T>
        = NoSampleOutput<T>
    where
        T: Clone;
}

pub struct NoSampleOutput<T>(PhantomData<fn() -> T>);

impl<T> Clone for NoSampleOutput<T> {
    fn clone(&self) -> Self {
        NoSampleOutput::default()
    }
}

impl<T> Copy for NoSampleOutput<T> {}

impl<T> Debug for NoSampleOutput<T> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("NoSampleOutput").finish_non_exhaustive()
    }
}

impl<T> Default for NoSampleOutput<T> {
    fn default() -> Self {
        NoSampleOutput(PhantomData)
    }
}

impl<T> Interpolation<T> for NoSampleOutput<T>
where
    T: Clone,
{
    fn interpolate<F>(&self, _statistic: &mut F, _n: NonZeroUsize) -> Result<(), FoldError>
    where
        F: Statistic<Sample = T>,
    {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use crate::experimental::series::interpolation::{
        ConstantSample, Interpolation, LastSample, NoSample,
    };
    use crate::experimental::series::statistic::{Max, Statistic};

    #[test]
    fn interpolate_constant() {
        let mut max = Max::<u64>::default();
        let mut constant = ConstantSample::new(1u64);

        constant.interpolate(&mut max, NonZeroUsize::MIN).unwrap();
        assert_eq!(max.aggregation().unwrap(), 1);

        constant.observe(7); // Observe a sample.
        constant.interpolate(&mut max, NonZeroUsize::MIN).unwrap();
        // The observation of the sample `7` is ignored by `Constant`. Interpolating again samples
        // the constant `1`, and so the aggregation is unchanged.
        assert_eq!(max.aggregation().unwrap(), 1);
    }

    #[test]
    fn interpolate_last_sample() {
        let mut max = Max::<u64>::default();
        let mut last = LastSample::or(1u64);

        last.interpolate(&mut max, NonZeroUsize::MIN).unwrap();
        assert_eq!(max.aggregation().unwrap(), 1);

        last.observe(7); // Observe a sample.
        last.interpolate(&mut max, NonZeroUsize::MIN).unwrap();
        // `LastSample` caches the last observed sample: `7` is remembered. Interpolating again
        // folds this cached sample, and so the aggregation is now `7`.
        assert_eq!(max.aggregation().unwrap(), 7);
    }

    #[test]
    fn interpolate_no_sample() {
        let mut max = Max::<u64>::default();
        let mut none = NoSample::new();

        none.interpolate(&mut max, NonZeroUsize::MIN).unwrap();
        assert_eq!(max.aggregation(), None);

        none.observe(7); // Observe a sample.
        none.interpolate(&mut max, NonZeroUsize::MIN).unwrap();
        // The observation of the sample `7` is ignored by `NoSample`. Interpolation does nothing,
        // and so the aggregation is unchanged.
        assert_eq!(max.aggregation(), None);
    }
}
