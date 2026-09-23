// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "list-composites",
    description = "List composites",
    example = "To list all composites:

    $ driver list-composites -v",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct ListCompositesCommand {
    /// list all the properties of a composite.
    #[argh(switch, short = 'v', long = "verbose")]
    pub verbose: bool,

    /// if this exists, the user will be prompted for a component to select.
    #[argh(switch, short = 's', long = "select")]
    pub select: bool,
}
