// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Error};
use async_utils::hanging_get::client::HangingGetStream;
use fidl_fuchsia_bluetooth_le::{
    CentralMarker, CentralProxy, ConnectionMarker, ConnectionOptions, ConnectionProxy, Filter,
    Peer, ScanOptions, ScanResultWatcherMarker, ScanResultWatcherProxy,
};
use fuchsia_bluetooth::types::le::Peer as btPeer;
use fuchsia_bluetooth::types::{PeerId, Uuid};
use fuchsia_component::client::connect_to_protocol;
use futures::stream::{StreamExt, TryStreamExt};
use log::{info, warn};
use std::collections::HashMap;
use {fidl_fuchsia_bluetooth_gatt2 as gatt, fuchsia_async as fasync};

/// The UUID of with the GATT battery service.
const BATTERY_SERVICE_UUID: Uuid = Uuid::new16(0x180f);
/// The UUID of the GATT Battery level characteristic.
const BATTERY_LEVEL_UUID: Uuid = Uuid::new16(0x2a19);

/// Represents an active LE connection to a remote peer that supports the Battery Service.
#[derive(Debug)]
struct BatteryClient {
    id: PeerId,
    /// LE connection to the peer - must be kept alive for GATT operations.
    _connection: ConnectionProxy,
    /// Connection to the peer's GATT services.
    _gatt: gatt::ClientProxy,
    /// Connection to the peer's GATT Battery service.
    _gatt_service_connection: gatt::RemoteServiceProxy,
    /// A task that monitors the current battery level of the peer. None if notifications aren't
    /// supported.
    _monitor_task: Option<fasync::Task<()>>,
}

fn is_battery_service(info: &gatt::ServiceInfo) -> bool {
    info.type_.is_some_and(|t| t == BATTERY_SERVICE_UUID.into())
}

fn is_battery_level(char: &gatt::Characteristic) -> bool {
    char.type_.is_some_and(|t| t == BATTERY_LEVEL_UUID.into())
}

fn is_readable(char: &gatt::Characteristic) -> bool {
    char.properties.is_some_and(|p| p.contains(gatt::CharacteristicPropertyBits::READ))
}

fn is_notifiable(char: &gatt::Characteristic) -> bool {
    char.properties.is_some_and(|p| p.contains(gatt::CharacteristicPropertyBits::NOTIFY))
}

/// Attempts to parse the GATT Read `result` and return a battery value in the range [0, 100].
/// Returns Error if the GATT Read Result is invalidly formatted.
fn read_battery_level(result: gatt::ReadValue) -> Result<u8, Error> {
    let Some(battery_level_bytes) = result.value else { return Err(format_err!("Missing value")) };
    if battery_level_bytes.len() != 1 {
        return Err(format_err!("Invalidly formatted: {battery_level_bytes:?}"));
    }
    let battery_percent = battery_level_bytes[0].clamp(0, 100);
    Ok(battery_percent)
}

/// Returns the handle associated with the battery level characteristic and a flag if notifications
/// are supported.
/// Returns Error if no compatible characteristic could be found.
fn read_characteristics(
    characteristics: Vec<gatt::Characteristic>,
) -> Result<(gatt::Handle, bool), Error> {
    let Some(chrc) = characteristics.iter().find(|c| is_battery_level(c) && is_readable(c)) else {
        return Err(format_err!("No battery level characteristic"));
    };
    // TODO(https://fxbug.dev/42074755): `handle` check can be removed when converted to a local type.
    let handle = chrc.handle.ok_or_else(|| format_err!("characteristic missing handle"))?;
    Ok((handle, is_notifiable(chrc)))
}

/// Parses the set of `services` and returns the Handle of the first valid Battery Service.
fn read_services(services: Vec<gatt::ServiceInfo>) -> Result<gatt::ServiceHandle, Error> {
    let Some(service) = services.into_iter().find(is_battery_service) else {
        return Err(format_err!("No compatible Battery service"));
    };
    // TODO(https://fxbug.dev/42074755): `handle` check can be removed when converted to a local type.
    service.handle.ok_or_else(|| format_err!("service missing handle"))
}

