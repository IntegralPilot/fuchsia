// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use super::*;

use bt_gatt::central::AdvertisingDatum;
use bt_gatt::client::{PeerService, PeerServiceHandle};
use bt_gatt::periodic_advertising::PeriodicAdvertising;
use bt_gatt::{Central, Client};

fn create_test_central() -> (fidl_le::CentralRequestStream, super::Central) {
    let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fidl_le::CentralMarker>();
    let central = super::Central::new(proxy);
    (stream, central)
}

// Connect to a fake peer and a fake service, used for later testing of gatt actions
async fn create_test_service_client() -> (
    fidl_le::CentralRequestStream,
    super::Central,
    super::Client,
    fidl_gatt2::ClientRequestStream,
    super::PeerServiceHandle,
) {
    let (mut stream, central) = create_test_central();

    let Poll::Ready(Ok(client)) =
        fasync::TestExecutor::poll_until_stalled(central.connect(PeerId(1))).await
    else {
        panic!("Expected the client to be returned");
    };

    let mut connection_request_stream = match stream.next().await {
        Some(Ok(fidl_le::CentralRequest::Connect { handle, .. })) => handle.into_stream(),
        request => panic!("Expected a Connect request and got {request:?}"),
    };

    let test_uuid = Uuid::from_u16(0xBEAD);
    let mut find_service_fut = client.find_service(test_uuid.clone());
    assert!(fasync::TestExecutor::poll_until_stalled(&mut find_service_fut).await.is_pending());
    let mut gatt_request_stream = match connection_request_stream.next().await {
        Some(Ok(fidl_le::ConnectionRequest::RequestGattClient { client, .. })) => {
            client.into_stream()
        }
        request => panic!("Expected a Gatt Client request, got {request:?}"),
    };
    let Some(Ok(fidl_gatt2::ClientRequest::WatchServices { responder, .. })) =
        gatt_request_stream.next().await
    else {
        panic!("Didn't get WatchServices request");
    };
    use fidl_gatt2::*;
    responder
        .send(
            &[ServiceInfo {
                handle: Some(ServiceHandle { value: 1 }),
                kind: Some(ServiceKind::Primary),
                type_: Some(to_fidl_uuid(&test_uuid)),
                characteristics: None,
                includes: Some(Vec::new()),
                ..Default::default()
            }],
            &[],
        )
        .expect("send response ok");
    let mut services_returned = find_service_fut.await.unwrap();
    let service_handle = services_returned.pop().unwrap();
    (stream, central, client, gatt_request_stream, service_handle)
}

const VCS_UUID: bt_common::Uuid = bt_common::Uuid::from_u16(0x1844);
const CSIS_UUID: bt_common::Uuid = bt_common::Uuid::from_u16(0x1846);

#[fuchsia::test]
async fn central_scan() {
    let (mut requests, central) = create_test_central();

    let simple_filter: bt_gatt::central::Filter =
        bt_gatt::central::Filter::ServiceUuid(VCS_UUID).into();
    let mut complex_filter = bt_gatt::central::ScanFilter::default();
    let _ = complex_filter
        .add(bt_gatt::central::Filter::ServiceUuid(CSIS_UUID))
        .add(bt_gatt::central::Filter::IsConnectable)
        .add(bt_gatt::central::Filter::HasServiceData(CSIS_UUID));
    let _stream = central.scan(&[simple_filter.into(), complex_filter]);

    // Should have a request to start the scan
    let request = requests.next().await;
    let Some(Ok(fidl_le::CentralRequest::Scan {
        options: fidl_le::ScanOptions { filters: Some(filters), .. },
        ..
    })) = request
    else {
        panic!("Expected a Scan request with filters and got {request:?}");
    };

    assert_eq!(filters.len(), 2);
    for filter in filters {
        match filter.service_uuid {
            Some(uuid) if uuid == to_fidl_uuid(&CSIS_UUID) => {
                assert_eq!(filter.connectable, Some(true));
                assert_eq!(filter.service_data_uuid, Some(to_fidl_uuid(&CSIS_UUID)));
            }
            Some(uuid) if uuid == to_fidl_uuid(&VCS_UUID) => {}
            x => panic!("Found filter with unexpected service UUID: {x:?}"),
        }
    }
}

