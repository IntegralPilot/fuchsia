// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use std::path::PathBuf;

#[ffx_command]
#[derive(ArgsInfo, FromArgs, Debug, Eq, PartialEq)]
#[argh(
    subcommand,
    name = "file-hash",
    description = "Compute the merkle tree root hash of one or more files."
)]
pub struct FileHashCommand {
    #[argh(
        positional,
        description = "for each file, its root hash will be computed and displayed"
    )]
    pub paths: Vec<PathBuf>,
}
