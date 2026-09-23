// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A convenient monotonically increasing identifier to use in tests.

use core::fmt::{self, Debug, Display};
use core::sync::atomic::{self, AtomicUsize};

/// A convenient monotonically increasing identifier to use in tests.
pub struct MonotonicIdentifier(usize);

impl Debug for MonotonicIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NB: This type is used as part of the debug implementation in device
        // IDs which should provide enough context themselves on the type. For
        // brevity we omit the type name.
        let Self(id) = self;
        Debug::fmt(id, f)
    }
}

impl Display for MonotonicIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

static MONOTONIC_COUNTER: AtomicUsize = AtomicUsize::new(1);

impl MonotonicIdentifier {
    /// Creates a new identifier with the next value.
    pub fn new() -> Self {
        Self(MONOTONIC_COUNTER.fetch_add(1, atomic::Ordering::SeqCst))
    }
}

impl Default for MonotonicIdentifier {
    fn default() -> Self {
        Self::new()
    }
}
