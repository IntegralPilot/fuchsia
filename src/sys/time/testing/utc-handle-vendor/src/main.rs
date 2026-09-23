// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Serves writable UTC handles for tests.
//!
//! Some tests require mutable UTC handles, which is not usually available on
//! Fuchsia, except under special circumstances.

use anyhow::{Context, Result};
use fidl_fuchsia_time as fftime;
use fuchsia_component::server::ServiceFs;
use fuchsia_runtime as fxr;
use fuchsia_runtime::UtcClock;
use futures::stream::StreamExt;
use std::rc::Rc;

enum Protocols {
    /// `fuchsia.time/Maintenance`.
    Maintenance(fftime::MaintenanceRequestStream),
}

async fn serve_maintenance(
    mut stream: fftime::MaintenanceRequestStream,
    utc_clock: Rc<UtcClock>,
) -> Result<()> {
    log::debug!("serve_maintenance: entry");
    while let Some(maybe_request) = stream.next().await {
        log::debug!("serve_maintenance: request: {maybe_request:?}");
        match maybe_request {
            Ok(request) => match request {
                fftime::MaintenanceRequest::GetWritableUtcClock { responder, .. } => {
                    let utc_clock_clone: UtcClock =
                        utc_clock.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
                    responder.send(utc_clock_clone.downcast())?;
                }
            },
            Err(err) => {
                log::warn!("serve_maintenance: error: {err:?}");
                break;
            }
        }
    }
    log::warn!("serve_maintenance: exiting");
    Ok(())
}

const HOURS_IN_THE_PAST: i64 = 100;

#[fuchsia::main(logging_tags=["time", "test", "utc-handle-vendor"])]
async fn main() -> Result<()> {
    log::info!("starting utc vendor for tests");

    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(Protocols::Maintenance);
    fs.take_and_serve_directory_handle()
        .context("while trying to serve fuchsia.time/Maintenance")?;

    // Create a new UTC clock handle based off of the current time, and start it. We ensure
    // that backstop is set far enough in the past to not prevent UTC updates in test.
    let utc_now = fxr::utc_time();
    let utc_backstop = utc_now - fxr::UtcDuration::from_hours(HOURS_IN_THE_PAST);
    let boot_now = zx::BootInstant::get();
    let boot_at_backstop = boot_now - zx::BootDuration::from_hours(HOURS_IN_THE_PAST);

    let utc_clock = Rc::new(
        UtcClock::create(zx::ClockOpts::BOOT | zx::ClockOpts::MAPPABLE, Some(utc_backstop))
            .context("while creating UTC clock")?,
    );
    utc_clock
        .set_name(&zx::Name::new_lossy("utc_from_vendor_for_test"))
        .context("while setting the UTC clock name")?;

    // Starts the UTC clock from current time, and 1/1 rate.
    let clock_builder = zx::ClockUpdate::builder().absolute_value(boot_now, utc_now);
    utc_clock.update(clock_builder.build()).context("while updating the test UTC clock")?;

    log::info!("Vendored UTC parameters:");
    log::info!("    - koid              = {:?}", utc_clock.as_handle_ref().koid());
    log::info!("    - boot_at_backstop  = {boot_at_backstop:?}");
    log::info!("    - boot_now          = {boot_now:?}");
    log::info!("    - utc_backstop:     = {utc_backstop:?}");
    log::info!("    - utc_now           = {utc_now:?}\n\n");

    // If you see other components quote the same koid, they are using the vendored UTC clock.

    let utc_clock_factory = || Rc::clone(&utc_clock);
    fs.for_each_concurrent(/*limit=*/ None, move |connection| {
        let utc_clock = utc_clock_factory();
        async move {
            match connection {
                Protocols::Maintenance(stream) => {
                    serve_maintenance(stream, utc_clock)
                        .await
                        .inspect_err(|err| {
                            log::error!("error: {err:?}");
                        })
                        .ok();
                }
            }
        }
    })
    .await;

    log::info!("stopping utc vendor for tests");
    Ok(())
}
