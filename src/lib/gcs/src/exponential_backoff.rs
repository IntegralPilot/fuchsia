// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common utilities for handling transient network errors.

use crate::error::GcsError;
use anyhow::Error;
use fuchsia_backoff::Backoff;
use rand::RngExt as _;
use rand::rngs::StdRng;
use std::cmp;
use std::time::Duration;

pub struct ExponentialBackoff {
    rng: StdRng,
    backoff_base: u64,
    backoff_budget: u64,
    transient_errors: u32,
}

/// Exponential backoff impl with backoff time budget and FullJitter:
/// https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/.
///
/// Retries according to `backoff_budget` in cases of `hyper::Error` or
/// `GcsError::HttpTransientError`, failing immediately for all other errors.
///
/// See https://cloud.google.com/storage/docs/retry-strategy for cases where
/// retrying would be useful.
impl Backoff<Error> for ExponentialBackoff {
    fn next_backoff(&mut self, err: &Error) -> Option<Duration> {
        // Handle transient errors.
        if err.is::<hyper::Error>()
            || matches!(err.downcast_ref::<GcsError>(), Some(GcsError::HttpTransientError(_)))
        {
            self.transient_errors += 1;
            if self.backoff_budget > 0 {
                let backoff_time = cmp::min(
                    self.rng.random_range(0..self.backoff_base.pow(self.transient_errors)),
                    self.backoff_budget,
                );

                self.backoff_budget -= backoff_time;
                return Some(Duration::from_millis(backoff_time));
            } else {
                eprintln!("A network request failed after {} attempts.", self.transient_errors);
            }
        }
        // Ignore non-transient errors (eg: NeedNewAccessToken or non-transient
        // [GcsError]s).
        None
    }
}

/// Returns a backoff strategy that can be used with the `fuchsia_backoff` crate
/// that will keep retrying up to a 5 second cumulative backoff.
///
/// Given these constants, we'll expect to handle up to ~6 transient network
/// failures.
///
/// The backoff strategy will recognize `hyper::Error` and
/// `GcsError::HttpTransientError` from the task as transient errors that cause
/// retries.
pub fn default_backoff_strategy() -> ExponentialBackoff {
    ExponentialBackoff {
        rng: rand::make_rng(),
        backoff_base: 4,
        backoff_budget: 5000,
        transient_errors: 0,
    }
}