#[fuchsia::test(allow_stalls = false)]
async fn central_connect_client_find_service() {
    let (mut requests, central) = create_test_central();

    let connect_fut = central.connect(PeerId(1));

    // Should be able to await to make the request happen
    let connect_poll_result = fasync::TestExecutor::poll_until_stalled(connect_fut).await;

    // We return the client right away
    let Poll::Ready(Ok(client)) = connect_poll_result else {
        panic!("Expected the client to be returned");
    };

    let (id, mut connection_request_stream) = match requests.next().await {
        Some(Ok(fidl_le::CentralRequest::Connect { id, handle, .. })) => (id, handle.into_stream()),
        request => panic!("Expected a Connect request and got {request:?}"),
    };

    assert_eq!(to_gatt_peer_id(&id), PeerId(1));
    assert_eq!(to_fidl_peer_id(&client.peer_id()), id);

    let test_uuid = Uuid::from_u16(0xBEAD);

    let mut find_service_fut = client.find_service(test_uuid.clone());

    assert!(fasync::TestExecutor::poll_until_stalled(&mut find_service_fut).await.is_pending());

    let gatt_client_server = match connection_request_stream.next().await {
        Some(Ok(fidl_le::ConnectionRequest::RequestGattClient { client, .. })) => client,
        request => panic!("Expected a Gatt Client request, got {request:?}"),
    };

    let mut gatt_request_stream = gatt_client_server.into_stream();

    let gatt_request = gatt_request_stream.next().await;
    let Some(Ok(fidl_gatt2::ClientRequest::WatchServices { uuids, responder })) = gatt_request
    else {
        panic!("Expected ask for services got {gatt_request:?}");
    };

    assert_eq!(uuids, vec![to_fidl_uuid(&test_uuid)]);

    use fidl_gatt2::*;
    responder
        .send(
            &[ServiceInfo {
                handle: Some(ServiceHandle { value: 1 }),
                kind: Some(ServiceKind::Primary),
                type_: Some(to_fidl_uuid(&test_uuid)),
                characteristics: None,
                includes: Some(Vec::new()),
                ..Default::default()
            }],
            &[],
        )
        .expect("send response ok");

    let mut services_returned = find_service_fut.await.unwrap();

    assert_eq!(services_returned.len(), 1);
    let service = services_returned.pop().unwrap();

    assert_eq!(service.uuid(), test_uuid);
    assert!(service.is_primary());

    // Find one that is not there (we never send a response
    let service_fut = client.find_service(Uuid::from_u16(0xCAFE));

    let gatt_request = gatt_request_stream.next().await;
    let Some(Ok(fidl_gatt2::ClientRequest::WatchServices { .. })) = gatt_request else {
        panic!("Expected ask for services got {gatt_request:?}");
    };

    // Advance time so that the timeout happens immediately.
    fasync::TestExecutor::advance_to(fasync::MonotonicInstant::after(
        fasync::MonotonicDuration::from_seconds(10),
    ))
    .await;
    let services_returned = service_fut.await.unwrap();

    assert_eq!(services_returned.len(), 0);
}

