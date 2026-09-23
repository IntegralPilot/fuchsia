// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Default, Debug, Eq, PartialEq)]
#[argh(subcommand, name = "shell", description = "open a developer shell to a target device")]
pub struct ShellCommand {
    /// optional command and arguments to pass to the shell. Drops into
    /// interactive shell otherwise.
    #[argh(positional, greedy)]
    pub shell_command: Vec<String>,
}
