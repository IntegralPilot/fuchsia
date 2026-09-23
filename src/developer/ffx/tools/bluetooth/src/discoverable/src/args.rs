// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "discoverable",
    description = "Set this device to be discoverable or undiscoverable.",
    example = "ffx bluetooth discoverable"
)]
pub struct DiscoverableCommand {
    #[argh(subcommand)]
    pub subcommand: DiscoverableSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(subcommand)]
pub enum DiscoverableSubCommand {
    Start(StartCommand),
    Stop(StopCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "start",
    description = "Make this device discoverable.",
    example = "ffx bluetooth discoverable start"
)]
pub struct StartCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "stop",
    description = "Revoke this device's discoverability.",
    example = "ffx bluetooth discoverable stop"
)]
pub struct StopCommand {}
