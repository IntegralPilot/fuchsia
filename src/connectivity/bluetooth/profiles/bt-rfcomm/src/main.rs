// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

use anyhow::{Context as _, Error};
use fidl_fuchsia_bluetooth_bredr::ProfileMarker;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect_derive::Inspect;
use futures::channel::mpsc;
use futures::future;
use log::{debug, info, warn};
use std::pin::pin;

mod fidl_service;
mod profile;
mod profile_registrar;
mod rfcomm;
mod types;

use crate::fidl_service::run_services;
use crate::profile_registrar::ProfileRegistrar;

const THREAD_ROLE_NAME: &str = "fuchsia.bluetooth.rfcomm";

#[fuchsia::main]
pub async fn main() -> Result<(), Error> {
    match fuchsia_scheduler::set_role_for_this_thread(THREAD_ROLE_NAME) {
        Ok(()) => info!("Thread role set successfully."),
        Err(e) => warn!(e:%; "Failed to set thread role."),
    }

    let profile_svc = fuchsia_component::client::connect_to_protocol::<ProfileMarker>()
        .context("Failed to connect to Bluetooth Profile service")?;

    let (service_sender, service_receiver) = mpsc::channel(1);

    let fs = ServiceFs::new();

    let inspect = fuchsia_inspect::Inspector::default();
    let _inspect_server_task =
        inspect_runtime::publish(&inspect, inspect_runtime::PublishOptions::default());

    let services = pin!(run_services(fs, service_sender)?);

    let mut profile_registrar = ProfileRegistrar::new(profile_svc);
    if let Err(e) = profile_registrar.iattach(inspect.root(), "rfcomm_server") {
        warn!("Failed to attach to inspect: {}", e);
    }
    let profile_registrar_fut = profile_registrar.start(service_receiver);
    debug!("RFCOMM component running");
    match future::select(services, profile_registrar_fut).await {
        future::Either::Left(((), _)) => {
            warn!("Service FS directory handle closed. Exiting.");
        }
        future::Either::Right(((), _)) => {
            warn!("All Profile related connections have terminated. Exiting.");
        }
    }

    Ok(())
}
