// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A component that serves a FIDL API `fuchsia.time.alarms/Wake`.
//!
//! Used in hermetic integration tests that exercise timekeeper functionality
//! unrelated to UTC clock maintenance.  This is done to remove the need to
//! set up UTC clock maintenance, which brings in complexity that these tests
//! do not need.
//!
//! This fake service uses the real alarm handling logic from the prod `alarms`
//! crate, so the fake's behavior is faithful to the production code.

use anyhow::{Context, Result};
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use std::rc::Rc;
use {fidl_fuchsia_time_alarms as ffta, fuchsia_runtime as fxr};

// This is the production alarms crate.
use alarms;

enum Services {
    Wake(ffta::WakeAlarmsRequestStream),
}

#[fuchsia::main(logging_tags = ["test"])]
async fn main() -> Result<()> {
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    log::debug!("starting fake wake alarms service");

    // In this component, we do not monitor changes to the UTC clock.
    let utc_clock =
        fxr::duplicate_utc_clock_handle(zx::Rights::SAME_RIGHTS).expect("UTC clock is accessible");

    // Provide inspect.
    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default());

    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(Services::Wake);
    fs.take_and_serve_directory_handle()
        .context("while trying to serve fuchsia.time.alarms/Wake")?;

    let scope = fuchsia_async::Scope::new();
    let timer_loop = alarms::connect_to_hrtimer_async()
        .await
        .inspect_err(|e| {
            log::error!("could not connect to the hrtimer driver, wakes will not work: {:?}", &e)
        })
        .map(|proxy| {
            Rc::new(alarms::Loop::new(
                scope.to_handle(),
                proxy,
                inspector.root().create_child("wake_alarms"),
                utc_clock,
            ))
        })?;
    fs.for_each_concurrent(/*limit=*/ None, move |connection| {
        let timer_loop = Rc::clone(&timer_loop);
        async move {
            match connection {
                Services::Wake(stream) => alarms::serve(timer_loop, stream).await,
            }
        }
    })
    .await;
    log::debug!("stopping fake wake alarms service");
    Ok(())
}
