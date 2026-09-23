// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::wlan::types;
use anyhow::{Context as _, Error};
use fidl_fuchsia_wlan_device_service::{DeviceMonitorMarker, DeviceMonitorProxy};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_sync::RwLock;

// WlanFacade: proxies commands from sl4f test to proper fidl APIs
//
// This object is shared among all threads created by server.  The inner object is the facade
// itself.  Callers interact with a wrapped version of the facade that enforces read/write
// protection.
//
// Use: Create once per server instantiation.
#[derive(Debug)]
struct InnerWlanFacade {
    // TODO(https://fxbug.dev/42165549)
    #[allow(unused)]
    scan_results: bool,
}

#[derive(Debug)]
pub(crate) struct WlanFacade {
    monitor_svc: DeviceMonitorProxy,
    // TODO(https://fxbug.dev/42165549)
    #[allow(unused)]
    inner: RwLock<InnerWlanFacade>,
}

impl WlanFacade {
    pub fn new() -> Result<WlanFacade, Error> {
        let monitor_svc = connect_to_protocol::<DeviceMonitorMarker>()?;

        Ok(WlanFacade { monitor_svc, inner: RwLock::new(InnerWlanFacade { scan_results: false }) })
    }

    pub async fn status(&self) -> Result<types::ClientStatusResponseDef, Error> {
        // get the first client interface
        let sme_proxy = wlan_service_util::client::get_first_sme(&self.monitor_svc)
            .await
            .context("Status: failed to get iface sme proxy")?;

        let rsp = sme_proxy.status().await.context("failed to get status from sme_proxy")?;

        Ok(rsp.into())
    }
}
