// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "shell",
    description = "Launch the scrutiny shell",
    example = "To run commands directly:

    $ ffx scrutiny shell \"tool.blobfs.extract --input <path> --output <path>\"
    ",
    note = "Runs a command in scrutiny. Deprecated for the more specific scrutiny methods."
)]
pub struct ScrutinyShellCommand {
    #[argh(positional)]
    pub command: String,
}
