// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdomain_fuchsia_hardware_usb_policy as fpolicy;
use fho::subtool_suite::{FfxSubtoolSuite, Subtool, SubtoolBox, SubtoolSuite, ToolSuiteCommand};
use fho::{FfxTool, FhoEnvironment, Result};

mod args;
mod diagnostics;

pub use args::{DiagnosticsCommand, UsbCommand, UsbSubCommand};
pub use diagnostics::DiagnosticsTool;

impl ToolSuiteCommand for UsbCommand {
    type SubCommand = UsbSubCommand;
    fn into_subcommand(self) -> Self::SubCommand {
        self.subcommand
    }
}

pub struct UsbSuite;

#[async_trait::async_trait(?Send)]
impl SubtoolSuite for UsbSuite {
    type Command = UsbCommand;

    async fn new_subtool(
        env: FhoEnvironment,
        subcommand: UsbSubCommand,
    ) -> Result<Box<dyn SubtoolBox>> {
        Ok(match subcommand {
            UsbSubCommand::Diagnostics(cmd) => {
                Subtool::new(diagnostics::DiagnosticsTool::from_env(env, cmd).await?)
            }
        })
    }
}

pub type UsbSuiteTool = FfxSubtoolSuite<UsbSuite>;

pub fn device_state_to_str(state: Option<fpolicy::DeviceState>) -> &'static str {
    match state {
        Some(fpolicy::DeviceState::NotAttached) => "Not Attached",
        Some(fpolicy::DeviceState::Attached) => "Attached",
        Some(fpolicy::DeviceState::Powered) => "Powered",
        Some(fpolicy::DeviceState::Default) => "Default",
        Some(fpolicy::DeviceState::Address) => "Address",
        Some(fpolicy::DeviceState::Configured) => "Configured",
        Some(fpolicy::DeviceState::Suspended) => "Suspended",
        Some(_) | None => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_state_to_str() {
        assert_eq!(device_state_to_str(Some(fpolicy::DeviceState::NotAttached)), "Not Attached");
        assert_eq!(device_state_to_str(Some(fpolicy::DeviceState::Attached)), "Attached");
        assert_eq!(device_state_to_str(Some(fpolicy::DeviceState::Powered)), "Powered");
        assert_eq!(device_state_to_str(Some(fpolicy::DeviceState::Default)), "Default");
        assert_eq!(device_state_to_str(Some(fpolicy::DeviceState::Address)), "Address");
        assert_eq!(device_state_to_str(Some(fpolicy::DeviceState::Configured)), "Configured");
        assert_eq!(device_state_to_str(Some(fpolicy::DeviceState::Suspended)), "Suspended");
        assert_eq!(device_state_to_str(None), "Unknown");
    }
}
