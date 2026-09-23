// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(Clone, ArgsInfo, FromArgs, Default, Debug, PartialEq)]
#[argh(subcommand, name = "show")]
/// Show Fuchsia emulator details.
pub struct ShowCommand {
    /// show all of the available details, which is the default.
    #[argh(switch)]
    pub all: bool,

    /// show the command line used to launch the emulator.
    #[argh(switch)]
    pub cmd: bool,

    /// show the configuration in a format consistent with the 'start' command's
    /// --config flag.
    #[argh(switch)]
    pub config: bool,

    /// show the virtual device information used to launch this emulator,
    /// in a format consistent with the 'start' command's --device flag.
    #[argh(switch)]
    pub device: bool,

    /// show the instance information.
    #[argh(switch)]
    pub instance: bool,

    /// name of the emulator instance to show details for.
    /// See a list of available instances by running `ffx emu list`.
    /// If only one instance is running, this defaults to that instance name.
    #[argh(positional)]
    pub name: Option<String>,

    /// switch to show network details.
    #[argh(switch)]
    pub net: bool,
}
