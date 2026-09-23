// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

pub mod subcommands;

use anyhow::{Context, Result};
use args::{HostCommand, HostSubcommand};
use flex_fuchsia_driver_development as fdd;
use std::io::Write;

pub async fn host(
    cmd: HostCommand,
    writer: &mut dyn Write,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    match cmd.subcommand {
        HostSubcommand::List(subcmd) => {
            subcommands::list::list(subcmd, writer, driver_development_proxy)
                .await
                .context("List subcommand failed")?;
        }
        HostSubcommand::Show(subcmd) => {
            subcommands::show::show(subcmd, writer, driver_development_proxy)
                .await
                .context("Show subcommand failed")?;
        }
    };
    Ok(())
}

pub async fn host_machine(
    cmd: &HostCommand,
    driver_development_proxy: &fdd::ManagerProxy,
) -> Result<Option<serde_json::Value>> {
    match &cmd.subcommand {
        HostSubcommand::List(_) => {
            let hosts = subcommands::list::get_driver_hosts(driver_development_proxy).await?;
            Ok(Some(serde_json::to_value(&hosts)?))
        }
        HostSubcommand::Show(subcmd) => {
            let details =
                subcommands::show::get_driver_host_details(subcmd, driver_development_proxy)
                    .await?;
            Ok(Some(serde_json::to_value(&details)?))
        }
    }
}
