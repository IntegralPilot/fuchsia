// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ServerEnd;
use fidl_fuchsia_hardware_power_statecontrol::{self as fpower, ShutdownOptions};
use fidl_fuchsia_io::DirectoryMarker;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::LocalComponentHandles;
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::{StreamExt, TryStreamExt};
use log::*;
use std::sync::Arc;

/// Mocks the fuchsia.hardware.power.statecontrol.Admin service to be used in integration tests.
pub struct MockStateControlAdminService {
    shutdown_received_sender: Mutex<mpsc::Sender<ShutdownOptions>>,
    shutdown_received_receiver: Mutex<mpsc::Receiver<ShutdownOptions>>,
}

impl MockStateControlAdminService {
    pub fn new() -> Arc<MockStateControlAdminService> {
        let (sender, receiver) = mpsc::channel(1);
        Arc::new(Self {
            shutdown_received_sender: Mutex::new(sender),
            shutdown_received_receiver: Mutex::new(receiver),
        })
    }

    /// Runs the mock using the provided `LocalComponentHandles`.
    ///
    /// The mock intentionally does not complete reboot requests in order to prevent further calls
    /// that may cause extra send.
    ///
    /// Expected usage is to call this function from a closure for the
    /// `local_component_implementation` parameter to `RealmBuilder.add_local_child`.
    ///
    /// For example:
    ///     let mock_admin_service = MockStateControlAdminService::new();
    ///     let admin_service_child = realm_builder
    ///         .add_local_child(
    ///             "admin_service",
    ///             move |handles| {
    ///                 Box::pin(mock_admin_service.clone().run(handles))
    ///             },
    ///             ChildOptions::new(),
    ///         )
    ///         .await
    ///         .unwrap();
    ///
    pub async fn run(self: Arc<Self>, handles: LocalComponentHandles) -> Result<(), anyhow::Error> {
        self.run_inner(handles.outgoing_dir).await
    }

    async fn run_inner(
        self: Arc<Self>,
        outgoing_dir: ServerEnd<DirectoryMarker>,
    ) -> Result<(), anyhow::Error> {
        let mut fs = ServiceFs::new();
        fs.dir("svc").add_fidl_service(move |mut stream: fpower::AdminRequestStream| {
            let this = self.clone();
            fasync::Task::local(async move {
                info!("MockStateControlAdminService: new connection Admin");
                while let Some(request) = stream.try_next().await.unwrap() {
                    match request {
                        fpower::AdminRequest::Shutdown { options, responder: _ } => {
                            info!("MockStateControlAdminService: received Shutdown request");
                            this.shutdown_received_sender
                                .lock()
                                .await
                                .try_send(options)
                                .expect("Failed to notify shutdown");
                        }
                        _ => {
                            unimplemented!();
                        }
                    }
                }
            })
            .detach();
        });

        fs.serve_connection(outgoing_dir).unwrap();
        fs.collect::<()>().await;

        Ok(())
    }

    /// Waits for the mock to receive a fidl.fuchsia.SystemController/Shutdown request.
    pub async fn wait_for_shutdown_request(&self) -> ShutdownOptions {
        self.shutdown_received_receiver
            .lock()
            .await
            .next()
            .await
            .expect("Failed to wait for shutdown request")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_component::client::connect_to_protocol_at_dir_svc;

    #[fuchsia::test]
    async fn test_shutdown() {
        // Create and serve the mock service
        let (dir, outgoing_dir) = fidl::endpoints::create_proxy::<DirectoryMarker>();
        let mock = MockStateControlAdminService::new();
        let _task = fasync::Task::local(mock.clone().run_inner(outgoing_dir));

        // Connect to the mock server
        let controller_client =
            connect_to_protocol_at_dir_svc::<fpower::AdminMarker>(&dir).unwrap();

        // Call the server's `shutdown` method and verify the mock sees the request
        let _task = fuchsia_async::Task::local(controller_client.shutdown(&ShutdownOptions {
            action: Some(fpower::ShutdownAction::Reboot),
            reasons: Some(vec![fpower::ShutdownReason::HighTemperature]),
            ..Default::default()
        }));
        mock.wait_for_shutdown_request().await;
    }
}
