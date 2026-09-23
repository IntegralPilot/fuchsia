// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use ffx_target_package_sub_command::SubCommand;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "package",
    description = "Interact with target package management",
    note = "The `package` subcommand contains various commands for package management on-target.

    Most of the commands depend on the RCS (Remote Control Service) on the target."
)]
pub struct TargetCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}