#[fuchsia::test(allow_stalls = false)]
async fn connect_service_and_read_write() {
    let (_central_requests, _central, _client, mut client_requests, peer_service_handle) =
        create_test_service_client().await;

    // We return the connection immediately
    let peer_service = peer_service_handle.connect().await.unwrap();

    let mut service_requests = match client_requests.next().await {
        Some(Ok(fidl_gatt2::ClientRequest::ConnectToService { service, .. })) => {
            service.into_stream()
        }
        request => panic!("Expected ConnectToService got {request:?}"),
    };

    let mut discover_characteristics_fut = peer_service.discover_characteristics(None);

    assert!(
        fasync::TestExecutor::poll_until_stalled(&mut discover_characteristics_fut)
            .await
            .is_pending()
    );

    let disc_char_responder = match service_requests.next().await {
        Some(Ok(fidl_gatt2::RemoteServiceRequest::DiscoverCharacteristics { responder })) => {
            responder
        }
        x => panic!("Expected DiscoverCharacteristics, got {x:?}"),
    };

    let security_none = fidl_gatt2::SecurityRequirements::default();
    let rw_permissions = fidl_gatt2::AttributePermissions {
        read: Some(security_none.clone()),
        write: Some(security_none.clone()),
        update: Some(security_none.clone()),
        ..Default::default()
    };

    {
        use fidl_gatt2::*;
        disc_char_responder
            .send(&[Characteristic {
                handle: Some(Handle { value: 1 }),
                type_: Some(to_fidl_uuid(&Uuid::from_u16(0xC001))),
                properties: Some(
                    CharacteristicPropertyBits::READ | CharacteristicPropertyBits::WRITE,
                ),
                permissions: Some(rw_permissions.clone()),
                descriptors: Some(Vec::new()),
                ..Default::default()
            }])
            .unwrap();
    }

    let mut characteristics = discover_characteristics_fut.await.unwrap();

    assert_eq!(characteristics.len(), 1);
    let c = characteristics.pop().unwrap();

    assert_eq!(c.handles().count(), 1);
    assert_eq!(c.descriptors().count(), 0);
    {
        use bt_gatt::types::CharacteristicProperty;
        assert!(!c.supports_property(&CharacteristicProperty::Notify));
        assert!(c.supports_property(&CharacteristicProperty::Read));
        assert!(c.supports_property(&CharacteristicProperty::Write));
    }

    let mut buf = [0u8; 255];
    let mut read_fut = peer_service.read_characteristic(&c.handle, 0, &mut buf);

    assert!(fasync::TestExecutor::poll_until_stalled(&mut read_fut).await.is_pending());

    let read_responder = match service_requests.next().await {
        Some(Ok(fidl_gatt2::RemoteServiceRequest::ReadCharacteristic {
            handle,
            options,
            responder,
        })) => {
            assert_eq!(handle.value, 1);
            assert!(
                matches!(options, fidl_gatt2::ReadOptions::LongRead(fidl_gatt2::LongReadOptions {
                offset, .. }) if offset == Some(0))
            );
            responder
        }
        x => panic!("Expected read request got {x:?}"),
    };

    read_responder
        .send(Ok(&fidl_gatt2::ReadValue {
            handle: Some(fidl_gatt2::Handle { value: 1 }),
            value: Some(vec![0xf0, 0x9f, 0x92, 0x96]),
            maybe_truncated: Some(false),
            ..Default::default()
        }))
        .unwrap();

    let read_result = read_fut.await.unwrap();
    assert_eq!(read_result, (4, false));
    assert_eq!(buf[0..4], [0xf0, 0x9f, 0x92, 0x96]);

    let write_buf = [4, 0, 4];

    let mut write_fut = peer_service.write_characteristic(
        &c.handle,
        bt_gatt::types::WriteMode::None,
        0,
        &write_buf,
    );

    assert!(fasync::TestExecutor::poll_until_stalled(&mut write_fut).await.is_pending());

    let write_responder = match service_requests.next().await {
        Some(Ok(fidl_gatt2::RemoteServiceRequest::WriteCharacteristic {
            handle,
            value,
            options,
            responder,
        })) => {
            assert_eq!(handle.value, 1);
            assert!(matches!(options, fidl_gatt2::WriteOptions { write_mode,
                offset, .. } if offset == Some(0) && write_mode == Some(fidl_gatt2::WriteMode::Default)));
            assert_eq!(value, vec![4, 0, 4]);
            responder
        }
        x => panic!("Expected read request got {x:?}"),
    };

    write_responder.send(Ok(())).unwrap();

    write_fut.await.unwrap();
}

