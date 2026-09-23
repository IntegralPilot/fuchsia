// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use component_debug_fdomain::config::RawConfigEntry;
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "launch",
    description = "Launch a session component.",
    example = "To launch the `hello-world-session.cm` component as a session:

    $ ffx session launch fuchsia-pkg://fuchsia.com/hello-world-session#meta/hello-world-session.cm
"
)]
pub struct SessionLaunchCommand {
    #[argh(positional)]
    /// the component URL of a session.
    pub url: String,

    #[argh(option)]
    /// provide additional configuration capabilities to the component being run. Specified
    /// in the format `fully.qualified.Name=VALUE` where `fully.qualified.Name` is the name
    /// of the configuration capability, and `VALUE` is a JSON string which can be resolved
    /// as the correct type of configuration value.
    pub config: Vec<RawConfigEntry>,
}
