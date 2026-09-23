// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::collaborative_reboot::args::CollaborativeRebootCommand;
use super::debugcmd::args::DebugCommand;
use super::suspend::args::SuspendCommand;
use argh::{ArgsInfo, FromArgs, TopLevelCommand};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "power", description = "Control system power features")]
pub struct PowerCommand {
    #[argh(subcommand)]
    pub subcommand: PowerSubCommand,
}

impl TopLevelCommand for PowerCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum PowerSubCommand {
    CollaborativeReboot(CollaborativeRebootCommand),
    Debugcmd(DebugCommand),
    Suspend(SuspendCommand),
}
