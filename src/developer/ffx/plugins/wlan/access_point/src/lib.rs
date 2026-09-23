// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use async_trait::async_trait;
use fdomain_fuchsia_wlan_policy as wlan_policy;
use ffx_wlan_ap_args as arg_types;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use target_holders::moniker;

#[derive(FfxTool)]
pub struct AccessPointTool {
    #[command]
    cmd: arg_types::ApCommand,
    #[with(moniker("/core/wlancfg"))]
    ap_provider: wlan_policy::AccessPointProviderProxy,
    #[with(moniker("/core/wlancfg"))]
    ap_listener: wlan_policy::AccessPointListenerProxy,
}

fho::embedded_plugin!(AccessPointTool);

#[async_trait(?Send)]
impl FfxMain for AccessPointTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        handle_client_command(self.ap_provider, self.ap_listener, self.cmd).await?;
        Ok(())
    }
}

async fn handle_client_command(
    ap_provider: wlan_policy::AccessPointProviderProxy,
    ap_listener: wlan_policy::AccessPointListenerProxy,
    cmd: arg_types::ApCommand,
) -> Result<(), Error> {
    let (ap_controller, _) = ffx_wlan_common::get_ap_controller(ap_provider).await?;
    let listener_stream = ffx_wlan_common::get_ap_listener_stream(ap_listener)?;

    match cmd.subcommand {
        arg_types::ApSubCommand::Listen(arg_types::Listen {}) => {
            donut_lib_fdomain::handle_ap_listen(listener_stream, false).await
        }
        arg_types::ApSubCommand::Status(arg_types::Status {}) => {
            donut_lib_fdomain::handle_ap_listen(listener_stream, true).await
        }
        arg_types::ApSubCommand::Start(config) => {
            let config = wlan_policy::NetworkConfig::from(config);
            donut_lib_fdomain::handle_start_ap(ap_controller, listener_stream, config).await
        }
        arg_types::ApSubCommand::Stop(config) => {
            let config = wlan_policy::NetworkConfig::from(config);
            donut_lib_fdomain::handle_stop_ap(ap_controller, config).await
        }
        arg_types::ApSubCommand::StopAll(arg_types::StopAll {}) => {
            donut_lib_fdomain::handle_stop_all_aps(ap_controller).await
        }
    }
}
