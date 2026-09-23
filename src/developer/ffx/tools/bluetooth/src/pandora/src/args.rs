// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

// ffx bluetooth pandora
#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "pandora",
    description = "Start/stop a Pandora gRPC test interface server and bluetooth-rootcanal virtual controller.",
    example = "ffx bluetooth pandora start --rootcanal-ip 172.16.243.142"
)]
pub struct PandoraCommand {
    /// start or stop
    #[argh(subcommand)]
    pub subcommand: PandoraSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum PandoraSubCommand {
    Start(StartCommand),
    Stop(StopCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "start",
    example = "ffx bluetooth pandora start --rootcanal-ip 172.16.243.142",
    description = "Start Pandora server and bluetooth-rootcanal."
)]
pub struct StartCommand {
    /// the Fuchsia port on which the Pandora server will listen. Default: 8042.
    #[argh(option, default = "8042")]
    pub grpc_port: u16,

    /// ip address of the host running the Rootcanal server.
    #[argh(option)]
    pub rootcanal_ip: String,

    /// port of Rootcanal server. Default: 6402.
    #[argh(option, default = "6402")]
    pub rootcanal_port: u16,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "stop", description = "Stop Pandora server and bt-rootcanal if running.")]
pub struct StopCommand {}
