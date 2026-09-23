// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;
pub mod subcommands;

use anyhow::{Context, Result};
use args::{CompositeCommand, CompositeSubcommand};
use flex_fuchsia_driver_development as fdd;
use std::io::Write;

pub async fn composite(
    cmd: CompositeCommand,
    writer: &mut dyn Write,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    match cmd.subcommand {
        CompositeSubcommand::List(subcmd) => {
            subcommands::list::list(subcmd, writer, driver_development_proxy)
                .await
                .context("List composite subcommand failed")?;
        }
        CompositeSubcommand::Show(subcmd) => {
            subcommands::show::show(subcmd, writer, driver_development_proxy)
                .await
                .context("Show composite subcommand failed")?;
        }
    }
    Ok(())
}
