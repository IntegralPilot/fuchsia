// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "focus",
    description = "Focus the position and viewId of all windows currently presented in the session."
)]
pub struct WMFocusCommand {
    /// position of the window that should be brought into focus
    #[argh(positional, arg_name = "position")]
    pub position: u64,
}
