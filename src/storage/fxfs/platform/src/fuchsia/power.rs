// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_power_battery::{
    BatteryInfoWatcherMarker, BatteryInfoWatcherRequest, BatteryManagerMarker, ChargeSource,
};
use fidl_fuchsia_power_system as _;
use fuchsia_component_client::connect_to_protocol;
use futures::stream::{BoxStream, StreamExt};
use fxfs::filesystem::{PowerManager, WakeLease};
use fxfs::log::*;
use std::sync::Arc;

pub struct FuchsiaPowerManager {}

impl FuchsiaPowerManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {})
    }
}

impl PowerManager for FuchsiaPowerManager {
    fn watch_battery(self: Arc<Self>) -> BoxStream<'static, (bool, WakeLease)> {
        let (watcher_client, watcher_stream) =
            fidl::endpoints::create_request_stream::<BatteryInfoWatcherMarker>();

        match connect_to_protocol::<BatteryManagerMarker>() {
            Ok(proxy) => {
                if let Err(error) = proxy.watch(watcher_client) {
                    error!(error:?; "Failed to register battery watcher");
                    return futures::stream::empty().boxed();
                }
            }
            Err(error) => {
                warn!(error:?; "Failed to connect to BatteryManager");
                return futures::stream::empty().boxed();
            }
        }

        futures::stream::unfold(watcher_stream, |mut stream| async move {
            match stream.next().await {
                Some(Ok(BatteryInfoWatcherRequest::OnChangeBatteryInfo {
                    info,
                    wake_lease,
                    responder,
                })) => {
                    let _ = responder.send();
                    // We check charge_source rather than charge_status because charge_status
                    // might be FULL even when the device has just been taken off charge.
                    // charge_source will indicate the actual source of power for the system.
                    let on_battery = !matches!(
                        info.charge_source,
                        Some(ChargeSource::AcAdapter)
                            | Some(ChargeSource::Usb)
                            | Some(ChargeSource::Wireless)
                    );
                    let handle =
                        wake_lease.map_or(zx::NullableHandle::invalid(), |l| l.into_handle());
                    Some(((on_battery, handle), stream))
                }
                Some(Err(error)) => {
                    error!(error:?; "Battery watcher stream error");
                    None
                }
                None => None,
            }
        })
        .boxed()
    }
}
