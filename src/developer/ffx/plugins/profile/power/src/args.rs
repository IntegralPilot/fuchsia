// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use ffx_profile_power_sub_command::SubCommand;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "power", description = "Access power-related information")]
/// Top-level command for "ffx profile power".
pub struct PowerCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}
