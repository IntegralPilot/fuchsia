// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides an atomic wrapper around [`zx::MonotonicInstant`].

use std::sync::atomic::{AtomicI64, Ordering};

/// An atomic wrapper around [`zx::MonotonicInstant`].
#[derive(Debug, Default)]
pub struct AtomicMonotonicInstant(AtomicI64);

impl From<zx::MonotonicInstant> for AtomicMonotonicInstant {
    fn from(t: zx::MonotonicInstant) -> Self {
        Self::new(t)
    }
}

impl AtomicMonotonicInstant {
    /// Creates an [`AtomicTime`].
    pub fn new(time: zx::MonotonicInstant) -> Self {
        Self(AtomicI64::new(time.into_nanos()))
    }

    /// Loads a [`zx::MonotonicInstant`].
    pub fn load(&self, order: Ordering) -> zx::MonotonicInstant {
        let Self(atomic_time) = self;
        zx::Instant::from_nanos(atomic_time.load(order))
    }

    /// Stores a [`zx::MonotonicInstant`].
    pub fn store(&self, val: zx::MonotonicInstant, order: Ordering) {
        let Self(atomic_time) = self;
        atomic_time.store(val.into_nanos(), order)
    }
}
