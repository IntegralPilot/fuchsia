// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(Clone, ArgsInfo, FromArgs, Default, Debug, PartialEq)]
#[argh(subcommand, name = "list")]
/// List running Fuchsia emulators.
pub struct ListCommand {
    /// list only the emulators that are currently in the "running" state.
    #[argh(switch, short = 'r')]
    pub only_running: bool,
}