#[fuchsia::test(allow_stalls = false)]
async fn connect_service_and_notify() {
    let (_central_requests, _central, _client, mut client_requests, peer_service_handle) =
        create_test_service_client().await;

    // We return the connection immediately
    let peer_service = peer_service_handle.connect().await.unwrap();

    let mut service_requests = match client_requests.next().await {
        Some(Ok(fidl_gatt2::ClientRequest::ConnectToService { service, .. })) => {
            service.into_stream()
        }
        request => panic!("Expected ConnectToService got {request:?}"),
    };

    let mut discover_characteristics_fut = peer_service.discover_characteristics(None);

    assert!(
        fasync::TestExecutor::poll_until_stalled(&mut discover_characteristics_fut)
            .await
            .is_pending()
    );

    let disc_char_responder = match service_requests.next().await {
        Some(Ok(fidl_gatt2::RemoteServiceRequest::DiscoverCharacteristics { responder })) => {
            responder
        }
        x => panic!("Expected DiscoverCharacteristics, got {x:?}"),
    };

    let security_none = fidl_gatt2::SecurityRequirements::default();
    let rw_permissions = fidl_gatt2::AttributePermissions {
        read: Some(security_none.clone()),
        write: Some(security_none.clone()),
        update: Some(security_none.clone()),
        ..Default::default()
    };

    {
        use fidl_gatt2::*;
        disc_char_responder
            .send(&[Characteristic {
                handle: Some(Handle { value: 1 }),
                type_: Some(to_fidl_uuid(&Uuid::from_u16(0xC001))),
                properties: Some(CharacteristicPropertyBits::NOTIFY),
                permissions: Some(rw_permissions.clone()),
                descriptors: Some(Vec::new()),
                ..Default::default()
            }])
            .unwrap();
    }

    let mut characteristics = discover_characteristics_fut.await.unwrap();
    let c = characteristics.pop().unwrap();
    {
        use bt_gatt::types::CharacteristicProperty;
        assert!(c.supports_property(&CharacteristicProperty::Notify));
    }

    let mut buf = [0u8; 255];
    let mut read_fut = peer_service.read_characteristic(&c.handle, 0, &mut buf);

    assert!(fasync::TestExecutor::poll_until_stalled(&mut read_fut).await.is_pending());

    let read_responder = match service_requests.next().await {
        Some(Ok(fidl_gatt2::RemoteServiceRequest::ReadCharacteristic {
            handle,
            options,
            responder,
        })) => {
            assert_eq!(handle.value, 1);
            assert!(
                matches!(options, fidl_gatt2::ReadOptions::LongRead(fidl_gatt2::LongReadOptions {
                offset, .. }) if offset == Some(0))
            );
            responder
        }
        x => panic!("Expected read request got {x:?}"),
    };

    read_responder.send(Err(fidl_gatt2::Error::ReadNotPermitted)).unwrap();

    let read_result = read_fut.await.unwrap_err();
    assert!(matches!(
        read_result,
        bt_gatt::types::Error::Gatt(bt_gatt::types::GattError::ReadNotPermitted)
    ));

    let write_buf = [4, 0, 4];
    let mut write_fut = peer_service.write_characteristic(
        &c.handle,
        bt_gatt::types::WriteMode::None,
        0,
        &write_buf,
    );

    assert!(fasync::TestExecutor::poll_until_stalled(&mut write_fut).await.is_pending());

    let write_responder = match service_requests.next().await {
        Some(Ok(fidl_gatt2::RemoteServiceRequest::WriteCharacteristic {
            handle,
            value,
            options,
            responder,
        })) => {
            assert_eq!(handle.value, 1);
            assert!(matches!(options, fidl_gatt2::WriteOptions { write_mode,
                offset, .. } if offset == Some(0) && write_mode == Some(fidl_gatt2::WriteMode::Default)));
            assert_eq!(value, vec![4, 0, 4]);
            responder
        }
        x => panic!("Expected read request got {x:?}"),
    };

    write_responder.send(Err(fidl_gatt2::Error::ApplicationError84)).unwrap();

    let err = write_fut.await.unwrap_err();

    assert!(matches!(
        err,
        bt_gatt::types::Error::Gatt(bt_gatt::types::GattError::ApplicationError84)
    ));

    let mut subscription = peer_service.subscribe(&c.handle);

    let notifier_proxy = match service_requests.next().await {
        Some(Ok(fidl_gatt2::RemoteServiceRequest::RegisterCharacteristicNotifier {
            handle,
            notifier,
            responder,
        })) => {
            assert_eq!(handle.value, 1);
            responder.send(Ok(())).unwrap();
            notifier.into_proxy()
        }
        x => panic!("Expected notifier registration, got {x:?}"),
    };

    let mut notify_response_fut = notifier_proxy.on_notification(&fidl_gatt2::ReadValue {
        handle: Some(fidl_gatt2::Handle { value: 1 }),
        value: Some(vec![0xDE, 0xAD]),
        maybe_truncated: Some(false),
        ..Default::default()
    });

    assert!(fasync::TestExecutor::poll_until_stalled(&mut notify_response_fut).await.is_pending());

    match subscription.next().await {
        Some(Ok(notification)) => {
            assert_eq!(notification.handle, c.handle);
            assert_eq!(notification.value, vec![0xDE, 0xAD]);
            assert_eq!(notification.maybe_truncated, false);
        }
        x => panic!("Expected a value from the notify, got {x:?}"),
    }
}