/// Monitors GATT notifications received in the `stream` and logs changes in battery level.
async fn watch_battery_level(id: PeerId, mut stream: gatt::CharacteristicNotifierRequestStream) {
    while let Some(notification) = stream.next().await {
        // `OnNotification` is the only request type in this protocol.
        let Ok((notif, responder)) =
            notification.map(|n| n.into_on_notification().expect("only request"))
        else {
            continue;
        };
        let _ = responder.send();
        match read_battery_level(notif) {
            Ok(battery_percent) => info!(id:%; "Battery level: {battery_percent}"),
            Err(e) => warn!(id:%; "Couldn't read battery level: {e:?}"),
        }
        // TODO(https://fxbug.dev/42074786): Integrate this with the Fuchsia power reporting services using
        // the `fuchsia.bluetooth.power.Reporter` API.
    }
    info!(id:%; "Battery notifications terminated");
}

/// Attempts to connect to the peer and read the battery level.
/// Returns OK if the battery level characteristic was successfully read, Error otherwise.
async fn try_connect(id: PeerId, central: &CentralProxy) -> Result<BatteryClient, Error> {
    info!(id:%; "Trying to connect");
    // Try to connect and establish a GATT connection.
    let (le_client, le_server) = fidl::endpoints::create_proxy::<ConnectionMarker>();
    central.connect(&id.into(), &ConnectionOptions::default(), le_server)?;
    let (gatt_client, gatt_server) = fidl::endpoints::create_proxy::<gatt::ClientMarker>();
    le_client.request_gatt_client(gatt_server)?;

    // Read the GATT services offered by the peer.
    let uuids = &[BATTERY_SERVICE_UUID.into()];
    let (added, _) = gatt_client.watch_services(uuids).await?;
    let service_handle = read_services(added)?;
    let (remote_client, remote_server) =
        fidl::endpoints::create_proxy::<gatt::RemoteServiceMarker>();
    gatt_client.connect_to_service(&service_handle, remote_server)?;

    // Discover the characteristics provided by the service.
    let characteristics = remote_client.discover_characteristics().await?;
    let (battery_level_handle, notifications) = read_characteristics(characteristics)?;

    // Read the current battery level and listen for updates if notifications are supported.
    let read_response = remote_client
        .read_characteristic(
            &battery_level_handle,
            &gatt::ReadOptions::ShortRead(gatt::ShortReadOptions),
        )
        .await?
        .map_err(|e| format_err!("{e:?}"))?;
    match read_battery_level(read_response) {
        Ok(level) => info!(id:%; "Battery level: {level}"),
        Err(e) => info!(id:%; "Failed to read battery level: {e:?}"),
    };

    let _monitor_task = if notifications {
        let (notification_client, notification_server) =
            fidl::endpoints::create_request_stream::<gatt::CharacteristicNotifierMarker>();
        remote_client
            .register_characteristic_notifier(&battery_level_handle, notification_client)
            .await?
            .map_err(|e| format_err! {"{e:?}"})?;
        Some(fasync::Task::local(watch_battery_level(id, notification_server)))
    } else {
        None
    };

    Ok(BatteryClient {
        id,
        _connection: le_client,
        _gatt: gatt_client,
        _gatt_service_connection: remote_client,
        _monitor_task,
    })
}

/// Processes the scan `result` and returns an LE connection to a peer with a compatible battery
/// service.
async fn handle_scan_result(
    central: &CentralProxy,
    result: Vec<Peer>,
) -> Result<BatteryClient, Error> {
    // Try each peer until one succeeds.
    for peer in result {
        let peer = btPeer::try_from(peer)?;
        match try_connect(peer.id, central).await {
            Ok(battery_client) => {
                info!(id:% = peer.id; "Successfully initiated LE connection");
                return Ok(battery_client);
            }
            Err(e) => info!(id:% = peer.id; "Couldn't connect: {e:?}"),
        }
    }

    Err(format_err!("No compatible peers"))
}

async fn watch_scan_results(
    central: CentralProxy,
    watcher: ScanResultWatcherProxy,
) -> Result<(), Error> {
    let mut scan_result_stream =
        HangingGetStream::new(watcher.clone(), ScanResultWatcherProxy::watch);
    // Saves active connections to peers.
    let mut peers = HashMap::new();

    while let Some(scan_result) = scan_result_stream.try_next().await? {
        match handle_scan_result(&central, scan_result).await {
            Ok(peer) => {
                let _ = peers.insert(peer.id, peer);
            }
            Err(e) => warn!("Error processing scan result: {e:?}"),
        }
    }
    info!("Scan result watcher finished");
    Ok(())
}

