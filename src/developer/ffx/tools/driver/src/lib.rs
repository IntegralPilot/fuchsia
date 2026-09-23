// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::{Context, Result};
use component_debug::capability;
use component_debug_fdomain as component_debug;
use driver_connector_fdomain as driver_connector;
use fdomain_client::fidl::{DiscoverableProtocolMarker, ProtocolMarker};
use fdomain_fuchsia_driver_development as fdd;
use fdomain_fuchsia_driver_registrar as fdr;
use fdomain_fuchsia_sys2 as fsys;
use fdomain_fuchsia_test_manager as ftm;
use ffx_writer::{MachineWriter, ToolIO};
use fho::{FfxMain, FfxTool};
use fidl as _;
use rcs;
use target_holders::RemoteControlProxyHolder;

mod args;

struct DriverConnector {
    remote_control: fho::Result<RemoteControlProxyHolder>,
}

struct CapabilityOptions {
    capability_name: &'static str,
    default_capability_name_for_query: &'static str,
}

struct DiscoverableCapabilityOptions<P> {
    _phantom: std::marker::PhantomData<P>,
}

// #[derive(Default)] imposes a spurious P: Default bound.
impl<P> Default for DiscoverableCapabilityOptions<P> {
    fn default() -> Self {
        Self { _phantom: Default::default() }
    }
}

impl<P: DiscoverableProtocolMarker> Into<CapabilityOptions> for DiscoverableCapabilityOptions<P> {
    fn into(self) -> CapabilityOptions {
        CapabilityOptions {
            capability_name: P::PROTOCOL_NAME,
            default_capability_name_for_query: P::PROTOCOL_NAME,
        }
    }
}

// Gets monikers for components that expose a capability matching the given |query|.
// This moniker is eventually converted into a selector and is used to connecting to
// the capability.
async fn find_components_with_capability(
    query_proxy: &fsys::RealmQueryProxy,
    query: &str,
) -> Result<Vec<String>> {
    Ok(capability::get_all_route_segments(query.to_string(), &query_proxy)
        .await?
        .iter()
        .filter_map(|segment| {
            if let capability::RouteSegment::ExposeBy { moniker, .. } = segment {
                Some(moniker.to_string())
            } else {
                None
            }
        })
        .collect())
}

/// Find the components that expose a given capability, and let the user
/// request which component they would like to connect to.
async fn user_choose_selector(
    query_proxy: &fsys::RealmQueryProxy,
    capability: &str,
) -> Result<String> {
    let capabilities = find_components_with_capability(query_proxy, capability).await?;
    println!("Please choose which component to connect to:");
    for (i, component) in capabilities.iter().enumerate() {
        println!("    {}: {}", i, component)
    }

    let mut line_editor = rustyline::DefaultEditor::new()?;
    loop {
        let line = line_editor.readline("$ ")?;
        let choice = line.trim().parse::<usize>();
        if choice.is_err() {
            println!("Error: please choose a value.");
            continue;
        }
        let choice = choice.unwrap();
        if choice >= capabilities.len() {
            println!("Error: please choose a correct value.");
            continue;
        }
        // We have to escape colons in the capability name to distinguish them from the
        // syntactically meaningful colons in the ':expose:" string.
        return Ok(capabilities[choice].clone());
    }
}

impl DriverConnector {
    fn new(remote_control: fho::Result<RemoteControlProxyHolder>) -> Self {
        Self { remote_control }
    }

    async fn get_component_with_capability<S: ProtocolMarker>(
        &self,
        moniker: &str,
        capability_options: impl Into<CapabilityOptions>,
        select: bool,
    ) -> Result<S::Proxy> {
        let CapabilityOptions { capability_name, default_capability_name_for_query } =
            capability_options.into();

        let Ok(ref remote_control) = self.remote_control else {
            anyhow::bail!("{}", self.remote_control.as_ref().unwrap_err());
        };
        let (moniker, capability): (String, &str) = match select {
            true => {
                let query_proxy =
                    rcs::root_realm_query(remote_control, std::time::Duration::from_secs(15))
                        .await
                        .context("opening query")?;
                (user_choose_selector(&query_proxy, capability_name).await?, capability_name)
            }
            false => (moniker.to_string(), default_capability_name_for_query),
        };
        Ok(rcs::connect_with_timeout_at::<S>(
            std::time::Duration::from_secs(15),
            &moniker,
            &capability,
            &remote_control,
        )
        .await?)
    }
}

#[async_trait::async_trait]
impl driver_connector::DriverConnector for DriverConnector {
    async fn get_driver_development_proxy(&self, select: bool) -> Result<fdd::ManagerProxy> {
        self.get_component_with_capability::<fdd::ManagerMarker>(
            "/bootstrap/driver_manager",
            DiscoverableCapabilityOptions::<fdd::ManagerMarker>::default(),
            select,
        )
        .await
        .context("Failed to get driver development component")
    }

    async fn get_driver_registrar_proxy(&self, select: bool) -> Result<fdr::DriverRegistrarProxy> {
        self.get_component_with_capability::<fdr::DriverRegistrarMarker>(
            "/bootstrap/driver_index",
            DiscoverableCapabilityOptions::<fdr::DriverRegistrarMarker>::default(),
            select,
        )
        .await
        .context("Failed to get driver registrar component")
    }

    async fn get_suite_runner_proxy(&self) -> Result<ftm::SuiteRunnerProxy> {
        self.get_component_with_capability::<ftm::SuiteRunnerMarker>(
            "/core/test_manager",
            DiscoverableCapabilityOptions::<ftm::SuiteRunnerMarker>::default(),
            false,
        )
        .await
        .context("Failed to get SuiteRunner component")
    }
}

#[derive(FfxTool)]
pub struct DriverTool {
    remote_control: fho::Result<RemoteControlProxyHolder>,
    #[command]
    cmd: args::DriverCommand,
}

#[async_trait::async_trait(?Send)]
impl FfxMain for DriverTool {
    type Writer = MachineWriter<serde_json::Value>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let DriverTool { remote_control, cmd } = self;
        let tool_cmd: driver_tools_fdomain::args::DriverCommand = cmd.into();

        if writer.is_machine() && driver_tools_fdomain::is_machine_supported(&tool_cmd) {
            let connector = DriverConnector::new(remote_control);
            if let Some(value) = driver_tools_fdomain::driver_machine(tool_cmd, connector).await? {
                writer.machine(&value).map_err(|e| anyhow::anyhow!(e))?;
                return Ok(());
            }
            return Err(anyhow::anyhow!("Machine output supported but returned None").into());
        }

        driver_tools_fdomain::driver(tool_cmd, DriverConnector::new(remote_control), &mut writer)
            .await
            .map_err(Into::into)
    }
}
