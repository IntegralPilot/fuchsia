// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "drop-power-lease",
    description = "Drop the power lease reserved for the current session component.",
    note = "This command is useful for testing system suspension. If the session component has not \
    taken the lease, then the lease will be dropped. \
    If no other components on the system hold a power lease on the execution state, this will \
    suspend the system."
)]
pub struct SessionDropPowerLeaseCommand {
    /// if true, the tool will not return an error if the lease has already been dropped
    #[argh(switch)]
    pub allow_missing: bool,
}
