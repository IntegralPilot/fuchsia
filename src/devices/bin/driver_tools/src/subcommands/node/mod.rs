// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;
pub mod common;

pub mod subcommands;

use anyhow::{Context, Result};
use args::{NodeCommand, NodeSubcommand};
use flex_fuchsia_driver_development as fdd;
use std::io::Write;

pub async fn node(
    cmd: NodeCommand,
    writer: &mut dyn Write,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    match cmd.subcommand {
        NodeSubcommand::List(subcmd) => {
            subcommands::list::list_node(subcmd, writer, driver_development_proxy)
                .await
                .context("List subcommand failed")?;
        }
        NodeSubcommand::Show(subcmd) => {
            subcommands::show::show_node(subcmd, writer, driver_development_proxy)
                .await
                .context("Show subcommand failed")?;
        }
        NodeSubcommand::Add(ref subcmd) => {
            subcommands::add::add_node(subcmd, driver_development_proxy)
                .await
                .context("Add subcommand failed")?;
        }
        NodeSubcommand::Remove(ref subcmd) => {
            subcommands::remove::remove_node(subcmd, driver_development_proxy)
                .await
                .context("Remove subcommand failed")?;
        }
        NodeSubcommand::Graph(subcmd) => {
            subcommands::graph::graph_node(subcmd, writer, driver_development_proxy)
                .await
                .context("Graph subcommand failed")?;
        }
    };
    Ok(())
}

pub async fn node_machine(
    cmd: &NodeCommand,
    driver_development_proxy: &fdd::ManagerProxy,
) -> Result<Option<serde_json::Value>> {
    match &cmd.subcommand {
        NodeSubcommand::List(subcmd) => {
            let nodes = subcommands::list::get_nodes(subcmd, driver_development_proxy).await?;
            Ok(Some(serde_json::to_value(&nodes)?))
        }
        NodeSubcommand::Show(subcmd) => {
            let details =
                subcommands::show::get_node_details(subcmd, driver_development_proxy).await?;
            Ok(Some(serde_json::to_value(&details)?))
        }
        _ => Ok(None),
    }
}
