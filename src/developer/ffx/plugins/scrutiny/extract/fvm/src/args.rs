// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use std::path::PathBuf;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "fvm",
    description = "Extracts a FVM file",
    example = "To extract a FVM file:

        $ffx scrutiny extract fvm fvm.blk /tmp/fvm",
    note = "Extracts a FVM to a specific directory."
)]
pub struct ScrutinyFvmCommand {
    #[argh(positional)]
    pub input: PathBuf,
    #[argh(positional)]
    pub output: PathBuf,
}
