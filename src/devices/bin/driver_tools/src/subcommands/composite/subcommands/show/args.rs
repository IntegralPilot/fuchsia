// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "show",
    description = "Show detailed information for a composite node spec",
    example = "To show a composite node spec:

    $ driver composite show my_composite_spec
    ",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct ShowCompositeCommand {
    #[argh(positional)]
    /// name of the composite node spec. Partial matches allowed.
    pub query: String,
}
