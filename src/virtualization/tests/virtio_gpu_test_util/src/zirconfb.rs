// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::framebuffer::{DetectResult, DisplayInfo, Framebuffer};
use anyhow::{Context, Error};
use fidl::endpoints;
use fidl_fuchsia_hardware_display::{
    CoordinatorListenerMarker, CoordinatorMarker, Info, ProviderSynchronousProxy,
    ServiceMarker as DisplayServiceMarker,
};
use fuchsia_component::client::Service;
use futures::executor::block_on;
use futures::{TryStreamExt, future};
use serde_json::json;

async fn connect_to_display_provider() -> Result<ProviderSynchronousProxy, Error> {
    Service::open(DisplayServiceMarker)
        .context("failed to open display Service")?
        .watch_for_any()
        .await?
        .connect_to_provider_sync()
        .context("failed to connect to FIDL provider")
}

fn convert_info(info: &Info) -> DisplayInfo {
    DisplayInfo {
        id: format!("[mfgr: '{}', model: '{}']", info.manufacturer_name, info.monitor_name),
        width: info.modes[0].active_area.width,
        height: info.modes[0].active_area.height,
    }
}

async fn read_info() -> Result<DetectResult, Error> {
    // Connect to the display coordinator.
    let provider = connect_to_display_provider().await?;

    let (_coordinator, listener_requests) = {
        let (dc_proxy, dc_server) = endpoints::create_sync_proxy::<CoordinatorMarker>();
        let (listener_client, listener_requests) =
            endpoints::create_request_stream::<CoordinatorListenerMarker>();
        let payload = fidl_fuchsia_hardware_display::ProviderOpenCoordinatorRequest {
            coordinator: Some(dc_server),
            coordinator_listener: Some(listener_client),
            priority: Some(fidl_fuchsia_hardware_display::ClientPriority {
                value: fidl_fuchsia_hardware_display::TEST_UTILITY_CLIENT_PRIORITY_VALUE,
            }),
            __source_breaking: fidl::marker::SourceBreaking,
        };
        provider
            .open_coordinator(payload, zx::MonotonicInstant::INFINITE)?
            .map_err(zx::Status::err_from_raw)?;
        (dc_proxy, listener_requests)
    };

    let mut stream = listener_requests.try_filter_map(|event| match event {
        fidl_fuchsia_hardware_display::CoordinatorListenerRequest::OnDisplaysChanged {
            added,
            removed: _,
            control_handle: _,
        } => future::ok(Some(added)),
        _ => future::ok(None),
    });
    let displays = &mut stream.try_next().await?.context("failed to get display streams")?;

    Ok(DetectResult {
        displays: displays.iter().map(convert_info).collect(),
        details: json!(format!("{:#?}", displays)),
        ..Default::default()
    })
}

fn read_info_from_display_coordinator() -> DetectResult {
    block_on(read_info()).unwrap_or_else(DetectResult::from_error)
}

pub struct ZirconFramebuffer;

impl Framebuffer for ZirconFramebuffer {
    fn detect_displays(&self) -> DetectResult {
        read_info_from_display_coordinator()
    }
}
