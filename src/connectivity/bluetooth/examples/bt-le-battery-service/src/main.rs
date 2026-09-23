// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Context as _, Error};
use battery_client::BatteryClient;
use bt_le_battery_service_config::Config;
use fidl::endpoints::{create_request_stream, RequestStream, Responder};
use fidl_fuchsia_bluetooth_gatt2 as gatt;
use fuchsia_bluetooth::types::{PeerId, Uuid};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_sync::Mutex;
use futures::stream::{StreamExt, TryStreamExt};
use futures::try_join;
use log::{info, warn};
use std::collections::HashSet;
use std::str::FromStr;

/// Arbitrary Handle assigned to the Battery Service.
const BATTERY_SERVICE_HANDLE: gatt::ServiceHandle = gatt::ServiceHandle { value: 10 };
/// Fixed Handle assigned to the Battery Level characteristic.
const BATTERY_LEVEL_CHARACTERISTIC_HANDLE: gatt::Handle = gatt::Handle { value: 1 };
const BATTERY_SERVICE_UUID: &str = "0000180f-0000-1000-8000-00805f9b34fb";
const BATTERY_LEVEL_UUID: &str = "00002A19-0000-1000-8000-00805f9b34fb";

/// Struct to manage all the shared state of the tool.
struct BatteryState {
    inner: Mutex<BatteryStateInner>,
}

impl BatteryState {
    pub fn new(service: gatt::LocalServiceControlHandle) -> BatteryState {
        BatteryState {
            inner: Mutex::new(BatteryStateInner { level: 0, service, peers: HashSet::new() }),
        }
    }

    /// Add a new peer to the set of peers interested in the battery level change notifications.
    pub fn add_peer(&self, peer_id: PeerId) {
        let _ = self.inner.lock().peers.insert(peer_id);
    }

    /// Remove a peer from the set of peers interested in notifications.
    pub fn remove_peer(&self, peer_id: &PeerId) {
        let _ = self.inner.lock().peers.remove(peer_id);
    }

    /// Get the last reported level of the battery as a percentage in [0, 100].
    pub fn get_level(&self) -> u8 {
        self.inner.lock().level
    }

    /// Set the level to the given value and notify any interested peers
    /// of the change.
    pub fn set_level(&self, level: u8) -> Result<(), Error> {
        let mut inner = self.inner.lock();
        let current_level = inner.level;
        if current_level != level {
            info!("Battery level changed: (current = {current_level}%, new = {level}%)");
            let params = gatt::ValueChangedParameters {
                peer_ids: Some(inner.peers.iter().cloned().map(Into::into).collect()),
                handle: Some(BATTERY_LEVEL_CHARACTERISTIC_HANDLE),
                value: Some(vec![level]),
                ..Default::default()
            };
            inner.service.send_on_notify_value(&params)?;
        }
        inner.level = level;
        Ok(())
    }
}

/// Inner data fields used for the `BatteryState` struct.
struct BatteryStateInner {
    /// The current battery percentage. In the range [0, 100].
    level: u8,

    /// The control handle used to send GATT notifications.
    service: gatt::LocalServiceControlHandle,

    /// Set of remote peers that have subscribed to GATT notifications for the
    /// Battery Level characteristic.
    peers: HashSet<PeerId>,
}

/// Handle a stream of incoming gatt battery service requests.
/// Returns when the channel backing the stream closes or an error occurs while handling requests.
async fn gatt_service_delegate(
    state: &BatteryState,
    mut stream: gatt::LocalServiceRequestStream,
) -> Result<(), Error> {
    use gatt::LocalServiceRequest;
    while let Some(request) = stream.try_next().await.context("error running service delegate")? {
        match request {
            LocalServiceRequest::ReadValue { responder, .. } => {
                let battery_level = state.get_level();
                let _ = responder.send(Ok(&[battery_level]));
            }
            LocalServiceRequest::WriteValue { responder, .. } => {
                // Writing to the battery level characteristic is not permitted.
                let _ = responder.send(Err(gatt::Error::WriteRequestRejected));
            }
            LocalServiceRequest::CharacteristicConfiguration {
                peer_id, notify, responder, ..
            } => {
                let peer_id = peer_id.into();
                info!(peer_id:%; "Configured characteristic (notify: {notify})");
                if notify {
                    state.add_peer(peer_id);
                } else {
                    state.remove_peer(&peer_id);
                }
                let _ = responder.send();
            }
            LocalServiceRequest::ValueChangedCredit { .. } => {}
            LocalServiceRequest::PeerUpdate { responder, .. } => {
                // Per FIDL docs, this can be safely ignored
                responder.drop_without_shutdown();
            }
        }
    }
    warn!("Battery GATT Server was closed");
    Ok(())
}

