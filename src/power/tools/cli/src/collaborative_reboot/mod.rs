// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use anyhow::{Result, anyhow};
use std::io::Write;

use args::{CollaborativeRebootCommand, PerformPendingRebootCommand, SubCommand};
use flex_fuchsia_power as fpower;

pub async fn collaborative_reboot(
    writer: &mut dyn Write,
    CollaborativeRebootCommand { subcommand }: CollaborativeRebootCommand,
    power_proxy: fpower::CollaborativeRebootInitiatorProxy,
) -> Result<()> {
    match subcommand {
        SubCommand::PerformPendingReboot(PerformPendingRebootCommand {}) => {
            let fpower::CollaborativeRebootInitiatorPerformPendingRebootResponse {
                rebooting, ..
            } = power_proxy
                .perform_pending_reboot()
                .await
                .map_err(|e| anyhow!("Failed to call PerformPendingReboot: {e}"))?;
            writeln!(writer, "rebooting = {rebooting:?}")
                .map_err(|e| anyhow!("Failed to write output: {e}"))?;
            Ok(())
        }
    }
}

#[cfg(feature = "fdomain")]
#[cfg(test)]
mod tests {
    use ffx_writer::TestBuffers;
    use target_holders::fake_proxy;

    use super::*;

    #[fuchsia::test]
    async fn test_perform_pending_reboot() {
        let client = fdomain_local::local_client_empty();
        let command = CollaborativeRebootCommand {
            subcommand: SubCommand::PerformPendingReboot(args::PerformPendingRebootCommand {}),
        };

        let power_proxy = fake_proxy(std::sync::Arc::clone(&client), move |req| match req {
            fpower::CollaborativeRebootInitiatorRequest::PerformPendingReboot {
                responder, ..
            } => responder
                .send(&fpower::CollaborativeRebootInitiatorPerformPendingRebootResponse {
                    rebooting: Some(true),
                    ..Default::default()
                })
                .expect("failed to respond"),
        });

        let bufs = TestBuffers::default();
        let writer = SimpleWriter::new_test(&bufs);

        collaborative_reboot(writer, command, power_proxy).await.unwrap();

        assert_eq!(bufs.into_stdout_str(), "rebooting = Some(true)\n");
    }
}
