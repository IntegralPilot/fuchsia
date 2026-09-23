// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "stop", description = "Stops the repository server")]
pub struct StopCommand {
    /// stop all repository servers.
    #[argh(switch)]
    pub all: bool,

    /// stop servers serving the product bundle location.
    #[argh(option)]
    pub product_bundle: Option<Utf8PathBuf>,

    #[argh(option, short = 'p')]
    /// repository server port number.
    /// Required to disambiguate multiple repositories with the same name.
    pub port: Option<u16>,

    /// stop the repository server with the given name.
    #[argh(positional)]
    pub name: Option<String>,
}
