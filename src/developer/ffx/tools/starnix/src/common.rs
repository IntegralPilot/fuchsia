// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, bail};
use component_debug::cli;
use component_debug_fdomain as component_debug;
use fdomain_fuchsia_developer_remotecontrol as rc;
use fdomain_fuchsia_starnix_container::{ControllerMarker, ControllerProxy};
use rcs;
use target_connector::Connector;
use target_holders::RemoteControlProxyHolder;

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const SESSION_CONTAINER: &str = "core/session-manager/session:session/container";

/// Returns the moniker for the container in the session, if there is one.
async fn find_session_container(rcs_proxy: &rc::RemoteControlProxy) -> Result<String> {
    let query_proxy = rcs::root_realm_query(&rcs_proxy, TIMEOUT)
        .await
        .map_err(anyhow::Error::from)
        .context("opening realm query")?;
    let instances = cli::list::get_instances_matching_filter(None, &query_proxy).await?;
    let container = instances.into_iter().find(|i| i.moniker.to_string() == SESSION_CONTAINER);

    if let Some(c) = container {
        Ok(c.moniker.to_string())
    } else {
        println!("Unable to find Starnix container in the session.");
        println!("Please specify a container with --moniker");
        bail!("cannot find container")
    }
}

async fn find_moniker(
    rcs_proxy: &rc::RemoteControlProxy,
    moniker: Option<String>,
) -> Result<String> {
    if let Some(moniker) = moniker {
        return Ok(moniker);
    }
    find_session_container(&rcs_proxy).await
}

pub async fn connect_to_rcs(
    rcs_connector: &Connector<RemoteControlProxyHolder>,
) -> Result<RemoteControlProxyHolder> {
    rcs_connector
        .try_connect(|target, _err| {
            eprintln!("Waiting for {target:?}...");
            Ok(())
        })
        .await
        .context("connecting to RCS")
}

pub async fn connect_to_controller(
    rcs_proxy: &rc::RemoteControlProxy,
    moniker: Option<String>,
) -> Result<ControllerProxy> {
    let moniker = find_moniker(&rcs_proxy, moniker).await?;
    rcs::connect_to_protocol::<ControllerMarker>(TIMEOUT, &moniker, &rcs_proxy)
        .await
        .map_err(anyhow::Error::from)
}
