// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use async_trait::async_trait;
use errors::ffx_bail;
use ffx_config::EnvironmentContext;
use ffx_writer::VerifiedMachineWriter;
use fho::{Deferred, FfxContext, FfxMain, FfxTool, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use target_connector::Connector;
use target_holders::{NodenameHolder, RemoteControlProxyHolder, SshAddrHolder};

pub mod common;
use common::connect_to_rcs;

mod adb;
mod console;
mod kill;
mod vmo;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum StarnixSubCommand {
    Adb(adb::StarnixAdbCommand),
    Console(console::StarnixConsoleCommand),
    Vmo(vmo::StarnixVmoCommand),
    Kill(kill::StarnixKillCommand),
}

#[derive(Debug, JsonSchema, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StarnixToolOutput {
    Adb(adb::AdbCommandOutput),
    Console(console::ConsoleCommandOutput),
    Vmo(vmo::VmoCommandOutput),
    Kill(kill::KillCommandOutput),
}

impl std::fmt::Display for StarnixToolOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Adb(o) => write!(f, "{o}"),
            Self::Console(o) => write!(f, "{o}"),
            Self::Vmo(o) => write!(f, "{o}"),
            Self::Kill(o) => write!(f, "{o}"),
        }
    }
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "starnix", description = "Control starnix containers")]
pub struct StarnixCommand {
    #[argh(subcommand)]
    subcommand: StarnixSubCommand,
}

#[derive(FfxTool)]
pub struct StarnixTool {
    #[command]
    cmd: StarnixCommand,
    rcs_connector: Connector<RemoteControlProxyHolder>,
    ssh_addr: Result<SshAddrHolder>,
    nodename: Deferred<NodenameHolder>,
    context: EnvironmentContext,
}

#[async_trait(?Send)]
impl FfxMain for StarnixTool {
    type Writer = VerifiedMachineWriter<StarnixToolOutput>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        let output = match self.cmd.subcommand {
            StarnixSubCommand::Adb(command) => command
                .run(
                    &self.context,
                    &self.rcs_connector,
                    self.ssh_addr?,
                    self.nodename.await?.into_inner(),
                )
                .await
                .map(StarnixToolOutput::Adb),
            StarnixSubCommand::Console(command) => {
                if cfg!(feature = "enable_console_tool") {
                    let rcs = connect_to_rcs(&self.rcs_connector).await?;
                    console::starnix_console(command, &rcs).await.map(StarnixToolOutput::Console)
                } else {
                    ffx_bail!(
                        "The console tool is intended only for developers. Disabled by default to \
                         prevent automated use. Build with \
                         \"fx set ... --args 'starnix_enable_console_tool = true'\" to enable it."
                    );
                }
            }
            StarnixSubCommand::Vmo(command) => {
                let rcs = connect_to_rcs(&self.rcs_connector).await?;
                vmo::starnix_vmo(command, &rcs).await.map(StarnixToolOutput::Vmo)
            }
            StarnixSubCommand::Kill(command) => {
                let rcs = connect_to_rcs(&self.rcs_connector).await?;
                kill::starnix_kill(command, &rcs).await.map(StarnixToolOutput::Kill)
            }
        }?;
        writer.machine_or_else(&output, || output.to_string()).bug_context("writing output")?;
        Ok(())
    }
}
