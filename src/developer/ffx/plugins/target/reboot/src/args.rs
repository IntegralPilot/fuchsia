// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use fdomain_fuchsia_hardware_power_statecontrol::ShutdownReason;
use ffx_core::ffx_command;

fn parse_shutdown_reason(value: &str) -> Result<ShutdownReason, String> {
    match value.to_ascii_lowercase().replace(['_', ' '], "-").as_str() {
        "system-update" => Ok(ShutdownReason::SystemUpdate),
        "developer-request" => Ok(ShutdownReason::DeveloperRequest),
        "user-request" => Ok(ShutdownReason::UserRequest),
        _ => Err(format!(
            "invalid reboot reason '{value}'. Supported reasons: 'system-update', 'developer-request', 'user-request'"
        )),
    }
}

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone, Copy)]
#[argh(
    subcommand,
    name = "reboot",
    description = "Reboots a target",
    note = "Reboot a target. Uses the 'fuchsia.hardware.power.statecontrol.Admin'
FIDL API to send the reboot command.

By default, target boots fully. This behavior can be overrided by passing
in either `--bootloader` or `--recovery` to boot into the bootloader or
recovery, respectively.

'fuchsia.hardware.power.statecontrol.Admin' is exposed by the 'power_manager'
component. To verify that the target exposes this service, `ffx component
select` or `ffx component knock` can be used.",
    error_code(1, "Timeout while powering off target.")
)]
pub struct RebootCommand {
    /// reboot to bootloader
    #[argh(switch, short = 'b')]
    pub bootloader: bool,

    /// reboot to recovery
    #[argh(switch, short = 'r')]
    pub recovery: bool,

    /// reboot reason. Defaults to 'developer-request' if unspecified. Ignored if the device is not
    /// in "product" mode (e.g. fastboot).
    #[argh(option, hidden_help, from_str_fn(parse_shutdown_reason))]
    pub reason: Option<ShutdownReason>,
}
