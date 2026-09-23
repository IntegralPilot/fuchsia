// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "show",
    description = "Show driver details",
    example = "To show details for a driver:

    $ driver show fuchsia-boot:///dovi#meta/dovi.cm",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct ShowCommand {
    #[argh(positional)]
    /// driver URL or name. Partial matches allowed.
    pub query: String,

    /// if this exists, the user will be prompted for a component to select.
    #[argh(switch, short = 's', long = "select")]
    pub select: bool,
}
