// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use ffx_core::ffx_command;

#[ffx_command]
#[derive(ArgsInfo, FromArgs, Debug, Eq, PartialEq)]
#[argh(
    subcommand,
    name = "hash",
    description = "Compute the merkle tree root hash of one or more delivery blobs or uncompressed blobs."
)]
pub struct HashCommand {
    #[argh(
        positional,
        description = "for each file, its root hash will be computed and displayed"
    )]
    pub paths: Vec<Utf8PathBuf>,

    #[argh(switch, short = 'u', description = "blobs are uncompressed instead of delivery blobs")]
    pub uncompressed: bool,
}
