// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::task::Poll;

use assert_matches::assert_matches;
use futures::{Future, FutureExt, StreamExt};

use bt_common::{PeerId, Uuid};

use crate::central::{AdvertisingDatum, PeerName, ScanResult};
use crate::server::{ServiceDefinition, ServiceEvent};
use crate::test_utils::*;
use crate::types::*;
use crate::{central::Filter, client::PeerService, server, Central};

const TEST_UUID_1: Uuid = Uuid::from_u16(0x1234);
const TEST_UUID_2: Uuid = Uuid::from_u16(0x2345);
const TEST_UUID_3: Uuid = Uuid::from_u16(0x3456);

// Sets up a fake peer service with some characteristics.
fn setup_peer_service() -> FakePeerService {
    let mut fake_peer_service = FakePeerService::new();
    fake_peer_service.add_characteristic(
        Characteristic {
            handle: Handle(1),
            uuid: TEST_UUID_1,
            properties: CharacteristicProperties(vec![
                CharacteristicProperty::Broadcast,
                CharacteristicProperty::Notify,
            ]),
            permissions: AttributePermissions::default(),
            descriptors: vec![],
        },
        vec![],
    );
    fake_peer_service.add_characteristic(
        Characteristic {
            handle: Handle(2),
            uuid: TEST_UUID_1,
            properties: CharacteristicProperties(vec![
                CharacteristicProperty::Broadcast,
                CharacteristicProperty::Notify,
            ]),
            permissions: AttributePermissions::default(),
            descriptors: vec![],
        },
        vec![],
    );
    fake_peer_service.add_characteristic(
        Characteristic {
            handle: Handle(3),
            uuid: TEST_UUID_1,
            properties: CharacteristicProperties(vec![
                CharacteristicProperty::Broadcast,
                CharacteristicProperty::Notify,
            ]),
            permissions: AttributePermissions::default(),
            descriptors: vec![],
        },
        vec![],
    );
    fake_peer_service.add_characteristic(
        Characteristic {
            handle: Handle(4),
            uuid: TEST_UUID_2,
            properties: CharacteristicProperties(vec![CharacteristicProperty::Notify]),
            permissions: AttributePermissions::default(),
            descriptors: vec![],
        },
        vec![],
    );
    fake_peer_service.add_characteristic(
        Characteristic {
            handle: Handle(5),
            uuid: TEST_UUID_3,
            properties: CharacteristicProperties(vec![CharacteristicProperty::Broadcast]),
            permissions: AttributePermissions::default(),
            descriptors: vec![],
        },
        vec![],
    );
    fake_peer_service
}

#[test]
fn peer_service_discover_characteristics_works() {
    let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());

    let fake_peer_service = setup_peer_service();

    let mut discover_results = fake_peer_service.discover_characteristics(Some(TEST_UUID_1));
    let polled = discover_results.poll_unpin(&mut noop_cx);
    assert_matches!(polled, Poll::Ready(Ok(chars)) => { assert_eq!(chars.len(), 3) });

    let mut discover_results = fake_peer_service.discover_characteristics(Some(TEST_UUID_2));
    let polled = discover_results.poll_unpin(&mut noop_cx);
    assert_matches!(polled, Poll::Ready(Ok(chars)) => { assert_eq!(chars.len(), 1) });

    let mut discover_results =
        fake_peer_service.discover_characteristics(Some(Uuid::from_u16(0xFF22)));
    let polled = discover_results.poll_unpin(&mut noop_cx);
    assert_matches!(polled, Poll::Ready(Ok(chars)) => { assert_eq!(chars.len(), 0) });

    let mut discover_results = fake_peer_service.discover_characteristics(None);
    let polled = discover_results.poll_unpin(&mut noop_cx);
    assert_matches!(polled, Poll::Ready(Ok(chars)) => { assert_eq!(chars.len(), 5) });
}

#[test]
fn peer_service_read_characteristic() {
    let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());

    let mut fake_peer_service = setup_peer_service();
    let mut buf = vec![0; 255];

    // For characteristic that was added, value is returned
    let mut read_result = fake_peer_service.read_characteristic(&Handle(0x1), 0, &mut buf[..]);
    let polled: Poll<std::prelude::v1::Result<(usize, bool), Error>> =
        read_result.poll_unpin(&mut noop_cx);
    assert_matches!(polled, Poll::Ready(Ok((len, _))) => { assert_eq!(len, 0) });

    // For characteristic that doesn't exist, fails.
    let mut read_result = fake_peer_service.read_characteristic(&Handle(0xF), 0, &mut buf[..]);
    let polled = read_result.poll_unpin(&mut noop_cx);
    assert_matches!(polled, Poll::Ready(Err(_)));

    // Change the value for characteristic with handle 1.
    fake_peer_service.add_characteristic(
        Characteristic {
            handle: Handle(1),
            uuid: TEST_UUID_1,
            properties: CharacteristicProperties(vec![
                CharacteristicProperty::Broadcast,
                CharacteristicProperty::Notify,
            ]),
            permissions: AttributePermissions::default(),
            descriptors: vec![],
        },
        vec![0, 1, 2, 3],
    );

    // Successfully reads the updated value.
    let mut read_result = fake_peer_service.read_characteristic(&Handle(0x1), 0, &mut buf[..]);
    let polled = read_result.poll_unpin(&mut noop_cx);
    assert_matches!(polled, Poll::Ready(Ok((len, _))) => {
        assert_eq!(len, 4);
        assert_eq!(buf[..len], vec![0,1,2,3]);
    });
}

