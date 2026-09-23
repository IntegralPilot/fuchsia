// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "gce",
    description = "Start and manage Fuchsia instances directly on Google Compute Engine (GCE).",
    note = "The `gce` command is used to start up, manage, and shut down Fuchsia instances running natively as GCE guest virtual machines without QEMU or nested virtualization."
)]
pub struct GceCommand {
    #[argh(subcommand)]
    pub subcommand: GceSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum GceSubCommand {
    List(ListCommand),
    Show(ShowCommand),
    Serial(SerialCommand),
    Stop(StopCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, Default, PartialEq)]
#[argh(subcommand, name = "list", description = "List Fuchsia GCE virtual machine instances.")]
pub struct ListCommand {
    /// GCP Project ID. If unset, uses default from config.
    #[argh(option)]
    pub project: Option<String>,

    /// GCE zone. If unset, uses default from config.
    #[argh(option)]
    pub zone: Option<String>,
}

#[derive(ArgsInfo, FromArgs, Debug, Default, PartialEq)]
#[argh(
    subcommand,
    name = "show",
    description = "Show detailed information about a Fuchsia GCE virtual machine instance."
)]
pub struct ShowCommand {
    /// name of the instance.
    #[argh(positional)]
    pub name: String,

    /// GCP Project ID. If unset, uses default from config.
    #[argh(option)]
    pub project: Option<String>,

    /// GCE zone. If unset, uses default from config.
    #[argh(option)]
    pub zone: Option<String>,
}

#[derive(ArgsInfo, FromArgs, Debug, Default, PartialEq)]
#[argh(
    subcommand,
    name = "serial",
    description = "Read or stream serial port output from a Fuchsia GCE virtual machine."
)]
pub struct SerialCommand {
    /// name of the instance.
    #[argh(positional)]
    pub name: String,

    /// GCP Project ID. If unset, uses default from config.
    #[argh(option)]
    pub project: Option<String>,

    /// GCE zone. If unset, uses default from config.
    #[argh(option)]
    pub zone: Option<String>,

    /// port number. Defaults to 1 (UART COM1).
    #[argh(option, default = "1")]
    pub port: u32,

    /// follow/stream output continuously as it becomes available.
    #[argh(switch)]
    pub follow: bool,

    /// start byte offset for reading output.
    #[argh(option)]
    pub start: Option<i64>,
}

#[derive(ArgsInfo, FromArgs, Debug, Default, PartialEq)]
#[argh(
    subcommand,
    name = "stop",
    description = "Stop or delete a running Fuchsia GCE virtual machine instance."
)]
pub struct StopCommand {
    /// name of the instance.
    #[argh(positional)]
    pub name: String,

    /// GCP Project ID. If unset, uses default from config.
    #[argh(option)]
    pub project: Option<String>,

    /// GCE zone. If unset, uses default from config.
    #[argh(option)]
    pub zone: Option<String>,

    /// if true, only stop the instance without deleting it.
    #[argh(switch)]
    pub keep: bool,
}
