// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "monitor",
    description = "Start a local server to monitor status.",
    example = "ffx monitor start"
)]
pub struct MonitorCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum SubCommand {
    Start(StartCommand),
    Stop(StopCommand),
    Status(StatusCommand),
    IntentionalDisconnect(IntentionalDisconnectCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "start",
    description = "Start the monitor server",
    example = "To start the monitor with a specific device:\n  ffx monitor start --nodename fuchsia-1234-5678-90ab\n\nTo start the monitor on a specific port:\n  ffx monitor start --port 8080"
)]
pub struct StartCommand {
    #[argh(option)]
    /// specify the device nodename ffx going to monitor
    pub nodename: Option<String>,

    #[argh(option)]
    /// specify the port ffx monitor going to listen on
    pub port: Option<u16>,

    #[argh(switch, description = "do not connect to targets (local discovery only)")]
    pub no_probe: bool,

    #[argh(switch, description = "do not do mDNS discovery (local discovery only)")]
    pub no_mdns: bool,

    #[argh(switch, description = "do not do USB discovery (local discovery only)")]
    pub no_usb: bool,

    #[argh(
        option,
        description = "file to store the monitor log file (relative to log.dir or absolute)"
    )]
    pub log_file: Option<String>,

    #[argh(
        option,
        description = "file to store the monitor log aggregations file (relative to log.dir or absolute)"
    )]
    pub aggregations_file: Option<String>,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "stop", description = "Stop the monitor server")]
pub struct StopCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "status", description = "Check the statuses")]
pub struct StatusCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "intentional-disconnect",
    description = "Notify monitor of an intentional disconnect"
)]
pub struct IntentionalDisconnectCommand {
    #[argh(option)]
    /// specify the device nodename
    pub nodename: String,
}
