// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The code in this module is useful, but does not meet the team's quality
//! bar. See go/fuchsia-display-rough

// TODO(https://fxbug.dev/559080325): Replace with shared input-report-reader crate once available.

use crate::hardware_integration::fidl_input_device::FidlInputDevice;
use fidl_fuchsia_device_fs as fidl_legacy_device_fs;
use fidl_fuchsia_driver_framework as fidl_legacy_driver_framework;
use fidl_next_fuchsia_input_report as fidl_input_report;
use futures::TryStreamExt;

/// Handles devfs connector requests for the input device.
pub struct DevfsHandler {
    fidl_input_device: FidlInputDevice,
}

impl DevfsHandler {
    pub fn new(fidl_input_device: FidlInputDevice) -> Self {
        Self { fidl_input_device }
    }

    /// Spawns the connector service loop and returns the
    /// [`fidl_legacy_driver_framework::DevfsAddArgs`] and the background task.
    pub fn serve(&self) -> (fidl_legacy_driver_framework::DevfsAddArgs, fuchsia_async::Task<()>) {
        let (connector_client_end, connector_server_end) =
            fidl::endpoints::create_endpoints::<fidl_legacy_device_fs::ConnectorMarker>();
        let device_clone = self.fidl_input_device.clone();

        let task = fuchsia_async::Task::spawn(async move {
            let mut stream = connector_server_end.into_stream();
            while let Ok(Some(request)) = stream.try_next().await {
                match request {
                    fidl_legacy_device_fs::ConnectorRequest::Connect { server, .. } => {
                        let server_end =
                            fidl_next::ServerEnd::<fidl_input_report::InputDevice, _>::from_untyped(
                                server,
                            );
                        #[expect(unused)]
                        let server_task = server_end.spawn(device_clone.clone());
                    }
                }
            }
        });

        let devfs_args = fidl_legacy_driver_framework::DevfsAddArgs {
            connector: Some(connector_client_end),
            class_name: Some("input-report".to_string()),
            ..Default::default()
        };

        (devfs_args, task)
    }
}
