// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::{Error, Result};

const ENCODE_RECURSION_LIMIT: usize = 32;

/// How much to add to a size to pad it to a multiple of 8
pub(crate) fn alignment_padding_for_size(sz: usize) -> usize {
    // Saturating ops prevent an overflow. The result is probably wrong
    // afterward but if an object is about to overflow a usize we're probably
    // gonna fail soon. This just gives the more sophisticated code upstairs
    // time to notice.
    ((sz.saturating_add(7usize)) & !7usize).saturating_sub(sz)
}

/// Tag for a FIDL message to identify it as a request or a response.
pub(crate) enum Direction {
    Request,
    Response,
}

impl std::string::ToString for Direction {
    fn to_string(&self) -> String {
        match self {
            Direction::Request => "request",
            Direction::Response => "response",
        }
        .to_owned()
    }
}

/// Helper object for counting recursion level.
#[derive(Copy, Clone)]
pub(crate) struct RecursionCounter(usize);

impl RecursionCounter {
    /// Create a new recursion counter.
    pub fn new() -> Self {
        Self(0)
    }

    /// Create a new recursion counter from this one with one more level of
    /// recursion recorded. Duplicating the counter and keeping the old one
    /// around means you can "decrement" by just throwing away the new counter
    /// and going back to the old one. The resulting usage pattern is an
    /// RAII-style frame guard.
    pub fn next(&self) -> Result<Self> {
        if self.0 == ENCODE_RECURSION_LIMIT {
            Err(Error::RecursionLimitExceeded)
        } else {
            Ok(Self(self.0 + 1))
        }
    }
}
