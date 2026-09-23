// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::append::{Append, BufferTooSmall, TrackedAppend, VecCursor};
use fuchsia_async::{DurationExt, OnTimeout, TimeoutExt};

use futures::Future;

pub mod fake_capabilities;
pub mod fake_features;
pub mod fake_frames;
pub mod fake_stas;

/// A trait which allows to expect a future to terminate within a given time or panic otherwise.
pub trait ExpectWithin: Future + Sized {
    fn expect_within<S: ToString + Clone>(
        self,
        duration: zx::MonotonicDuration,
        msg: S,
    ) -> OnTimeout<Self, Box<dyn FnOnce() -> Self::Output>> {
        let msg = msg.to_string();
        self.on_timeout(duration.after_now(), Box::new(move || panic!("{}", msg)))
    }
}

impl<F: Future + Sized> ExpectWithin for F {}

pub struct FixedSizedTestBuffer(VecCursor);
impl FixedSizedTestBuffer {
    pub fn new(capacity: usize) -> Self {
        Self(VecCursor::with_capacity(capacity))
    }
}

impl Append for FixedSizedTestBuffer {
    fn append_bytes(&mut self, bytes: &[u8]) -> Result<(), BufferTooSmall> {
        if !self.can_append(bytes.len()) {
            return Err(BufferTooSmall);
        }
        self.0.append_bytes(bytes)
    }

    fn append_bytes_zeroed(&mut self, len: usize) -> Result<&mut [u8], BufferTooSmall> {
        if !self.can_append(len) {
            return Err(BufferTooSmall);
        }
        self.0.append_bytes_zeroed(len)
    }

    fn can_append(&self, bytes: usize) -> bool {
        self.0.len() + bytes <= self.0.capacity()
    }
}

impl TrackedAppend for FixedSizedTestBuffer {
    fn bytes_appended(&self) -> usize {
        self.0.bytes_appended()
    }
}
