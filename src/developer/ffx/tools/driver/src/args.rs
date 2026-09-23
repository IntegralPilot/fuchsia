// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use driver_tools_fdomain::args::DriverSubCommand;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "driver", description = "Support driver development workflows")]
pub struct DriverCommand {
    /// if this exists, the user will be prompted for a component to select.
    #[argh(switch, short = 's', long = "select")]
    pub select: bool,

    #[argh(subcommand)]
    pub subcommand: DriverSubCommand,
}

impl Into<driver_tools_fdomain::args::DriverCommand> for DriverCommand {
    fn into(self) -> driver_tools_fdomain::args::DriverCommand {
        driver_tools_fdomain::args::DriverCommand {
            select: self.select,
            subcommand: self.subcommand,
        }
    }
}
