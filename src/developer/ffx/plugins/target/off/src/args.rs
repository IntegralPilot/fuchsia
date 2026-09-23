// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "off",
    description = "Powers off a target",
    note = "Power off a target. Uses the 'fuchsia.hardware.power.statecontrol.Admin'
FIDL API to send the power off command.

'fuchsia.hardware.power.statecontrol.Admin' is exposed by the 'power_manager'
component. To verify that the target exposes this service, `ffx component
select` or `ffx component knock` can be used.",
    error_code(1, "Timeout while powering off target.")
)]
pub struct OffCommand {}