#[fuchsia::main(logging_tags = ["bt-le-battery-monitor"])]
async fn main() -> Result<(), Error> {
    info!("Starting LE Battery Monitor");
    let central = connect_to_protocol::<CentralMarker>()?;
    let (scan_client, scan_server) = fidl::endpoints::create_proxy::<ScanResultWatcherMarker>();
    // Only scan for devices with the Battery Service.
    let options = ScanOptions {
        filters: Some(vec![Filter {
            service_uuid: Some(BATTERY_SERVICE_UUID.into()),
            connectable: Some(true),
            ..Default::default()
        }]),
        ..Default::default()
    };
    // The lifetime of the scan will be determined by `watch_scan_results` so this Future can be
    // ignored.
    let _scan_fut = central.scan(&options, scan_server).check()?;
    let scan_result = watch_scan_results(central, scan_client).await;
    info!("LE Battery Monitor finished: {scan_result:?}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use async_test_helpers::run_while;
    use async_utils::PollExt;
    use fidl_fuchsia_bluetooth_le::CentralRequest;
    use fuchsia_async as fasync;
    use std::pin::pin;

    #[fuchsia::test]
    fn watch_scan_results_lifetime() {
        let mut exec = fasync::TestExecutor::new();
        let (central, _central_server) =
            fidl::endpoints::create_proxy_and_stream::<CentralMarker>();
        let (watch_client, mut watch_server) =
            fidl::endpoints::create_proxy_and_stream::<ScanResultWatcherMarker>();

        let watch_fut = pin!(watch_scan_results(central.clone(), watch_client));
        let server_fut = pin!(watch_server.next());
        // Should receive initial request to watch.
        let (result, mut watch_fut) = run_while(&mut exec, watch_fut, server_fut);
        let responder = result.unwrap().expect("fidl request").into_watch().unwrap();

        let _ = responder.send(&[]);
        // If the ScanResultWatcher server terminates, then the watcher future should terminate.
        drop(watch_server);

        // After running the watcher, it will register a new call to `watch` and will fail because
        // the server is gone.
        let result = exec.run_until_stalled(&mut watch_fut).expect("finished");
        assert_matches!(result, Err(_));
    }

    #[fuchsia::test]
    fn watch_scan_results_empty_reply_is_ok() {
        let mut exec = fasync::TestExecutor::new();
        let (central, _central_server) =
            fidl::endpoints::create_proxy_and_stream::<CentralMarker>();
        let (watch_client, mut watch_server) =
            fidl::endpoints::create_proxy_and_stream::<ScanResultWatcherMarker>();

        let watch_fut = pin!(watch_scan_results(central.clone(), watch_client));
        let server_fut = pin!(watch_server.next());
        // Should receive initial request to watch.
        let (result, mut watch_fut) = run_while(&mut exec, watch_fut, server_fut);
        let responder = result.unwrap().expect("fidl request").into_watch().unwrap();
        // Replying with no found peers is OK. Watcher should still be active.
        let _ = responder.send(&[]);
        let _ = exec.run_until_stalled(&mut watch_fut).expect_pending("still active");
    }

    #[fuchsia::test]
    fn read_battery_level_success() {
        let value = example_battery_level();
        let parsed_battery_level = read_battery_level(value).expect("valid read result");
        assert_eq!(parsed_battery_level, 50);

        // The value should be clamped between [0, 100].
        let value = gatt::ReadValue {
            handle: Some(gatt::Handle { value: 10 }),
            value: Some(vec![u8::MAX]),
            maybe_truncated: Some(false),
            ..Default::default()
        };
        let truncated_value = read_battery_level(value).expect("valid read result");
        assert_eq!(truncated_value, 100);
    }

    #[fuchsia::test]
    fn read_battery_level_error() {
        // Missing all fields is an Error.
        assert_matches!(read_battery_level(gatt::ReadValue::default()), Err(_));

        // Missing the read result value is an Error.
        let missing_value = gatt::ReadValue {
            handle: Some(gatt::Handle { value: 10 }),
            maybe_truncated: Some(false),
            ..Default::default()
        };
        assert_matches!(read_battery_level(missing_value), Err(_));

        // A non 1-byte battery value is an Error.
        let invalid_value = gatt::ReadValue {
            handle: Some(gatt::Handle { value: 10 }),
            value: Some(vec![0, 1, 2]),
            maybe_truncated: Some(false),
            ..Default::default()
        };
        assert_matches!(read_battery_level(invalid_value), Err(_));
    }

    fn example_battery_service() -> gatt::ServiceInfo {
        gatt::ServiceInfo {
            handle: Some(gatt::ServiceHandle { value: 5 }),
            kind: Some(gatt::ServiceKind::Primary),
            type_: Some(BATTERY_SERVICE_UUID.into()),
            ..Default::default()
        }
    }

    fn example_battery_level_characteristic() -> gatt::Characteristic {
        gatt::Characteristic {
            handle: Some(gatt::Handle { value: 15 }),
            type_: Some(BATTERY_LEVEL_UUID.into()),
            properties: Some(
                gatt::CharacteristicPropertyBits::READ | gatt::CharacteristicPropertyBits::NOTIFY,
            ),
            ..Default::default()
        }
    }

    fn example_battery_level() -> gatt::ReadValue {
        gatt::ReadValue {
            handle: Some(gatt::Handle { value: 10 }),
            value: Some(vec![50]),
            maybe_truncated: Some(false),
            ..Default::default()
        }
    }

    #[fuchsia::test]
    fn try_connect_success() {
        let mut exec = fasync::TestExecutor::new();

        let id = PeerId(123);
        let (central_client, mut central_server) =
            fidl::endpoints::create_proxy_and_stream::<CentralMarker>();
        let connect_fut = try_connect(id, &central_client);
        let mut connect_fut = pin!(connect_fut);

        exec.run_until_stalled(&mut connect_fut).expect_pending("waiting for result");

        // Expect a LE connect request.
        let central_fut = central_server.select_next_some();
        let (central_result, connect_fut) = run_while(&mut exec, connect_fut, central_fut);
        let mut gatt_connection_server = match central_result {
            Ok(CentralRequest::Connect { id: received_id, handle, .. }) => {
                assert_eq!(received_id, id.into());
                handle.into_stream()
            }
            x => panic!("Expected Connect got: {x:?}"),
        };

        // Expect a request to connect GATT.
        let gatt_connect_fut = gatt_connection_server.select_next_some();
        let (gatt_connect_result, connect_fut) =
            run_while(&mut exec, connect_fut, gatt_connect_fut);
        let (gatt_server, _) =
            gatt_connect_result.unwrap().into_request_gatt_client().expect("only request");
        let mut gatt_server = gatt_server.into_stream();

        // Expect a request to watch GATT services - send back the example service.
        let gatt_fut = gatt_server.select_next_some();
        let (gatt_result, connect_fut) = run_while(&mut exec, connect_fut, gatt_fut);
        let (_, responder) = gatt_result.unwrap().into_watch_services().unwrap();
        let _ = responder.send(&[example_battery_service()], &[]);

        // Expect a request to connect to the service and discover characteristics.
        let gatt_fut = gatt_server.select_next_some();
        let (gatt_result, connect_fut) = run_while(&mut exec, connect_fut, gatt_fut);
        let (_, remote_service_server, _) = gatt_result.unwrap().into_connect_to_service().unwrap();
        let mut remote_service_server = remote_service_server.into_stream();
        let discover_fut = remote_service_server.select_next_some();
        let (discover_result, connect_fut) = run_while(&mut exec, connect_fut, discover_fut);
        let responder = discover_result.unwrap().into_discover_characteristics().unwrap();
        // Send back an example battery level characteristic.
        let _ = responder.send(&[example_battery_level_characteristic()]);

        // Expect a request to read the battery level - send back an example battery level.
        let read_fut = remote_service_server.select_next_some();
        let (read_result, connect_fut) = run_while(&mut exec, connect_fut, read_fut);
        let (_, _, responder) = read_result.unwrap().into_read_characteristic().unwrap();
        let _ = responder.send(Ok(&example_battery_level()));

        // Because notifications are supported on the example characteristic, we expect a request
        // to subscribe to notifications.
        let notification_fut = remote_service_server.select_next_some();
        let (notification_result, mut connect_fut) =
            run_while(&mut exec, connect_fut, notification_fut);
        let (_, _notifier, responder) =
            notification_result.unwrap().into_register_characteristic_notifier().unwrap();
        let _ = responder.send(Ok(()));

        let result = exec.run_until_stalled(&mut connect_fut).expect("connect success");
        assert_matches!(result, Ok(_));
    }
}
