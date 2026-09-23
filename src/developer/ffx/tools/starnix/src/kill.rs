// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use fho::{FfxContext, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use {
    fdomain_fuchsia_developer_remotecontrol as rc,
    fdomain_fuchsia_starnix_container as fstarcontainer,
};

use crate::common::*;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "kill",
    example = "ffx starnix kill <pid> <signal>",
    description = "Sends <signal> to the task with <pid>, inside the container"
)]
pub struct StarnixKillCommand {
    #[argh(option, short = 'p')]
    /// the linux pid to send the signal to
    pub pid: i32,

    #[argh(option, short = 's')]
    /// the signal to send
    pub signal: u64,
}

#[derive(Debug, JsonSchema, Deserialize, Serialize)]
pub struct KillCommandOutput {
    pub success: bool,
}

impl std::fmt::Display for KillCommandOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.success {
            writeln!(f, "Success")?;
        } else {
            writeln!(f, "Failure")?;
        }
        Ok(())
    }
}

pub async fn starnix_kill(
    StarnixKillCommand { pid, signal }: StarnixKillCommand,
    rcs_proxy: &rc::RemoteControlProxy,
) -> Result<KillCommandOutput> {
    let controller_proxy = connect_to_controller(&rcs_proxy, None).await?;
    match controller_proxy
        .send_signal(&fstarcontainer::ControllerSendSignalRequest {
            pid: Some(pid),
            signal: Some(signal),
            ..Default::default()
        })
        .await
        .bug_context("sending signal")
    {
        Ok(_) => Ok(KillCommandOutput { success: true }),
        Err(_) => Ok(KillCommandOutput { success: false }),
    }
}
