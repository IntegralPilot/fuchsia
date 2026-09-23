// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use std::path::PathBuf;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, Eq, PartialEq)]
#[argh(
    subcommand,
    name = "create",
    description = "Create a FAR file from a directory. WARNING: this will overwrite <output_file> if it exists."
)]
pub struct CreateCommand {
    #[argh(positional, description = "directory containing files to be archived")]
    pub input_directory: PathBuf,

    #[argh(
        positional,
        description = "file name of the resulting FAR file -- should probably end in \".far\""
    )]
    pub output_file: PathBuf,
}