#[test]
fn peer_service_write_characteristic() {
    let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());

    let mut fake_peer_service: FakePeerService = setup_peer_service();

    // Set expected characteristic value before calling `write_characteristic`.
    fake_peer_service.expect_characteristic_value(&Handle(1), vec![0, 1, 2, 3]);
    let mut write_result = fake_peer_service.write_characteristic(
        &Handle(0x1),
        WriteMode::WithoutResponse,
        0,
        vec![0, 1, 2, 3].as_slice(),
    );

    match write_result.poll_unpin(&mut noop_cx) {
        Poll::Ready(Ok(())) => {}
        _ => panic!("expected write to succeed"),
    }
}

#[test]
#[should_panic]
fn peer_service_write_characteristic_fail() {
    let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());

    let fake_peer_service = setup_peer_service();

    // Write some random value without calling `expect_characteristic_value` first.
    let mut write_result = fake_peer_service.write_characteristic(
        &Handle(0x1),
        WriteMode::WithoutResponse,
        0,
        vec![13, 14, 15, 16].as_slice(),
    );

    match write_result.poll_unpin(&mut noop_cx) {
        Poll::Ready(Err(_)) => {}
        _ => panic!("expected write to fail"),
    }
}

#[test]
fn peer_service_subscribe() {
    let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());

    let mut fake_peer_service = setup_peer_service();
    let mut notification_stream = fake_peer_service.subscribe(&Handle(0x1));

    // Stream is empty unless we add an item through the FakeNotificationStream
    // struct.
    assert!(notification_stream.poll_next_unpin(&mut noop_cx).is_pending());

    // Update the characteristic value so that notification is sent.
    fake_peer_service.add_characteristic(
        Characteristic {
            handle: Handle(1),
            uuid: TEST_UUID_1,
            properties: CharacteristicProperties(vec![
                CharacteristicProperty::Broadcast,
                CharacteristicProperty::Notify,
            ]),
            permissions: AttributePermissions::default(),
            descriptors: vec![],
        },
        vec![0, 1, 2, 3],
    );

    // Stream should be ready.
    let polled = notification_stream.poll_next_unpin(&mut noop_cx);
    assert_matches!(polled, Poll::Ready(Some(Ok(notification))) => {
        assert_eq!(notification.handle, Handle(0x1));
        assert_eq!(notification.value, vec![0,1,2,3]);
        assert_eq!(notification.maybe_truncated, false);
    });

    assert!(notification_stream.poll_next_unpin(&mut noop_cx).is_pending());
}

#[test]
fn central_search_works() {
    let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
    let central = FakeCentral::new();

    let mut scan_results = central.scan(&[Filter::ServiceUuid(Uuid::from_u16(0x1844)).into()]);
    let polled = scan_results.poll_next_unpin(&mut noop_cx);
    assert_matches!(polled, Poll::Pending);

    let scanned_result = ScanResult {
        id: PeerId(1),
        connectable: true,
        name: PeerName::CompleteName("Marie's Pixel 7 Pro".to_owned()),
        advertised: vec![AdvertisingDatum::Services(vec![Uuid::from_u16(0x1844)])],
        advertising_sid: Some(0),
        periodic_advertising_interval: None,
    };
    let controller = scan_results.controller();
    let _ = controller.add_scanned_result(Ok(scanned_result));

    let polled = scan_results.poll_next_unpin(&mut noop_cx);
    assert_matches!(polled, Poll::Ready(Some(Ok(_))));
}

