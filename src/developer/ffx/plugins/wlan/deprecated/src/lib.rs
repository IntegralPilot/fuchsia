// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use async_trait::async_trait;
use fdomain_fuchsia_wlan_product_deprecatedconfiguration as wlan_deprecated;
use ffx_wlan_deprecated_args as arg_types;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use target_holders::moniker;

#[derive(FfxTool)]
pub struct DeprecatedTool {
    #[command]
    cmd: arg_types::DeprecatedCommand,
    #[with(moniker("/core/wlancfg"))]
    proxy: wlan_deprecated::DeprecatedConfiguratorProxy,
}

fho::embedded_plugin!(DeprecatedTool);

#[async_trait(?Send)]
impl FfxMain for DeprecatedTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        handle_deprecated_command(self.proxy, self.cmd).await?;
        Ok(())
    }
}

async fn handle_deprecated_command(
    proxy: wlan_deprecated::DeprecatedConfiguratorProxy,
    cmd: arg_types::DeprecatedCommand,
) -> Result<(), Error> {
    match cmd.subcommand {
        arg_types::DeprecatedSubCommand::SuggestMac(mac) => {
            donut_lib_fdomain::handle_suggest_ap_mac(proxy, mac.mac).await
        }
    }
}