/// Watches and saves updates from the local battery service.
async fn battery_manager_watcher(
    state: &BatteryState,
    mut battery_client: BatteryClient,
) -> Result<(), Error> {
    while let Some(update) = battery_client.next().await {
        if let Some(battery_level) = update.map(|u| u.level())? {
            state.set_level(battery_level)?;
        }
    }

    warn!("BatteryClient was closed; battery level updates are no longer available.");
    Ok(())
}

#[fuchsia::main(logging_tags = ["bt-le-battery-service"])]
async fn main() -> Result<(), Error> {
    let config = Config::take_from_startup_handle();
    let security = match config.security.as_str() {
        "none" => gatt::SecurityRequirements::default(),
        "enc" => {
            gatt::SecurityRequirements { encryption_required: Some(true), ..Default::default() }
        }
        "auth" => gatt::SecurityRequirements {
            encryption_required: Some(true),
            authentication_required: Some(true),
            ..Default::default()
        },
        other => return Err(format_err!("invalid security value: {}", other)),
    };
    info!("Starting LE Battery service with security: {:?}", security);

    // Connect to the gatt2.Server protocol to publish the service.
    let gatt_server = connect_to_protocol::<gatt::Server_Marker>()?;
    let (service_client, service_stream) = create_request_stream::<gatt::LocalServiceMarker>();
    let service_notification_handle = service_stream.control_handle();

    // Connect to the battery service and initialize the shared state.
    let battery_client = BatteryClient::create()?;
    let state = BatteryState::new(service_notification_handle);

    // Build a GATT Battery service with the mandatory Battery Level characteristic.
    let battery_level_chrc = gatt::Characteristic {
        handle: Some(BATTERY_LEVEL_CHARACTERISTIC_HANDLE),
        type_: Uuid::from_str(BATTERY_LEVEL_UUID).ok().map(Into::into),
        properties: Some(
            gatt::CharacteristicPropertyBits::READ | gatt::CharacteristicPropertyBits::NOTIFY,
        ),
        permissions: Some(gatt::AttributePermissions {
            read: Some(security.clone()),
            update: Some(security),
            ..Default::default()
        }),
        ..Default::default()
    };
    let service_info = gatt::ServiceInfo {
        handle: Some(BATTERY_SERVICE_HANDLE),
        kind: Some(gatt::ServiceKind::Primary),
        type_: Uuid::from_str(BATTERY_SERVICE_UUID).ok().map(Into::into),
        characteristics: Some(vec![battery_level_chrc]),
        ..Default::default()
    };

    // Publish the local gatt service delegate with the gatt service.
    gatt_server
        .publish_service(&service_info, service_client)
        .await?
        .map_err(|e| format_err!("Failed to publish battery service to gatt server: {:?}", e))?;
    info!("Published Battery Service to local GATT database.");

    // Start the gatt service delegate and battery watcher server.
    let service_delegate = gatt_service_delegate(&state, service_stream);
    let battery_watcher = battery_manager_watcher(&state, battery_client);
    try_join!(service_delegate, battery_watcher).map(|((), ())| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use async_test_helpers::expect_stream_item;
    use async_utils::PollExt;
    use fuchsia_async as fasync;
    use std::pin::pin;

    #[fuchsia::test]
    fn read_battery_level() {
        let mut exec = fasync::TestExecutor::new();
        let (client, server) =
            fidl::endpoints::create_proxy_and_stream::<gatt::LocalServiceMarker>();
        let service_notification_handle = server.control_handle();
        let state = BatteryState::new(service_notification_handle);
        let mut gatt_server_fut = pin!(gatt_service_delegate(&state, server));
        exec.run_until_stalled(&mut gatt_server_fut).expect_pending("server active");

        let mut read_fut =
            pin!(client.read_value(&PeerId(123).into(), &BATTERY_LEVEL_CHARACTERISTIC_HANDLE, 0));
        exec.run_until_stalled(&mut read_fut).expect_pending("GATT read waiting for response");
        exec.run_until_stalled(&mut gatt_server_fut).expect_pending("server active");
        let result = exec
            .run_until_stalled(&mut read_fut)
            .expect("GATT read response")
            .expect("successful FIDL request");
        // The initial battery level is 0.
        assert_eq!(result, Ok(vec![0]));
    }

    #[fuchsia::test]
    fn battery_level_change_notifications() {
        let mut exec = fasync::TestExecutor::new();
        let (client, server) =
            fidl::endpoints::create_proxy_and_stream::<gatt::LocalServiceMarker>();
        let service_notification_handle = server.control_handle();
        let state = BatteryState::new(service_notification_handle);
        let mut gatt_server_fut = pin!(gatt_service_delegate(&state, server));
        exec.run_until_stalled(&mut gatt_server_fut).expect_pending("server active");

        // Peer subscribes to notifications on the Battery Level characteristic.
        let id = PeerId(123).into();
        let mut notify_fut = pin!(client.characteristic_configuration(
            &id,
            &BATTERY_LEVEL_CHARACTERISTIC_HANDLE,
            true,
            false
        ));
        exec.run_until_stalled(&mut notify_fut)
            .expect_pending("GATT notify request waiting for response");
        exec.run_until_stalled(&mut gatt_server_fut).expect_pending("server active");
        let result = exec.run_until_stalled(&mut notify_fut).expect("GATT notify response");
        assert_matches!(result, Ok(()));

        // Simulate battery level change.
        state.set_level(50).expect("successfully notified");

        // Should receive battery level as a notification on the FIDL client event stream.
        let mut notifications = client.take_event_stream();
        let event = expect_stream_item(&mut exec, &mut notifications);
        match event {
            Ok(gatt::LocalServiceEvent::OnNotifyValue {
                payload: gatt::ValueChangedParameters { handle, peer_ids, value, .. },
            }) => {
                assert_eq!(handle, Some(BATTERY_LEVEL_CHARACTERISTIC_HANDLE));
                assert_eq!(peer_ids, Some(vec![id]));
                assert_eq!(value, Some(vec![50]));
            }
            x => panic!("Expected notification, got: {x:?}"),
        }
    }

    #[fuchsia::test]
    fn write_is_error() {
        let mut exec = fasync::TestExecutor::new();
        let (client, server) =
            fidl::endpoints::create_proxy_and_stream::<gatt::LocalServiceMarker>();
        let service_notification_handle = server.control_handle();
        let state = BatteryState::new(service_notification_handle);
        let mut gatt_server_fut = pin!(gatt_service_delegate(&state, server));

        exec.run_until_stalled(&mut gatt_server_fut).expect_pending("server active");

        // The Battery Level characteristic doesn't support GATT writes.
        let request = gatt::LocalServiceWriteValueRequest {
            peer_id: Some(PeerId(123).into()),
            handle: Some(BATTERY_LEVEL_CHARACTERISTIC_HANDLE),
            offset: Some(0),
            value: Some(vec![1]),
            ..Default::default()
        };
        let mut write_fut = pin!(client.write_value(&request));
        exec.run_until_stalled(&mut write_fut).expect_pending("GATT write waiting for response");
        exec.run_until_stalled(&mut gatt_server_fut).expect_pending("server active");
        let result = exec.run_until_stalled(&mut write_fut).expect("GATT write response");
        assert_matches!(result, Ok(Err(gatt::Error::WriteRequestRejected)));
    }
}
