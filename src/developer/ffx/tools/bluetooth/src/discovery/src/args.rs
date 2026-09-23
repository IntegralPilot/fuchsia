// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "discovery",
    description = "Start or stop a general discovery procedure.",
    example = "ffx bluetooth discovery"
)]
pub struct DiscoveryCommand {
    #[argh(subcommand)]
    pub subcommand: DiscoverySubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(subcommand)]
pub enum DiscoverySubCommand {
    Start(StartCommand),
    Stop(StopCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "start",
    description = "Start a general discovery procedure.",
    example = "ffx bluetooth discovery start"
)]
pub struct StartCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "stop",
    description = "Stop an ongoing general discovery procedure.",
    example = "ffx bluetooth discovery stop"
)]
pub struct StopCommand {}
