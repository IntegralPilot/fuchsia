// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use ffx_wlan_sub_command::SubCommand;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "wlan", description = "Developer tool for manipulating WLAN state.")]
pub struct WlanCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}
