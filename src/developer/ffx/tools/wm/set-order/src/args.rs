// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "set-order",
    description = "Re-order the window currently at `old_position` to be placed at `new_position`."
)]
pub struct WMSetOrderCommand {
    /// original position of the window that should be moved
    #[argh(positional, arg_name = "old-position")]
    pub old_position: u64,

    /// new position the window should be moved to
    #[argh(positional, arg_name = "new-position")]
    pub new_position: u64,
}
