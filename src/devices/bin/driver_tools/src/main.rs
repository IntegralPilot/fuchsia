// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use driver_tools::args::DriverCommand;
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_registrar as fdr;
use fidl_fuchsia_test_manager as ftm;
use fuchsia_component::client;

struct DriverConnector {}

impl DriverConnector {
    fn new() -> Self {
        Self {}
    }
}

#[async_trait::async_trait]
impl driver_connector::DriverConnector for DriverConnector {
    async fn get_driver_development_proxy(&self, select: bool) -> Result<fdd::ManagerProxy> {
        if select {
            anyhow::bail!(
                "The 'driver' tool cannot use the select flag. Please use 'ffx driver' in order to select a component."
            );
        }
        client::connect_to_protocol::<fdd::ManagerMarker>()
            .context("Failed to connect to driver development service")
    }

    async fn get_driver_registrar_proxy(&self, select: bool) -> Result<fdr::DriverRegistrarProxy> {
        if select {
            anyhow::bail!(
                "The 'driver' tool cannot use the select flag. Please use 'ffx driver' in order to select a component."
            );
        }
        client::connect_to_protocol::<fdr::DriverRegistrarMarker>()
            .context("Failed to connect to driver registrar service")
    }

    async fn get_suite_runner_proxy(&self) -> Result<ftm::SuiteRunnerProxy> {
        client::connect_to_protocol::<ftm::SuiteRunnerMarker>()
            .context("Failed to connect to test manager service")
    }
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let cmd: DriverCommand = argh::from_env();
    driver_tools::driver(cmd, DriverConnector::new(), &mut std::io::stdout()).await
}
