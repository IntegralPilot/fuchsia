// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use async_trait::async_trait;
use component_debug_fdomain::config::RawConfigEntry;
use errors::ffx_error;
use fdomain_fuchsia_component_decl as fdecl;
use fdomain_fuchsia_developer_remotecontrol as rc;
use fdomain_fuchsia_session::{LaunchConfiguration, LauncherProxy};
use ffx_session_launch_args::SessionLaunchCommand;
use ffx_writer::{MachineWriter, ToolIO};
use fho::{FfxMain, FfxTool};
use moniker::Moniker;
use std::io::Write;
use target_holders::{RemoteControlProxyHolder, moniker};

const SESSION_MANAGER_MONIKER: &str = "/core/session-manager";

#[derive(FfxTool)]
pub struct LaunchTool {
    #[command]
    cmd: SessionLaunchCommand,
    rcs: RemoteControlProxyHolder,
    #[with(moniker(SESSION_MANAGER_MONIKER))]
    launcher_proxy: LauncherProxy,
}

fho::embedded_plugin!(LaunchTool);

#[async_trait(?Send)]
impl FfxMain for LaunchTool {
    type Writer = MachineWriter<()>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        launch_impl(self.launcher_proxy, self.rcs, self.cmd, &mut writer).await?;
        if writer.is_machine() {
            writer.machine(&())?;
        }
        Ok(())
    }
}

pub async fn launch_impl(
    launcher_proxy: LauncherProxy,
    rcs: RemoteControlProxyHolder,
    cmd: SessionLaunchCommand,
    writer: &mut MachineWriter<()>,
) -> Result<()> {
    if !writer.is_machine() {
        writeln!(writer, "Launching session: {}", cmd.url)?;
    }
    // A moniker is needed to resolve the component declaration because resolution is
    // context dependent on the location within the topology (e.g. to find resolver).
    // But the child name doesn't matter so we can use `placeholder`.
    let moniker = format!("{SESSION_MANAGER_MONIKER}/session:placeholder").parse().unwrap();
    let config_capabilities =
        resolve_config_capabilities(&cmd.url, &moniker, &rcs, cmd.config).await?;
    let config = LaunchConfiguration {
        session_url: Some(cmd.url),
        config_capabilities: Some(config_capabilities),
        ..Default::default()
    };
    launcher_proxy.launch(&config).await?.map_err(|err| format_err!("{:?}", err))
}

/// Parse raw config entries from the command line into appropriately typed
/// config values by looking up the component declaration from `url`.
async fn resolve_config_capabilities(
    url: &str,
    moniker: &Moniker,
    rcs: &rc::RemoteControlProxy,
    raw_capabilities: Vec<RawConfigEntry>,
) -> Result<Vec<fdecl::Configuration>> {
    if raw_capabilities.is_empty() {
        return Ok(vec![]);
    }
    let realm_query = rcs::root_realm_query(rcs, std::time::Duration::from_secs(15))
        .await
        .map_err(|err| ffx_error!("Could not open RealmQuery: {err}"))?;
    let resolved_capabilities = component_debug_fdomain::config::resolve_raw_config_capabilities(
        &realm_query,
        moniker,
        url,
        &raw_capabilities,
    )
    .await?;
    Ok(resolved_capabilities)
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_session::LauncherRequest;
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_launch_session() {
        const SESSION_URL: &str = "Session URL";

        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client.clone(), |req| match req {
            LauncherRequest::Launch { configuration, responder } => {
                assert!(configuration.session_url.is_some());
                let session_url = configuration.session_url.unwrap();
                assert!(session_url == SESSION_URL.to_string());
                let _ = responder.send(Ok(()));
            }
        });

        let (rcs_proxy, _) = client.create_proxy_and_stream::<rc::RemoteControlMarker>();
        let rcs = rcs_proxy.into();

        let launch_cmd = SessionLaunchCommand { url: SESSION_URL.to_string(), config: vec![] };
        let test_buffers = ffx_writer::TestBuffers::default();
        let mut writer = MachineWriter::new_test(None, &test_buffers);
        let result = launch_impl(proxy, rcs, launch_cmd, &mut writer).await;
        assert!(result.is_ok());
    }

    #[fuchsia::test]
    async fn test_machine_output_is_valid_json() {
        const SESSION_URL: &str = "Session URL";

        let client = fdomain_local::local_client_empty();
        let proxy = target_holders::fake_proxy(client.clone(), |req| match req {
            LauncherRequest::Launch { configuration: _, responder } => {
                let _ = responder.send(Ok(()));
            }
        });

        let (rcs_proxy, _) = client.create_proxy_and_stream::<rc::RemoteControlMarker>();
        let rcs = rcs_proxy.into();

        let launch_cmd = SessionLaunchCommand { url: SESSION_URL.to_string(), config: vec![] };
        let test_buffers = ffx_writer::TestBuffers::default();
        let writer = MachineWriter::new_test(Some(ffx_writer::Format::Json), &test_buffers);

        let tool = LaunchTool { cmd: launch_cmd, rcs, launcher_proxy: proxy };

        let result = tool.main(writer).await;
        assert!(result.is_ok());

        let output = test_buffers.into_stdout_str();
        assert_eq!(output, "null\n");
    }
}
