// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fuchsia_component::client;
use powercli::args::PowerCommand;
use {
    fidl_fuchsia_power as fpower, fidl_fuchsia_power_manager_debug as fdebug,
    fidl_fuchsia_power_topology_test as fpt,
};

struct Connector {}

impl Connector {
    fn new() -> Self {
        Self {}
    }
}

impl powercli::connector::Connector for Connector {
    async fn get_system_activity_control(&self) -> Result<fpt::SystemActivityControlProxy> {
        client::connect_to_protocol::<fpt::SystemActivityControlMarker>()
            .context("Failed to connect to system activity control service")
    }
    async fn get_debug(&self) -> Result<fdebug::DebugProxy> {
        client::connect_to_protocol::<fdebug::DebugMarker>()
            .context("Failed to connect to power manager debug service")
    }
    async fn get_reboot_initiator(&self) -> Result<fpower::CollaborativeRebootInitiatorProxy> {
        client::connect_to_protocol::<fpower::CollaborativeRebootInitiatorMarker>()
            .context("Failed to connect to collaborative reboot initiator service")
    }
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let cmd: PowerCommand = argh::from_env();
    powercli::power(cmd, Connector::new(), &mut std::io::stdout()).await
}