#[fuchsia::test(allow_stalls = false)]
async fn periodic_advertising_sync_success() {
    let (mut central_requests, central) = create_test_central();

    let pa = central.periodic_advertising().unwrap();
    let peer_id = PeerId(42);
    let adv_sid = 2;
    let config = bt_gatt::periodic_advertising::SyncConfiguration { filter_duplicates: true };

    let mut sync_fut = pa.sync_to_advertising_reports(peer_id, adv_sid, config);

    // We need to poll the future to make it progress and send the FIDL request.
    assert!(fasync::TestExecutor::poll_until_stalled(&mut sync_fut).await.is_pending());

    // Verify Central received SyncToPeriodicAdvertising request
    let sync_server_end = match central_requests.next().await {
        Some(Ok(fidl_le::CentralRequest::SyncToPeriodicAdvertising { payload, .. })) => {
            assert_eq!(payload.peer_id, Some(to_fidl_peer_id(&peer_id)));
            assert_eq!(payload.advertising_sid, Some(adv_sid));
            assert_eq!(payload.config.and_then(|c| c.filter_duplicates), Some(true));
            payload.sync.unwrap()
        }
        request => panic!("Expected SyncToPeriodicAdvertising request, got {request:?}"),
    };

    let mut sync_request_stream = sync_server_end.into_stream();

    // Mock establishment: Send OnEstablished event
    let control_handle = sync_request_stream.control_handle();
    control_handle
        .send_on_established(&fidl_le::PeriodicAdvertisingSyncOnEstablishedRequest {
            id: Some(fidl_le::PeriodicAdvertisingSyncId { value: 100 }),
            peer_id: Some(to_fidl_peer_id(&peer_id)),
            advertising_sid: Some(adv_sid),
            phy: Some(fidl_le::PhysicalLayer::Le1M),
            periodic_advertising_interval: Some(100),
            ..Default::default()
        })
        .expect("send on_established ok");

    // Now the future should resolve to Ok(stream)
    let mut sync_stream = match fasync::TestExecutor::poll_until_stalled(&mut sync_fut).await {
        Poll::Ready(Ok(stream)) => stream,
        Poll::Ready(Err(e)) => panic!("Expected Ready(Ok(stream)), got Ready(Err({e:?}))"),
        Poll::Pending => panic!("Expected Ready(Ok(stream)), got Pending"),
    };

    // Now we test receiving reports.
    // The HangingGetStream should have called WatchAdvertisingReport eagerly.
    let watch_responder = match sync_request_stream.next().await {
        Some(Ok(fidl_le::PeriodicAdvertisingSyncRequest::WatchAdvertisingReport { responder })) => {
            responder
        }
        request => panic!("Expected WatchAdvertisingReport request, got {request:?}"),
    };

    // Mock report
    let service_uuid = Uuid::from_u16(0x1852);
    let service_data =
        fidl_le::ServiceData { uuid: to_fidl_uuid(&service_uuid), data: vec![0x01, 0x02, 0x03] };
    let report = fidl_le::PeriodicAdvertisingReport {
        rssi: Some(-50),
        data: Some(fidl_le::ScanData {
            service_data: Some(vec![service_data]),
            ..Default::default()
        }),
        event_counter: Some(5),
        subevent: Some(1),
        timestamp: Some(1234567),
        ..Default::default()
    };

    watch_responder
        .send(&fidl_le::PeriodicAdvertisingSyncWatchAdvertisingReportResponse {
            reports: Some(vec![fidl_le::SyncReport::PeriodicAdvertisingReport(report)]),
            ..Default::default()
        })
        .expect("send report ok");

    // Poll the stream to get the report
    let mut next_report_fut = sync_stream.next();
    let report_res = match fasync::TestExecutor::poll_until_stalled(&mut next_report_fut).await {
        Poll::Ready(Some(res)) => res,
        res => panic!("Expected Ready(Some(res)), got {res:?}"),
    };

    let gatt_report = report_res.unwrap();
    match gatt_report {
        bt_gatt::periodic_advertising::SyncReport::PeriodicAdvertisingReport(r) => {
            assert_eq!(r.rssi, -50);
            assert_eq!(r.data.len(), 1);
            let AdvertisingDatum::ServiceData(uuid, data) = &r.data[0] else {
                panic!("Expected ServiceData datum");
            };
            assert_eq!(*uuid, service_uuid);
            assert_eq!(*data, vec![1, 2, 3]);
            assert_eq!(r.event_counter, Some(5));
            assert_eq!(r.subevent, Some(1));
            assert_eq!(r.timestamp, 1234567);
        }
        r => panic!("Expected PeriodicAdvertisingReport, got {r:?}"),
    }
}

