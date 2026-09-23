// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_component_sub_command::SubCommand;
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "component", description = "Discover and manage components", short = 'c')]
pub struct ComponentCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}
