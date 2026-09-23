// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! # Memory Sampler
//!
//! Memory Sampler is a component that serves the
//! `fuchsia.memory.sampler/Sampler` protocol, used by applications to
//! report their memory usage. It collects allocation information
//! across the lifetime of a process('s connection), and produces
//! `pprof`-compatible profile files once done.
mod crash_reporter;
mod pprof;
mod profile_builder;
mod sampler_service;

use anyhow::Error;
use fidl_fuchsia_feedback::CrashReporterMarker;
use fuchsia_component::client::connect_to_protocol;
use futures::try_join;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    crash_reporter::register_crash_product().await?;
    let crash_reporter = connect_to_protocol::<CrashReporterMarker>()?;
    let (tx, crash_reporter_service) = crash_reporter::setup_crash_reporter_task(crash_reporter);
    let sampler_service = sampler_service::setup_sampler_service(tx)?;
    try_join!(crash_reporter_service, sampler_service)?;
    Ok(())
}