#[fuchsia::test(allow_stalls = false)]
async fn periodic_advertising_sync_lost() {
    let (mut central_requests, central) = create_test_central();

    let pa = central.periodic_advertising().unwrap();
    let peer_id = PeerId(42);
    let adv_sid = 2;
    let config = bt_gatt::periodic_advertising::SyncConfiguration { filter_duplicates: true };

    let mut sync_fut = pa.sync_to_advertising_reports(peer_id, adv_sid, config);
    assert!(fasync::TestExecutor::poll_until_stalled(&mut sync_fut).await.is_pending());

    let sync_server_end = match central_requests.next().await {
        Some(Ok(fidl_le::CentralRequest::SyncToPeriodicAdvertising { payload, .. })) => {
            payload.sync.unwrap()
        }
        request => panic!("Expected SyncToPeriodicAdvertising request, got {request:?}"),
    };

    let sync_request_stream = sync_server_end.into_stream();
    let control_handle = sync_request_stream.control_handle();

    // Establish
    control_handle
        .send_on_established(&fidl_le::PeriodicAdvertisingSyncOnEstablishedRequest {
            id: Some(fidl_le::PeriodicAdvertisingSyncId { value: 100 }),
            peer_id: Some(to_fidl_peer_id(&peer_id)),
            advertising_sid: Some(adv_sid),
            phy: Some(fidl_le::PhysicalLayer::Le1M),
            periodic_advertising_interval: Some(100),
            ..Default::default()
        })
        .expect("send on_established ok");

    let mut sync_stream = match fasync::TestExecutor::poll_until_stalled(&mut sync_fut).await {
        Poll::Ready(Ok(stream)) => stream,
        Poll::Ready(Err(e)) => panic!("Expected Ready(Ok(stream)), got Ready(Err({e:?}))"),
        Poll::Pending => panic!("Expected Ready(Ok(stream)), got Pending"),
    };

    // Send OnError (Sync Lost)
    control_handle
        .send_on_error(fidl_le::PeriodicAdvertisingSyncError::SynchronizationLost)
        .expect("send on_error ok");

    // Poll the stream. It should yield Err(SyncLost).
    let mut next_report_fut = sync_stream.next();
    let report_res = match fasync::TestExecutor::poll_until_stalled(&mut next_report_fut).await {
        Poll::Ready(Some(res)) => res,
        res => panic!("Expected Ready(Some(res)), got {res:?}"),
    };

    let err = report_res.unwrap_err();
    assert!(format!("{err:?}").contains("SyncLost"));

    // Simulate channel closure by dropping the server stream and control handle
    drop(control_handle);
    drop(sync_request_stream);

    // The in-flight WatchAdvertisingReport call should fail due to channel closure
    let mut next_report_fut = sync_stream.next();
    let err_res = fasync::TestExecutor::poll_until_stalled(&mut next_report_fut).await;
    let Poll::Ready(Some(Err(e))) = err_res else {
        panic!("Expected Ready(Some(Err)), got {err_res:?}");
    };
    assert!(
        format!("{e:?}").contains("ClientChannelClosed")
            || format!("{e:?}").contains("PeerClosed")
            || format!("{e:?}").contains("SyncLost")
    );
}

