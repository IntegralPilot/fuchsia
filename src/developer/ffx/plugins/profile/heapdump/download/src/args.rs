// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "download", description = "Download stored snapshot")]
pub struct DownloadCommand {
    #[argh(option, description = "moniker of the collector to be queried (default: autodetect)")]
    pub collector: Option<String>,
    #[argh(option, description = "snapshot ID to be downloaded")]
    pub snapshot_id: u32,
    #[argh(switch, description = "write per-block metadata (as tags) in the protobuf file")]
    pub with_tags: bool,
    #[argh(option, description = "output compressed protobuf file (extension: .pb.gz)")]
    pub output_file: String,
    #[argh(switch, description = "symbolize the generated profile")]
    pub symbolize: bool,
}
