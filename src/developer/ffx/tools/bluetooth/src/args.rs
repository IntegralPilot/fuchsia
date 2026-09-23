// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_bluetooth_sub_command::SubCommand;
use ffx_core::ffx_command;

// Top-level command: ffx bluetooth
#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "bluetooth",
    description = "Issue a command to Sapphire, the Bluetooth system."
)]
pub struct BluetoothCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}
