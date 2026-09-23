// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "usb",
    description = "Collect and view telemetry for USB controllers and devices"
)]
pub struct UsbCommand {
    #[argh(subcommand)]
    pub subcommand: UsbSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum UsbSubCommand {
    Diagnostics(DiagnosticsCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "diagnostics",
    description = "Inspect USB health and diagnostics on the target device"
)]
pub struct DiagnosticsCommand {
    /// query and print the fuchsia.usb.policy.Health report (default mode if no flags are specified)
    #[argh(switch, short = 'H')]
    pub health: bool,

    /// query and print device-side USB Inspect diagnostics
    #[argh(switch, short = 'i')]
    pub inspect: bool,

    /// query and print all available USB diagnostics (both health and inspect)
    #[argh(switch, short = 'a')]
    pub all: bool,

    /// print verbose health details
    #[argh(switch, short = 'v')]
    pub verbose: bool,
}
