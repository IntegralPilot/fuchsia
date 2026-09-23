// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use ffx_core::ffx_command;
use std::path::PathBuf;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, Eq, PartialEq)]
#[argh(
    subcommand,
    name = "cat",
    description = "Dump the contents of the <far_file> entry associated with <path> to stdout"
)]
pub struct CatCommand {
    #[argh(positional)]
    pub far_file: PathBuf,

    #[argh(positional, description = "a path within the FAR file")]
    pub path: Utf8PathBuf,
}
