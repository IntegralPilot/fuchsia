// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use ffx_config::EnvironmentContext;
use ffx_diagnostics::{NotificationType, Notifier};
use ffx_diagnostics_checks::run_diagnostics;
use ffx_target_status_args as args;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxContext, FfxMain, FfxTool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::time::Duration;

#[derive(Serialize, Deserialize, JsonSchema, Debug, PartialEq, Eq)]
pub struct StatusUpdate {
    status: NotificationType,
    message: String,
}

#[derive(FfxTool)]
pub struct Status {
    #[command]
    cmd: args::TargetStatus,
    ctx: EnvironmentContext,
}

fho::embedded_plugin!(Status);

struct DefaultNotifier {
    writer: <Status as FfxMain>::Writer,
}

impl Notifier for DefaultNotifier {
    fn update_status(
        &mut self,
        status: NotificationType,
        message: impl Into<String>,
    ) -> anyhow::Result<()> {
        let update = StatusUpdate { message: message.into(), status };
        self.writer.machine_or_else(&update, || {
            let prefix = match update.status {
                NotificationType::Info => "[i] ",
                NotificationType::Success => "\t[✓] ",
                NotificationType::Warning => "\t[!] ",
                NotificationType::Error => "\t[✗] ",
            };
            format!("{}{}", prefix, update.message)
        })?;
        self.writer.flush().map_err(Into::into)
    }
}

#[async_trait(?Send)]
impl FfxMain for Status {
    // This is a machine notifier, but there is (currently) no machine output in use yet.
    type Writer = VerifiedMachineWriter<StatusUpdate>;
    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        let mut notifier = DefaultNotifier { writer };
        run_diagnostics(
            &self.ctx,
            &mut notifier,
            Duration::from_secs_f64(self.cmd.proxy_connect_timeout),
        )
        .await
        .user_message("\nDiagnostics failed")
    }
}
