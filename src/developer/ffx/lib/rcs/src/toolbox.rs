// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fdomain_client::fidl::{DiscoverableProtocolMarker, Proxy};
use fdomain_fuchsia_developer_remotecontrol::RemoteControlProxy;
use fdomain_fuchsia_io as fio;
use fdomain_fuchsia_sys2::OpenDirType;
use std::time::{Duration, Instant};

pub const MONIKER: &str = "toolbox";
const LEGACY_MONIKER: &str = "core/toolbox";

/// Open the service directory of the toolbox.
pub async fn open_toolbox(rcs: &RemoteControlProxy) -> Result<fio::DirectoryProxy> {
    rcs.domain().namespace().await.map_err(Into::into).map(fio::DirectoryProxy::from_channel)
}

/// Connects to a protocol available in the namespace of the `toolbox` component.
/// If we fail to connect to the protocol in the namespace of the `toolbox` component, then we'll
/// attempt to connect to the protocol in the exposed directory of the component located at the
/// given `backup_moniker`.
pub async fn connect_with_timeout<P>(
    rcs_proxy: &RemoteControlProxy,
    dur: Duration,
) -> Result<P::Proxy>
where
    P: DiscoverableProtocolMarker,
{
    let protocol_name = P::PROTOCOL_NAME;
    let start_time = Instant::now();
    let toolbox_res = crate::open_with_timeout_at::<P>(
        dur,
        MONIKER,
        OpenDirType::NamespaceDir,
        &format!("svc/{protocol_name}"),
        rcs_proxy,
    )
    .await;

    // Fallback to legacy toolbox moniker if toolbox is not available.
    let toolbox_res = match toolbox_res {
        Ok(toolbox) => Ok(toolbox),
        Err(_) => {
            let toolbox_took = Instant::now() - start_time;
            let timeout = dur.saturating_sub(toolbox_took);
            crate::open_with_timeout_at::<P>(
                timeout,
                LEGACY_MONIKER,
                OpenDirType::NamespaceDir,
                &format!("svc/{protocol_name}"),
                rcs_proxy,
            )
            .await
        }
    };

    toolbox_res.context(toolbox_error_message(protocol_name))
}

fn toolbox_error_message(protocol_name: &str) -> String {
    format!(
        "\
        Attempted to find protocol marker {protocol_name} at \
        '/toolbox', but it wasn't available. \n\n\
        Make sure the target is connected and otherwise functioning, \
        and that it is configured to provide capabilities over the \
        network to host tools.\n\n\
        If the protocol is provided by a component that is not in the \
        base image, you may need to have a package server running and \
        available to your target.
    "
    )
}