#[fuchsia::test(allow_stalls = false)]
async fn periodic_advertising_sync_filter_empty_info() {
    let (mut central_requests, central) = create_test_central();

    let pa = central.periodic_advertising().unwrap();
    let peer_id = PeerId(42);
    let adv_sid = 2;
    let config = bt_gatt::periodic_advertising::SyncConfiguration { filter_duplicates: true };

    let mut sync_fut = pa.sync_to_advertising_reports(peer_id, adv_sid, config);
    assert!(fasync::TestExecutor::poll_until_stalled(&mut sync_fut).await.is_pending());

    let sync_server_end = match central_requests.next().await {
        Some(Ok(fidl_le::CentralRequest::SyncToPeriodicAdvertising { payload, .. })) => {
            payload.sync.unwrap()
        }
        request => panic!("Expected SyncToPeriodicAdvertising request, got {request:?}"),
    };

    let mut sync_request_stream = sync_server_end.into_stream();
    let control_handle = sync_request_stream.control_handle();

    // Establish
    control_handle
        .send_on_established(&fidl_le::PeriodicAdvertisingSyncOnEstablishedRequest {
            id: Some(fidl_le::PeriodicAdvertisingSyncId { value: 100 }),
            peer_id: Some(to_fidl_peer_id(&peer_id)),
            advertising_sid: Some(adv_sid),
            phy: Some(fidl_le::PhysicalLayer::Le1M),
            periodic_advertising_interval: Some(100),
            ..Default::default()
        })
        .expect("send on_established ok");

    let mut sync_stream = match fasync::TestExecutor::poll_until_stalled(&mut sync_fut).await {
        Poll::Ready(Ok(stream)) => stream,
        Poll::Ready(Err(e)) => panic!("Expected Ready(Ok(stream)), got Ready(Err({e:?}))"),
        Poll::Pending => panic!("Expected Ready(Ok(stream)), got Pending"),
    };

    let watch_responder = match sync_request_stream.next().await {
        Some(Ok(fidl_le::PeriodicAdvertisingSyncRequest::WatchAdvertisingReport { responder })) => {
            responder
        }
        request => panic!("Expected WatchAdvertisingReport request, got {request:?}"),
    };

    // Mock reports batch:
    // 1. Valid PeriodicAdvertisingReport
    // 2. BroadcastIsochronousGroupInfoReport with info = None (empty)
    // 3. Valid BroadcastIsochronousGroupInfoReport with info = Some(...)
    let report1 = fidl_le::PeriodicAdvertisingReport {
        rssi: Some(-50),
        data: Some(fidl_le::ScanData { tx_power: Some(-20), ..Default::default() }),
        event_counter: Some(1),
        timestamp: Some(111),
        ..Default::default()
    };
    let report2 = fidl_le::BroadcastIsochronousGroupInfoReport {
        info: None, // EMPTY INFO! Should be filtered out.
        timestamp: Some(222),
        ..Default::default()
    };
    let report3 = fidl_le::BroadcastIsochronousGroupInfoReport {
        info: Some(fidl_le::BroadcastIsochronousGroupInfo {
            streams_count: Some(3),
            max_sdu_size: Some(100),
            phy: Some(fidl_le::PhysicalLayer::Le2M),
            encryption: Some(false),
            ..Default::default()
        }),
        timestamp: Some(333),
        ..Default::default()
    };

    watch_responder
        .send(&fidl_le::PeriodicAdvertisingSyncWatchAdvertisingReportResponse {
            reports: Some(vec![
                fidl_le::SyncReport::PeriodicAdvertisingReport(report1),
                fidl_le::SyncReport::BroadcastIsochronousGroupInfoReport(report2),
                fidl_le::SyncReport::BroadcastIsochronousGroupInfoReport(report3),
            ]),
            ..Default::default()
        })
        .expect("send reports ok");

    // Poll stream for 1st report (should be report1)
    let mut next_report_fut = sync_stream.next();
    let report_res1 = match fasync::TestExecutor::poll_until_stalled(&mut next_report_fut).await {
        Poll::Ready(Some(res)) => res,
        res => panic!("Expected Ready(Some(res)), got {res:?}"),
    };
    let gatt_report1 = report_res1.unwrap();
    let bt_gatt::periodic_advertising::SyncReport::PeriodicAdvertisingReport(r1) = gatt_report1
    else {
        panic!("Expected PeriodicAdvertisingReport, got {gatt_report1:?}");
    };
    assert_eq!(r1.rssi, -50);
    assert_eq!(r1.data.len(), 1);
    assert!(matches!(r1.data[0], AdvertisingDatum::TxPowerLevel(-20)));
    assert_eq!(r1.event_counter, Some(1));
    assert_eq!(r1.timestamp, 111);

    // Poll stream for 2nd report (should be report3, report2 is skipped!)
    let mut next_report_fut = sync_stream.next();
    let report_res2 = match fasync::TestExecutor::poll_until_stalled(&mut next_report_fut).await {
        Poll::Ready(Some(res)) => res,
        res => panic!("Expected Ready(Some(res)), got {res:?}"),
    };
    let gatt_report2 = report_res2.unwrap();
    match gatt_report2 {
        bt_gatt::periodic_advertising::SyncReport::BroadcastIsochronousGroupInfoReport(r) => {
            assert_eq!(r.info.streams_count, 3);
            assert_eq!(r.info.max_sdu_size, 100);
            assert_eq!(r.info.phy, bt_common::core::Phy::Le2m);
            assert_eq!(r.info.encryption, false);
            assert_eq!(r.timestamp, 333);
        }
        r => panic!("Expected BroadcastIsochronousGroupInfoReport, got {r:?}"),
    }

    // Poll stream again. It should be pending (waiting for next WatchAdvertisingReport call).
    let mut next_report_fut = sync_stream.next();
    assert!(fasync::TestExecutor::poll_until_stalled(&mut next_report_fut).await.is_pending());
}
