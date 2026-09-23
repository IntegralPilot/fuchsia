// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "deregister",
    description = "Make the target forget a specific repository"
)]
pub struct DeregisterCommand {
    #[argh(option, short = 'r')]
    /// remove the repository named `name` from the target, rather than the default.
    pub repository: Option<String>,

    #[argh(option, short = 'p')]
    /// repository server port number.
    /// Required to disambiguate multiple repositories with the same name.
    pub port: Option<u16>,
}
