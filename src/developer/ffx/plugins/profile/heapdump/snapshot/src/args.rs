// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "snapshot", description = "Snapshot current heap memory usage")]
pub struct SnapshotCommand {
    #[argh(option, description = "moniker of the collector to be queried (default: autodetect)")]
    pub collector: Option<String>,
    #[argh(option, description = "select process by name")]
    pub by_name: Option<String>,
    #[argh(option, description = "select process by koid")]
    pub by_koid: Option<u64>,
    #[argh(option, description = "output compressed protobuf file (extension: .pb.gz)")]
    pub output_file: String,
    #[argh(switch, description = "write per-block metadata (as tags) in the protobuf file")]
    pub with_tags: bool,
    #[argh(option, description = "optional directory to dump each blocks' contents into")]
    pub output_contents_dir: Option<String>,
    #[argh(switch, description = "symbolize the generated profile")]
    pub symbolize: bool,
    #[argh(switch, description = "allow selecting more than one process")]
    pub multi_process: bool,
}
