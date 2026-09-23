// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use ffx_package_far_sub_command::SubCommand;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "far", description = "Work with Fuchsia Archive Format (FAR) files")]
pub struct FarCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}
