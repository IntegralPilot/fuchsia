// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use async_trait::async_trait;
use fdomain_fuchsia_wlan_policy as wlan_policy;
use ffx_wlan_client_args as arg_types;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use target_holders::moniker;

#[derive(FfxTool)]
pub struct ClientTool {
    #[command]
    cmd: arg_types::ClientCommand,
    #[with(moniker("/core/wlancfg"))]
    client_provider: wlan_policy::ClientProviderProxy,
    #[with(moniker("/core/wlancfg"))]
    client_listener: wlan_policy::ClientListenerProxy,
}

fho::embedded_plugin!(ClientTool);

#[async_trait(?Send)]
impl FfxMain for ClientTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        handle_client_command(self.client_provider, self.client_listener, self.cmd).await?;
        Ok(())
    }
}

async fn handle_client_command(
    client_provider: wlan_policy::ClientProviderProxy,
    client_listener: wlan_policy::ClientListenerProxy,
    cmd: arg_types::ClientCommand,
) -> Result<(), Error> {
    let (client_controller, _) = ffx_wlan_common::get_client_controller(client_provider).await?;
    let listener_stream = ffx_wlan_common::get_client_listener_stream(client_listener)?;

    match cmd.subcommand {
        arg_types::ClientSubCommand::BatchConfig(batch_cmd) => match batch_cmd.subcommand {
            arg_types::BatchConfigSubCommand::Dump(arg_types::Dump {}) => {
                let saved_networks =
                    donut_lib_fdomain::handle_get_saved_networks(&client_controller).await?;
                donut_lib_fdomain::print_serialized_saved_networks(saved_networks)
            }
            arg_types::BatchConfigSubCommand::Restore(arg_types::Restore { serialized_config }) => {
                donut_lib_fdomain::restore_serialized_config(client_controller, serialized_config)
                    .await
            }
        },
        arg_types::ClientSubCommand::Connect(connect_args) => {
            let security = connect_args.security_type.map(|s| s.into());
            donut_lib_fdomain::handle_connect(
                client_controller,
                listener_stream,
                connect_args.ssid,
                security,
            )
            .await
        }
        arg_types::ClientSubCommand::List(arg_types::ListSavedNetworks {}) => {
            let saved_networks =
                donut_lib_fdomain::handle_get_saved_networks(&client_controller).await?;
            donut_lib_fdomain::print_saved_networks(saved_networks)
        }
        arg_types::ClientSubCommand::Listen(arg_types::Listen {}) => {
            donut_lib_fdomain::handle_listen(listener_stream, false).await
        }
        arg_types::ClientSubCommand::Status(arg_types::Status {}) => {
            donut_lib_fdomain::handle_listen(listener_stream, true).await
        }
        arg_types::ClientSubCommand::ForgetNetwork(forget_args) => {
            donut_lib_fdomain::handle_forget_network(
                client_controller,
                forget_args.ssid.into_bytes(),
                forget_args.security_type.map(|s| s.into()),
            )
            .await
        }
        arg_types::ClientSubCommand::SaveNetwork(config_args) => {
            let network_config = wlan_policy::NetworkConfig::from(config_args);
            donut_lib_fdomain::handle_save_network(client_controller, network_config).await
        }
        arg_types::ClientSubCommand::Scan(arg_types::Scan {}) => {
            let scan_results = donut_lib_fdomain::handle_scan(client_controller).await?;
            donut_lib_fdomain::print_scan_results(scan_results)
        }
        arg_types::ClientSubCommand::Start(arg_types::StartClientConnections {}) => {
            donut_lib_fdomain::handle_start_client_connections(client_controller).await
        }
        arg_types::ClientSubCommand::Stop(arg_types::StopClientConnections {}) => {
            donut_lib_fdomain::handle_stop_client_connections(client_controller).await
        }
    }
}
