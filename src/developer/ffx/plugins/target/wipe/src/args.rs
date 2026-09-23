// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone, Copy)]
#[argh(
    subcommand,
    name = "wipe",
    description = "Performs a factory data reset",
    note = "Performs a factory data reset. Uses the 'fuchsia.recovery.FactoryReset' FIDL API to send the reset command."
)]
pub struct WipeCommand {
    /// reset without prompting for confirmation
    #[argh(switch)]
    pub force: bool,
}
