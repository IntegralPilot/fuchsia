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
    name = "extract",
    description = "Extract <paths> from <far_file> to <output-dir>. WARNING: this may overwrite existing files."
)]
pub struct ExtractCommand {
    #[argh(
        switch,
        short = 'v',
        description = "verbose output: print file names that were extracted"
    )]
    pub verbose: bool,

    #[argh(
        option,
        short = 'o',
        long = "output-dir",
        description = "output directory (defaults to current directory, creates the directory if it doesn't exist)",
        default = "Utf8PathBuf::from(\".\")"
    )]
    pub output_dir: Utf8PathBuf,

    #[argh(positional)]
    pub far_file: PathBuf,

    #[argh(
        positional,
        description = "path(s) within the FAR file. If no paths are specified, extract everything."
    )]
    pub paths: Vec<Utf8PathBuf>,
}
