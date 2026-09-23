// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_rust::CapabilityDecl;
use component_debug::capability::{RouteSegment, get_all_route_segments};
use component_debug_fdomain as component_debug;
use errors::{ffx_bail, ffx_error};
use fdomain_client::fidl::{DiscoverableProtocolMarker, Proxy};
use fdomain_fuchsia_developer_remotecontrol::RemoteControlProxy;
use fdomain_fuchsia_memory_heapdump_client as fheapdump_client;
use fdomain_fuchsia_sys2::{OpenDirType, RealmQueryProxy};
use rcs;

const COLLECTOR_CAPABILITY: &str = fheapdump_client::CollectorMarker::PROTOCOL_NAME;

/// Retrieve the monikers of all the collectors in the system, i.e. all the components that declare
/// the COLLECTOR_CAPABILITY.
async fn list_collectors(query_proxy: &RealmQueryProxy) -> anyhow::Result<Vec<String>> {
    Ok(get_all_route_segments(COLLECTOR_CAPABILITY.to_string(), query_proxy)
        .await?
        .into_iter()
        .filter_map(|rs| match rs {
            RouteSegment::DeclareBy { moniker, capability: CapabilityDecl::Protocol(protocol) }
                if protocol.name == COLLECTOR_CAPABILITY =>
            {
                Some(moniker.to_string())
            }
            _ => None,
        })
        .collect())
}

/// Connects to the collector.
///
/// If a moniker is provided, this function directly connects to it.
///
/// If no moniker is provided, this function checks if there is exactly one collector component in
/// the system and connects to it.
pub async fn connect_to_collector(
    remote_control: &RemoteControlProxy,
    moniker: Option<String>,
) -> anyhow::Result<fheapdump_client::CollectorProxy> {
    let query_proxy =
        rcs::root_realm_query(remote_control, std::time::Duration::from_secs(15)).await?;

    let moniker = if let Some(moniker) = moniker {
        moniker
    } else {
        let candidates = list_collectors(&query_proxy).await?;
        if candidates.len() > 1 {
            ffx_bail!(
                "More than one collector was found, use --collector to disambiguate:\n{}",
                candidates.join("\n")
            );
        } else if let Some(candidate) = candidates.into_iter().next() {
            candidate
        } else {
            ffx_bail!("No collector found");
        }
    };

    // Try to connect via fuchsia.developer.remotecontrol/RemoteControl.ConnectCapability.
    let (proxy, server) =
        remote_control.domain().create_proxy::<fheapdump_client::CollectorMarker>();
    remote_control
        .connect_capability(
            &moniker,
            OpenDirType::ExposedDir,
            fheapdump_client::CollectorMarker::PROTOCOL_NAME,
            server.into_channel().into(),
        )
        .await?
        .map_err(|err| {
            ffx_error!("Attempting to connect to moniker {moniker} failed with {err:?}",)
        })?;
    Ok(proxy)
}
