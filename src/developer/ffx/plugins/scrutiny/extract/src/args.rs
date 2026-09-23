// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use ffx_scrutiny_extract_sub_command::SubCommand;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "extract", description = "Extracts common Fuchsia file types")]
pub struct ScrutinyExtractCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}
