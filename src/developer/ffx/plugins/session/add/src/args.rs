// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "add",
    description = "Add an element to the current session.

If the --persist option is used, the package should be in the base or cache
package set as otherwise it might fail to launch after a reboot.",
    example = "To add the `bouncing_ball.cm` component as an element:

    $ ffx session add fuchsia-pkg://fuchsia.com/bouncing_ball#meta/bouncing_ball.cm"
)]
pub struct SessionAddCommand {
    /// component URL for the element to add
    #[argh(positional)]
    pub url: String,

    /// pass to keep element alive until command exits
    #[argh(switch)]
    pub interactive: bool,

    /// pass to have the element persist over reboots
    #[argh(switch)]
    pub persist: bool,

    /// name for the element which defaults to random if not specified
    #[argh(option)]
    pub name: Option<String>,
}
