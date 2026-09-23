// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow};
use flex_client::fidl::ServerEnd;
use flex_fuchsia_audio_controller as fac;
use futures::TryStreamExt;
use std::sync::atomic::{AtomicBool, Ordering};

pub mod dai;
pub mod device;
pub mod format;
pub mod format_set;
pub mod registry;
pub mod sigproc;
#[cfg(target_os = "fuchsia")]
pub mod vmo_buffer;

pub use format::{Format, parse_duration, str_to_clock};
pub use registry::Registry;
#[cfg(target_os = "fuchsia")]
pub use vmo_buffer::VmoBuffer;

pub async fn stop_listener(
    canceler: ServerEnd<fac::RecordCancelerMarker>,
    stop_signal: &AtomicBool,
) -> Result<(), Error> {
    let mut stream = canceler.into_stream();

    let item = stream.try_next().await;
    stop_signal.store(true, Ordering::SeqCst);

    match item {
        Ok(Some(request)) => match request {
            fac::RecordCancelerRequest::Cancel { responder } => {
                responder.send(Ok(())).context("FIDL error with stop request")
            }
            _ => Err(anyhow!("Unimplemented method on canceler")),
        },
        Ok(None) => Ok(()),
        Err(e) => Err(anyhow!("FIDL error with stop request: {e}")),
    }
}
