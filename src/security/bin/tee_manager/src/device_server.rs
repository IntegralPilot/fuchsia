// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::provider_server::ProviderServer;
use anyhow::{Context as _, Error};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_hardware_tee::DeviceConnectorProxy;
use fidl_fuchsia_tee as fuchsia_tee;

const STORAGE_DIR: &str = "/data";

/// Serves a `fuchsia.tee.Application` protocol request by passing it through the
/// `TeeDeviceConnection` and specifying the application UUID.
pub async fn serve_application_passthrough(
    uuid: fuchsia_tee::Uuid,
    device_connector: DeviceConnectorProxy,
    server: ServerEnd<fuchsia_tee::ApplicationMarker>,
) -> Result<(), Error> {
    // Create a ProviderServer to support the TEE driver
    let provider = ProviderServer::try_new(STORAGE_DIR)?;
    let (client, stream) = fidl::endpoints::create_request_stream();

    let () = device_connector
        .connect_to_application(&uuid, Some(client), server)
        .context("Could not connect to fuchsia.tee.Application over DeviceConnectorProxy")?;

    provider.serve(stream).await
}

/// Serves a `fuchsia.tee.DeviceInfo` protocol request by passing it through the
/// `TeeDeviceConnection`.
pub async fn serve_device_info_passthrough(
    device_connector: DeviceConnectorProxy,
    server: ServerEnd<fuchsia_tee::DeviceInfoMarker>,
) -> Result<(), Error> {
    device_connector
        .connect_to_device_info(server)
        .context("Could not connect to fuchsia.tee.DeviceInfo over DeviceConnectorProxy")
}
