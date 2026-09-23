// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_hierarchy::{Property, SelectResult};
use std::num::TryFromIntError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error<'a> {
    #[error("Inspect histogram's bucket count changed between samples, rejected by Cobalt")]
    NumberOfHistogramBucketsChanged,

    #[error(
        "Count in one or more histogram buckets decreased between samples: index {index} went from {old_count} to {new_count}"
    )]
    HistogramBucketCountDecreased { index: u32, old_count: u64, new_count: u64 },

    #[error("Inspect histogram contained an index too large for a u32, required by Cobalt")]
    IndexTooLargeForU32,

    #[error("Inspect histogram contained an invalid index")]
    HistogramIndexOutOfBounds,

    #[error("Inspect histogram has negative count, unsupported by Cobalt")]
    HistogramHasNegativeCount,

    #[error("Sampler expected an Inspect histogram type, but got something else")]
    UnexpectedPropertyTypeForHistogram,

    #[error("Double histograms are unsupported. See https://fxbug.dev/380981679")]
    DoubleHistogramsUnsupported,

    #[error("Property type changed illegally in-between samples")]
    PropertyTypeChangedBetweenSamples,

    #[error("Failed to log metrics due to fidl error")]
    FailedToLogMetrics(#[from] fidl::Error),

    #[error("Metric logger server returned an error: {0:?}")]
    MetricLoggerServer(fidl_fuchsia_metrics::Error),

    #[error("Invalid SelectResult, skipping. SelectResult: {0:?}")]
    InvalidSelectResult(SelectResult<'a, String>),

    #[error("Invalid property type; expected Int or Uint, found {0:?}")]
    InvalidPropertyExpectedIntOrUint(Property),

    #[error("Invalid occurrence value; occurrence was negative: {0}")]
    OccurrenceWasNegative(#[from] TryFromIntError),

    #[error("Occurrence decreased, which is illegal. Prior: {prior}, current: {current}")]
    OccurrenceDecreased { prior: u64, current: u64 },

    #[error("Invalid property type; expected String, found {0:?}")]
    InvalidPropertyExpectedString(Property),
}

impl From<fidl_fuchsia_metrics::Error> for Error<'_> {
    fn from(err: fidl_fuchsia_metrics::Error) -> Self {
        Self::MetricLoggerServer(err)
    }
}
