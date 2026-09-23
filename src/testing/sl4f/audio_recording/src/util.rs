// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::future::Future;
use std::task::{Poll, Waker};

use anyhow::{format_err, Error};
use fidl_fuchsia_media::AudioSampleFormat;
use std::sync::{Arc, Mutex as StdMutex, Weak};

// Values found in:
//   zircon/system/public/zircon/device/audio.h
const AUDIO_SAMPLE_FORMAT_8BIT: u32 = 1 << 1;
const AUDIO_SAMPLE_FORMAT_16BIT: u32 = 1 << 2;
const AUDIO_SAMPLE_FORMAT_24BIT_IN32: u32 = 1 << 7;
const AUDIO_SAMPLE_FORMAT_32BIT_FLOAT: u32 = 1 << 9;

/// Utility function for converting from a format flag to the size of each sample.
pub(crate) fn get_sample_size(format: u32) -> Result<u32, Error> {
    Ok(match format {
        // These are the currently implemented formats.
        AUDIO_SAMPLE_FORMAT_8BIT => 1,
        AUDIO_SAMPLE_FORMAT_16BIT => 2,
        AUDIO_SAMPLE_FORMAT_24BIT_IN32 => 4,
        AUDIO_SAMPLE_FORMAT_32BIT_FLOAT => 4,
        _ => return Err(format_err!("Cannot handle sample_format: {:?}", format)),
    })
}

/// Utility function to convert from an AudioSampleFormat into the numeric format known to zircon.
pub(crate) fn get_zircon_sample_format(format: AudioSampleFormat) -> u32 {
    match format {
        AudioSampleFormat::Signed16 => AUDIO_SAMPLE_FORMAT_16BIT,
        AudioSampleFormat::Unsigned8 => AUDIO_SAMPLE_FORMAT_8BIT,
        AudioSampleFormat::Signed24In32 => AUDIO_SAMPLE_FORMAT_24BIT_IN32,
        AudioSampleFormat::Float => AUDIO_SAMPLE_FORMAT_32BIT_FLOAT,
        // No default case, these are all the audio sample formats supported right now.
    }
}

/// A condition object that allows for asynchronous waiting for a specific condition.
///
/// The variable can be either true or false, and readers can wait for a specific value they want.
#[derive(Debug)]
pub struct Condition {
    state: StdMutex<(bool, Vec<Waker>)>,
}

impl Default for Condition {
    fn default() -> Self {
        Self { state: StdMutex::new((false, Vec::new())) }
    }
}

impl Condition {
    /// Set the condition value, triggering waiters if the value changed.
    ///
    /// If the value stayed the same, wakers will not be notified.
    ///
    /// Returns true if the value changed, false otherwise.
    pub fn set(&self, value: bool) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.0 == value {
            return false;
        }
        state.0 = value;
        for waker in state.1.drain(..) {
            waker.wake();
        }
        return true;
    }

    /// Create a future that resolves once the value matches expected.
    pub fn wait(self: &Arc<Self>, expected: bool) -> WaitFuture {
        WaitFuture { condition: Arc::downgrade(self), expected }
    }
}

/// Future for Condition.wait()
pub struct WaitFuture {
    condition: Weak<Condition>,
    expected: bool,
}

impl Future for WaitFuture {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let condition = self.condition.upgrade();
        if condition.is_none() {
            // Release waiters if the condition is just gone.
            return Poll::Ready(());
        }

        let condition = condition.unwrap();

        let mut state = condition.state.lock().unwrap();

        if state.0 == self.expected {
            Poll::Ready(())
        } else {
            state.1.push(cx.waker().clone());
            Poll::Pending
        }
    }
}
