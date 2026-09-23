// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use ffx_package_blob_sub_command::SubCommand;

#[ffx_command]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "blob", description = "Work with Fuchsia blob files.")]
pub struct BlobCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}
