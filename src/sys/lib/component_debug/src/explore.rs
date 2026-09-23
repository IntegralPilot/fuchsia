// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use futures::prelude::*;
use moniker::Moniker;
use std::str::FromStr;

use flex_client::Socket;
use flex_fuchsia_dash as fdash;

#[derive(Debug, PartialEq)]
pub enum DashNamespaceLayout {
    NestAllInstanceDirs,
    InstanceNamespaceIsRoot,
}

impl FromStr for DashNamespaceLayout {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "namespace" => Ok(Self::InstanceNamespaceIsRoot),
            "nested" => Ok(Self::NestAllInstanceDirs),
            _ => Err(format_err!("unknown layout (expected 'namespace' or 'nested')")),
        }
    }
}

impl Into<fdash::DashNamespaceLayout> for DashNamespaceLayout {
    fn into(self) -> fdash::DashNamespaceLayout {
        match self {
            Self::NestAllInstanceDirs => fdash::DashNamespaceLayout::NestAllInstanceDirs,
            Self::InstanceNamespaceIsRoot => fdash::DashNamespaceLayout::InstanceNamespaceIsRoot,
        }
    }
}

pub async fn explore_over_socket(
    moniker: Moniker,
    pty_server: Socket,
    tools_urls: Vec<String>,
    command: Option<String>,
    ns_layout: DashNamespaceLayout,
    launcher_proxy: &fdash::LauncherProxy,
) -> Result<()> {
    launcher_proxy
        .explore_component_over_socket(
            moniker.as_ref(),
            pty_server,
            &tools_urls,
            command.as_deref(),
            ns_layout.into(),
        )
        .await
        .map_err(|e| format_err!("fidl error launching dash: {}", e))?
        .map_err(|e| match e {
            fdash::LauncherError::InstanceNotFound => {
                format_err!("No instance was found matching the moniker '{}'.", moniker)
            }
            fdash::LauncherError::InstanceNotResolved => format_err!(
                "{} is not resolved. Resolve the instance and retry this command",
                moniker
            ),
            e => format_err!("Unexpected error launching dash: {:?}", e),
        })?;
    Ok(())
}

pub async fn wait_for_shell_exit(launcher_proxy: &fdash::LauncherProxy) -> Result<i32> {
    // Report process errors and return the exit status.
    let mut event_stream = launcher_proxy.take_event_stream();
    match event_stream.next().await {
        Some(Ok(fdash::LauncherEvent::OnTerminated { return_code })) => Ok(return_code),
        Some(Err(e)) => Err(format_err!("OnTerminated event error: {:?}", e)),
        None => Err(format_err!("didn't receive an expected OnTerminated event")),
    }
}
