// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_power_battery::{ChargerRequest, ChargerRequestStream, ChargerServiceRequest};
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::LocalComponentHandles;
use fuchsia_sync::Mutex;
use futures::prelude::*;
use std::sync::Arc;

pub type ChargerEnableRequests = Arc<Mutex<Vec<bool>>>;

pub async fn mock_charger_service(
    handles: LocalComponentHandles,
    requests: ChargerEnableRequests,
) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service_instance_at(
        "fuchsia.power.battery.ChargerService",
        "mock_charger_service",
        |request: ChargerServiceRequest| {
            let ChargerServiceRequest::Device(stream) = request;
            stream
        },
    );
    fs.serve_connection(handles.outgoing_dir)?;

    fs.for_each_concurrent(0, |stream: ChargerRequestStream| {
        let requests = requests.clone();
        async move {
            stream
                .try_for_each(|request: ChargerRequest| {
                    let requests = requests.clone();
                    async move {
                        match request {
                            ChargerRequest::Enable { enable, responder } => {
                                log::debug!(
                                    "mock_charger_service: Received Enable({}) request",
                                    enable
                                );
                                requests.lock().push(enable);
                                responder.send(Ok(()))?;
                            }
                            _ => unreachable!("Unknown charger request"),
                        }
                        Ok(())
                    }
                })
                .await
                .unwrap()
        }
    })
    .await;

    Ok(())
}
