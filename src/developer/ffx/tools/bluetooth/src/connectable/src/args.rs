// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "connectable",
    description = "Set this device to be connectable or unconnectable.",
    example = "ffx bluetooth connectable"
)]
pub struct ConnectableCommand {
    #[argh(subcommand)]
    pub subcommand: ConnectableSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(subcommand)]
pub enum ConnectableSubCommand {
    Start(StartCommand),
    Stop(StopCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "start",
    description = "Make this device connectable.",
    example = "ffx bluetooth connectable start"
)]
pub struct StartCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "stop",
    description = "Revoke this device's connectability.",
    example = "ffx bluetooth connectable stop"
)]
pub struct StopCommand {}
