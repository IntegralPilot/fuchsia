// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use std::path::PathBuf;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, Eq, PartialEq)]
#[argh(subcommand, name = "list", description = "List the entry paths contained in a FAR file")]
pub struct ListCommand {
    #[argh(positional)]
    pub far_file: PathBuf,

    #[argh(
        switch,
        short = 'l',
        description = "show detailed information for each entry (does nothing if --machine json is specified, which shows everything)"
    )]
    pub long_format: bool,
}
