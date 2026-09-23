// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;
mod common;
pub mod subcommands;

use anyhow::{Context, Result};
use args::{DriverCommand, DriverSubCommand};
use std::io;
use subcommands::host::args::HostSubcommand;
use subcommands::node::args::NodeSubcommand;

#[cfg(feature = "fdomain")]
use driver_connector_fdomain as driver_connector;

use driver_connector::DriverConnector;

pub async fn driver(
    cmd: DriverCommand,
    driver_connector: impl DriverConnector,
    writer: &mut dyn io::Write,
) -> Result<()> {
    match cmd.subcommand {
        DriverSubCommand::Disable(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::disable::disable(*subcmd.0, writer, driver_development_proxy)
                .await
                .context("Disable subcommand failed")?;
        }
        DriverSubCommand::Enable(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::enable::enable(*subcmd.0, writer, driver_development_proxy)
                .await
                .context("Enable subcommand failed")?;
        }
        DriverSubCommand::Doctor(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::doctor::doctor(*subcmd.0, driver_development_proxy, writer)
                .await
                .context("Doctor subcommand failed")?;
        }
        DriverSubCommand::Dump(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::dump::dump(*subcmd.0, writer, driver_development_proxy)
                .await
                .context("Dump subcommand failed")?;
        }
        DriverSubCommand::List(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::list::list(*subcmd.0, writer, driver_development_proxy)
                .await
                .context("List subcommand failed")?;
        }
        DriverSubCommand::ListComposites(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::list_composites::list_composites(
                *subcmd.0,
                writer,
                driver_development_proxy,
            )
            .await
            .context("ListComposites subcommand failed")?;
        }
        DriverSubCommand::ListDevices(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::list_devices::list_devices(*subcmd.0, driver_development_proxy)
                .await
                .context("ListDevices subcommand failed")?;
        }
        DriverSubCommand::ListHosts(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::list_hosts::list_hosts(*subcmd.0, driver_development_proxy)
                .await
                .context("ListHosts subcommand failed")?;
        }
        DriverSubCommand::ListCompositeNodeSpecs(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::list_composite_node_specs::list_composite_node_specs(
                *subcmd.0,
                writer,
                driver_development_proxy,
            )
            .await
            .context("ListCompositeNodeSpecs subcommand failed")?;
        }
        DriverSubCommand::Register(subcmd) => {
            let driver_registrar_proxy = driver_connector
                .get_driver_registrar_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver registrar proxy")?;
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::register::register(
                *subcmd.0,
                writer,
                driver_registrar_proxy,
                driver_development_proxy,
            )
            .await
            .context("Register subcommand failed")?;
        }
        DriverSubCommand::Restart(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::restart::restart(*subcmd.0, writer, driver_development_proxy)
                .await
                .context("Restart subcommand failed")?;
        }
        DriverSubCommand::Show(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::show::show(*subcmd.0, writer, driver_development_proxy)
                .await
                .context("Show subcommand failed")?;
        }
        DriverSubCommand::TestNode(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(subcmd.0.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::test_node::test_node(&subcmd.0, driver_development_proxy)
                .await
                .context("TestNode subcommand failed")?;
        }
        DriverSubCommand::Node(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(cmd.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::node::node(*subcmd.0, writer, driver_development_proxy)
                .await
                .context("Node subcommand failed")?;
        }
        DriverSubCommand::Host(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(cmd.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::host::host(*subcmd.0, writer, driver_development_proxy)
                .await
                .context("Host subcommand failed")?;
        }
        DriverSubCommand::Composite(subcmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(cmd.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::composite::composite(*subcmd.0, writer, driver_development_proxy)
                .await
                .context("Composite subcommand failed")?;
        }
        #[cfg(not(target_os = "fuchsia"))]
        DriverSubCommand::StaticChecks(subcmd) => {
            static_checks_lib::static_checks(*subcmd.0, writer)
                .await
                .context("StaticChecks subcommand failed")?;
        }
    }

    Ok(())
}

pub fn is_machine_supported(cmd: &DriverCommand) -> bool {
    match &cmd.subcommand {
        DriverSubCommand::Host(host_cmd) => {
            matches!(host_cmd.0.subcommand, HostSubcommand::List(_) | HostSubcommand::Show(_))
        }
        DriverSubCommand::Node(node_cmd) => {
            matches!(node_cmd.0.subcommand, NodeSubcommand::List(_) | NodeSubcommand::Show(_))
        }
        _ => false,
    }
}

pub async fn driver_machine(
    cmd: DriverCommand,
    driver_connector: impl DriverConnector,
) -> Result<Option<serde_json::Value>> {
    match cmd.subcommand {
        DriverSubCommand::Node(node_cmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(cmd.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::node::node_machine(&node_cmd.0, &driver_development_proxy).await
        }
        DriverSubCommand::Host(host_cmd) => {
            let driver_development_proxy = driver_connector
                .get_driver_development_proxy(cmd.select)
                .await
                .context("Failed to get driver development proxy")?;
            subcommands::host::host_machine(&host_cmd.0, &driver_development_proxy).await
        }
        _ => Ok(None),
    }
}
