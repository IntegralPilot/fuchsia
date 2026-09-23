// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use ffx_wm_sub_command::SubCommand;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "wm",
    description = "Control views presented by the window manager.",
    note = "The `wm` subcommand contains various commands that allow us to manipulate and inspect
    views being presented by the window manager component."
)]
pub struct WMCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}
