// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use ffx_core::ffx_command;

#[ffx_command]
#[derive(ArgsInfo, FromArgs, Debug, Eq, PartialEq)]
#[argh(subcommand, name = "decompress", description = "Decompress one or more blobs")]
pub struct DecompressCommand {
    #[argh(option, short = 'o', description = "output decompressed blobs into this directory")]
    pub output: Utf8PathBuf,

    #[argh(positional, description = "decompress these files")]
    pub paths: Vec<Utf8PathBuf>,
}
