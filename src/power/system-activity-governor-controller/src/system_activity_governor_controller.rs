// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use log::{error, info, warn};
use std::cell::RefCell;
use std::rc::Rc;
use {
    fidl_fuchsia_power_system as fsystem, fidl_fuchsia_power_topology_test as fpt,
    fuchsia_async as fasync,
};

const SYSTEM_ACTIVITY_GOVERNOR_CONTROLLER: &'static str = "system_activity_governor_controller";

enum IncomingRequest {
    SystemActivityControl(fpt::SystemActivityControlRequestStream),
}

/// SystemActivityGovernorController runs the server for fuchsia.power.topology.test.SystemActivityControl.
pub struct SystemActivityGovernorController {
    // Holds token for application activity.
    application_activity_token: RefCell<Option<fsystem::LeaseToken>>,
}

impl SystemActivityGovernorController {
    pub fn new() -> Rc<Self> {
        Rc::new(Self { application_activity_token: RefCell::new(None) })
    }

    pub async fn run(self: Rc<Self>) -> Result<()> {
        info!("Starting FIDL server");
        let mut service_fs = ServiceFs::new_local();

        service_fs.dir("svc").add_fidl_service(IncomingRequest::SystemActivityControl);
        service_fs
            .take_and_serve_directory_handle()
            .context("failed to serve outgoing namespace")?;

        service_fs
            .for_each_concurrent(None, move |request: IncomingRequest| {
                let ttd = self.clone();
                async move {
                    match request {
                        IncomingRequest::SystemActivityControl(stream) => {
                            fasync::Task::local(ttd.handle_system_activity_request(stream)).detach()
                        }
                    }
                }
            })
            .await;
        Ok(())
    }

    async fn handle_system_activity_request(
        self: Rc<Self>,
        mut stream: fpt::SystemActivityControlRequestStream,
    ) {
        while let Some(request) = stream.next().await {
            match request {
                Ok(fpt::SystemActivityControlRequest::StartApplicationActivity { responder }) => {
                    let result = responder.send(self.start_application_activity().await);

                    if let Err(error) = result {
                        warn!(error:?; "Error while responding to StartApplicationActivity request");
                    }
                }
                Ok(fpt::SystemActivityControlRequest::StopApplicationActivity { responder }) => {
                    let result = responder.send(self.stop_application_activity());

                    if let Err(error) = result {
                        warn!(error:?; "Error while responding to StopApplicationActivity request");
                    }
                }
                Ok(fpt::SystemActivityControlRequest::RestartApplicationActivity {
                    responder,
                    wait_time_ns,
                }) => {
                    let result =
                        responder.send(self.restart_application_activity(wait_time_ns).await);

                    if let Err(error) = result {
                        warn!(error:?; "Error while responding to StopApplicationActivity request");
                    }
                }
                Ok(fpt::SystemActivityControlRequest::_UnknownMethod { ordinal, .. }) => {
                    warn!(ordinal:?; "Unknown ActivityGovernorRequest method");
                }
                Err(error) => {
                    error!(error:?; "Error handling SystemActivityControl request stream");
                }
            }
        }
    }

    async fn start_application_activity(
        self: &Self,
    ) -> fpt::SystemActivityControlStartApplicationActivityResult {
        let sag = connect_to_protocol::<fsystem::ActivityGovernorMarker>().map_err(|err| {
            error!(err:%; "Failed to connect to fuchsia.power.system");
            fpt::SystemActivityControlError::Internal
        })?;

        let token = sag
            .take_application_activity_lease(SYSTEM_ACTIVITY_GOVERNOR_CONTROLLER)
            .await
            .map_err(|err| {
                error!(err:%; "Failed to take application activity lease");
                fpt::SystemActivityControlError::Internal
            })?;
        info!("Took lease on application activity");

        self.application_activity_token.borrow_mut().replace(token);
        Ok(())
    }

    fn stop_application_activity(
        self: &Self,
    ) -> fpt::SystemActivityControlStartApplicationActivityResult {
        drop(self.application_activity_token.borrow_mut().take());
        info!("Dropped lease on application activity");
        Ok(())
    }

    async fn restart_application_activity(
        self: &Self,
        wait_time_ns: u64,
    ) -> fpt::SystemActivityControlStartApplicationActivityResult {
        self.stop_application_activity()?;
        fasync::Timer::new(std::time::Duration::from_nanos(wait_time_ns)).await;
        self.start_application_activity().await?;
        Ok(())
    }
}
