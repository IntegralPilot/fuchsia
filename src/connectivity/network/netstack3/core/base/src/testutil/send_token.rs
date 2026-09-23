// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

/// A tracker for [`FakeSendToken`]s.
#[derive(Debug, Default, Clone)]
pub struct FakeSendTokenTracker(Arc<AtomicUsize>);

impl FakeSendTokenTracker {
    /// Creates a new `FakeSendTokenTracker`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of tokens that are currently live.
    pub fn live_tokens(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }

    /// Creates a new token associated with this tracker.
    pub fn token(&self) -> FakeSendToken {
        let _: usize = self.0.fetch_add(1, Ordering::SeqCst);
        FakeSendToken(self.0.clone())
    }
}

/// A fake send token for testing.
#[derive(Debug, Default)]
pub struct FakeSendToken(Arc<AtomicUsize>);

impl Drop for FakeSendToken {
    fn drop(&mut self) {
        let _: usize = self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