fn boxed_generic_usage<T: crate::GattTypes>(central: Box<dyn crate::Central<T>>) {
    use crate::client::PeerServiceHandle;

    let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
    let _stream = central.scan(&[]);

    let connect_fut = central.connect(PeerId(1));
    futures::pin_mut!(connect_fut);
    let Poll::Ready(Ok(client)) = connect_fut.poll(&mut noop_cx) else {
        panic!("Connect should be ready Ok");
    };

    let client_boxed: Box<dyn crate::client::Client<T>> = Box::new(client);

    let find_serv_fut = client_boxed.find_service(Uuid::from_u16(0));

    futures::pin_mut!(find_serv_fut);
    let Poll::Ready(Ok(services)) = find_serv_fut.poll(&mut noop_cx) else {
        panic!("Expected services future to resolve");
    };

    assert_eq!(services.len(), 1);

    let connect_service_fut = services[0].connect();

    futures::pin_mut!(connect_service_fut);
    let Poll::Ready(Ok(service)) = connect_service_fut.poll(&mut noop_cx) else {
        panic!("Expected service to connect");
    };

    let _service_box: Box<dyn crate::client::PeerService<T>> = Box::new(service);
}

#[test]
fn central_dynamic_usage() {
    let mut central = FakeCentral::new();
    let mut fake_client = FakeClient::new();
    fake_client.add_service(Uuid::from_u16(0), true, FakePeerService::new());
    central.add_client(PeerId(1), fake_client);

    let boxed: Box<dyn crate::Central<FakeTypes>> = Box::new(central);

    boxed_generic_usage(boxed);
}

/// Generate an example service definition with a variety of things in it.
fn example_service_definition() -> ServiceDefinition {
    let mut def =
        ServiceDefinition::new(server::ServiceId::new(1), TEST_UUID_1, ServiceKind::Primary);
    // Only readable
    def.add_characteristic(Characteristic {
        handle: Handle(1),
        uuid: TEST_UUID_2,
        properties: CharacteristicProperty::Read.into(),
        permissions: AttributePermissions {
            read: Some(SecurityLevels::default()),
            write: None,
            update: None,
        },
        descriptors: vec![],
    })
    .unwrap();
    // Only writable
    def.add_characteristic(Characteristic {
        handle: Handle(2),
        uuid: TEST_UUID_3,
        properties: CharacteristicProperty::Write.into(),
        permissions: AttributePermissions {
            read: None,
            write: Some(SecurityLevels::default()),
            update: None,
        },
        descriptors: vec![],
    })
    .unwrap();
    // Readable, writable, and notifiable
    def.add_characteristic(Characteristic {
        handle: Handle(3),
        uuid: TEST_UUID_3,
        properties: CharacteristicProperty::Read
            | CharacteristicProperty::Write
            | CharacteristicProperty::Notify,
        permissions: AttributePermissions {
            read: Some(SecurityLevels::default()),
            write: Some(SecurityLevels::default()),
            update: Some(SecurityLevels::default()),
        },
        descriptors: vec![],
    })
    .unwrap();
    // Only Readable and Indicatable
    def.add_characteristic(Characteristic {
        handle: Handle(4),
        uuid: TEST_UUID_3,
        properties: CharacteristicProperty::Read | CharacteristicProperty::Indicate,
        permissions: AttributePermissions {
            read: Some(SecurityLevels::default()),
            write: None,
            update: Some(SecurityLevels::default()),
        },
        descriptors: vec![],
    })
    .unwrap();
    def
}

#[test]
fn fake_server_usable() {
    use server::{LocalService, Server, ServiceId};
    let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());

    let (server, mut events) = FakeServer::new();
    let mut local_service_fut = server.prepare(example_service_definition());

    let Poll::Ready(Ok(local_service)) = local_service_fut.poll_unpin(&mut noop_cx) else {
        panic!("Expected local service to prepare fine");
    };

    let mut service_event_stream = local_service.publish();

    // Published shouldn't be emitted until the service is present on the server.
    let Poll::Ready(Some(FakeServerEvent::Published { id, definition })) =
        events.poll_next_unpin(&mut noop_cx)
    else {
        panic!("Expected to generate an event from publishing");
    };

    assert_eq!(server::ServiceId::new(1), id);
    assert_eq!(definition.characteristics().count(), 4);

    server.incoming_read(PeerId(1), ServiceId::new(1), Handle(1), 0);

    let Poll::Ready(Some(Ok(ServiceEvent::Read { peer_id, handle, offset: _, responder }))) =
        service_event_stream.poll_next_unpin(&mut noop_cx)
    else {
        panic!("Expected read on the service stream");
    };

    assert_eq!(PeerId(1), peer_id);
    assert_eq!(Handle(1), handle);

    use server::ReadResponder;
    responder.respond(&[0, 1, 2, 3, 4, 5]);

    let Poll::Ready(Some(FakeServerEvent::ReadResponded { service_id, handle, value })) =
        events.poll_next_unpin(&mut noop_cx)
    else {
        panic!("Expected a read response");
    };

    assert_eq!(ServiceId::new(1), service_id);
    assert_eq!(Handle(1), handle);
    assert_eq!(&[0, 1, 2, 3, 4, 5], value.expect("should be ok").as_slice());
}
