// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "list", description = "List stored snapshots")]
pub struct ListCommand {
    #[argh(option, description = "moniker of the collector to be queried (default: autodetect)")]
    pub collector: Option<String>,
    #[argh(option, description = "select process by name")]
    pub by_name: Option<String>,
    #[argh(option, description = "select process by koid")]
    pub by_koid: Option<u64>,
}
