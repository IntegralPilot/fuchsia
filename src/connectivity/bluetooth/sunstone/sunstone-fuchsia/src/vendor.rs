// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::pin::pin;
use fidl::endpoints::Proxy as _;
use fidl_fuchsia_hardware_bluetooth as hardware;
use futures_util::future::select;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Failed to connect to Vendor protocol at {path}: {status}")]
    ServiceConnect { path: String, status: zx::Status },
    #[error("FIDL error calling open_hci_transport: {0}")]
    Fidl(#[from] fidl::Error),
    #[error("open_hci_transport returned error status: {0:?}")]
    OpenHciTransport(zx::Status),
}

/// FIDL client for the `fuchsia.hardware.bluetooth.Vendor` and `HciTransport` protocols.
#[derive(Debug)]
pub struct Vendor {
    vendor_proxy: hardware::VendorProxy,
    hci_transport: hardware::HciTransportProxy,
}

impl Vendor {
    /// Connects to the Vendor protocol at `device_path` and opens the HCI transport.
    pub async fn connect(device_path: &str) -> Result<Self, Error> {
        let (vendor_proxy, server_end) = fidl::endpoints::create_proxy::<hardware::VendorMarker>();
        fdio::service_connect(device_path, server_end.into_channel())
            .map_err(|status| Error::ServiceConnect { path: device_path.to_string(), status })?;

        let client_end = vendor_proxy
            .open_hci_transport()
            .await?
            .map_err(|status| Error::OpenHciTransport(zx::Status::err_from_raw(status)))?;

        let hci_transport = client_end.into_proxy();

        Ok(Self { vendor_proxy, hci_transport })
    }

    /// Returns a future that completes when either the `VendorProxy` or `HciTransportProxy` closes.
    pub async fn on_closed(&self) {
        let vendor_closed = pin!(self.vendor_proxy.on_closed());
        let hci_closed = pin!(self.hci_transport.on_closed());
        let _ = select(vendor_closed, hci_closed).await;
    }
}
