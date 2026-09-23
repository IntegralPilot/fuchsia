// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use anyhow::Result;
use args::{SuspendCommand, SuspendSubcommand};
use flex_fuchsia_power_topology_test as fpt;
use std::io::Write;

pub async fn suspend(
    cmd: SuspendCommand,
    _writer: &mut dyn Write,
    system_activity_control: fpt::SystemActivityControlProxy,
) -> Result<()> {
    match cmd.subcommand {
        SuspendSubcommand::Prevent(command) => {
            if command.restart {
                let _ = system_activity_control
                    .restart_application_activity(command.wait_time.as_nanos() as u64)
                    .await?;
            } else {
                let _ = system_activity_control.start_application_activity().await?;
            }
        }
        SuspendSubcommand::Allow(_) => {
            let _ = system_activity_control.stop_application_activity().await?;
        }
    }
    Ok(())
}
