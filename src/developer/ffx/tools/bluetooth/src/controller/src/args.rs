// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use fuchsia_bluetooth::types::HostId;

#[path = "local_name/args.rs"]
pub mod local_name;

#[path = "device_class/args.rs"]
pub mod device_class;

// ffx bluetooth controller
#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "controller",
    description = "Interact with the Bluetooth controller(s) available to the system.",
    example = "ffx bluetooth controller"
)]
pub struct ControllerCommand {
    #[argh(subcommand)]
    pub subcommand: ControllerSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(subcommand)]
pub enum ControllerSubCommand {
    Show(ShowCommand),
    List(ListCommand),
    Set(SetCommand),
    LocalName(local_name::LocalNameCommand),
    DeviceClass(device_class::DeviceClassCommand),
}
/// ffx bluetooth controller show
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "show",
    description = "Show details about the active Bluetooth controller.",
    example = "ffx bluetooth controller show"
)]
pub struct ShowCommand {}

/// ffx bluetooth controller list
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "list",
    description = "List information about all Bluetooth controllers available to the system.",
    example = "ffx bluetooth controller list"
)]
pub struct ListCommand {}

/// ffx bluetooth controller set
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "set",
    description = "Set the active Bluetooth controller.",
    example = "ffx bluetooth controller set <id>"
)]
pub struct SetCommand {
    #[argh(positional)]
    pub id: HostId,
}
