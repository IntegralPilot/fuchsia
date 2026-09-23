// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use powercli::args::PowerSubCommand;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "power", description = "Control system power features")]
pub struct PowerCommand {
    #[argh(subcommand)]
    pub subcommand: PowerSubCommand,
}

impl Into<powercli::args::PowerCommand> for PowerCommand {
    fn into(self) -> powercli::args::PowerCommand {
        powercli::args::PowerCommand { subcommand: self.subcommand }
    }
}
