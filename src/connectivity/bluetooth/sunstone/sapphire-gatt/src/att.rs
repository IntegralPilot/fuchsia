// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::num::NonZeroU16;
use thiserror::Error;
mod bearer;
mod l2cap;
pub mod pdu;

pub mod attribute;
pub mod client;
pub mod database;
pub mod router;
pub mod server;

/// A valid, non-zero ATT Attribute Handle (0x0001 - 0xFFFF).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AttributeHandle(NonZeroU16);

impl AttributeHandle {
    /// Creates a new `AttributeHandle` if the given value is non-zero.
    pub const fn new(value: u16) -> Option<Self> {
        if let Some(nonzero) = NonZeroU16::new(value) { Some(Self(nonzero)) } else { None }
    }

    /// Returns the raw `u16` value of this handle.
    pub const fn value(self) -> u16 {
        self.0.get()
    }

    /// Returns the raw `u16` value of this handle (alias for `.value()`).
    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

/// Error type for invalid handle conversions (e.g. converting 0).
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
#[error("Invalid attribute handle")]
pub struct InvalidAttributeHandle;

impl TryFrom<u16> for AttributeHandle {
    type Error = InvalidAttributeHandle;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(InvalidAttributeHandle)
    }
}

impl From<AttributeHandle> for u16 {
    fn from(handle: AttributeHandle) -> Self {
        handle.value()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::att::attribute::testing::MockAttribute;
    use crate::att::bearer::{AttReceiver, BearerRx, BearerTx, MAX_SUPPORTED_MTU};
    use crate::att::client::{Client, DiscoveredInformation, ServerEventStream};
    use crate::att::database::Database;
    use crate::att::database::testing::MockDb;
    use crate::att::l2cap::L2CapChannelTx;
    use crate::att::l2cap::mock::setup_mock_channel;
    use crate::att::pdu::ExecuteWriteFlags;
    use crate::att::router::{BearerRouter, RouteFilter};
    use crate::att::server::{PrepareQueue, Server, ServerError};
    use core::cmp::min;
    use core::mem::MaybeUninit;
    use sapphire_async::executor::BoundedExecutor;
    use sapphire_async::testing::TestExecutor;
    use sapphire_collections::storage::ArrayStorage;
    use sapphire_common::{PeerId, Uuid};
    use sapphire_emboss::att::{
        AttHandlesInformation, AttInformationData16, AttInformationData128,
    };

    const CLIENT_PREFERRED_MTU: u16 = 512;
    const SERVER_MTU: u16 = 256;
    const SMALL_TEST_MTU: u16 = 23;
    const READ_BLOB_OFFSET: u16 = 10;
    const TEST_ARENA_SIZE: usize = 1024;

    fn new_server<Tx, R, DB>(
        peer_id: PeerId,
        bearer_tx: BearerTx<Tx>,
        bearer_rx: R,
        server_rx_mtu: u16,
        database: DB,
    ) -> Server<Tx, R, DB, ArrayStorage<TEST_ARENA_SIZE>>
    where
        Tx: L2CapChannelTx,
        R: AttReceiver,
        DB: Database,
    {
        let constructor = Server::new;
        constructor(peer_id, bearer_tx, bearer_rx, server_rx_mtu, database, PrepareQueue::new())
    }

    #[test]
    fn test_attribute_handle_new() {
        assert!(AttributeHandle::new(0).is_none());
        assert_eq!(AttributeHandle::new(1).unwrap().value(), 1);
        assert_eq!(AttributeHandle::new(0xFFFF).unwrap().value(), 0xFFFF);
    }

    #[test]
    fn test_attribute_handle_try_from() {
        assert_eq!(AttributeHandle::try_from(0), Err(InvalidAttributeHandle));
        assert_eq!(AttributeHandle::try_from(1).unwrap(), AttributeHandle::new(1).unwrap());
        assert_eq!(
            AttributeHandle::try_from(0xFFFF).unwrap(),
            AttributeHandle::new(0xFFFF).unwrap()
        );
    }

    #[test]
    fn test_attribute_handle_from() {
        let handle = AttributeHandle::new(42).unwrap();
        let value: u16 = u16::from(handle);
        assert_eq!(value, 42);
    }

    #[test]
    fn test_client_server_integration_handshake() {
        let (app_channel, test_tx, test_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                client_router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                MockDb::new(),
            );

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
                assert_eq!(server.mtu(), SERVER_MTU);
            });

            let client_handle = executor.spawn(async move {
                client.exchange_mtu().await.unwrap();
                assert_eq!(client.mtu(), SERVER_MTU);
            });

            executor.run_until_stalled();

            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    fn h(val: u16) -> AttributeHandle {
        AttributeHandle::try_from(val).unwrap()
    }

    #[test]
    fn test_client_server_integration_find_information() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut db = MockDb::new();
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), b"Sunstone"); // handle 1
            let custom_uuid =
                Uuid::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
            let custom_attr = MockAttribute::new(custom_uuid, b"Custom"); // handle 2
            db.insert(h(1), name_attr);
            db.insert(h(2), custom_attr);

            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                client_router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SERVER_MTU,
                db,
            );

            // Server task
            let server_handle = executor.spawn(async move {
                // 1. Process Handshake
                server.handle_request().await.unwrap();
                let negotiated = server.mtu();
                assert_eq!(negotiated, SERVER_MTU);

                // 2. Process Find Information request
                server.handle_request().await.unwrap();

                // 3. Process another request (for handle 2)
                server.handle_request().await.unwrap();
            });

            // Client task
            let client_handle = executor.spawn(async move {
                // 1. Handshake
                client.exchange_mtu().await.unwrap();
                let negotiated = client.mtu();
                assert_eq!(negotiated, SERVER_MTU);

                // 2. Discover descriptors starting from 1 to 2
                let mut rx_buf = [MaybeUninit::uninit(); 256];
                let info1 = client.find_information(h(1), h(2), &mut rx_buf).await.unwrap();
                match info1 {
                    DiscoveredInformation::Uuid16(entries) => {
                        assert_eq!(entries.len(), 1);
                        let e = AttInformationData16::new(entries.get(0).unwrap());
                        assert_eq!(e.attribute_handle().try_read().unwrap(), 1);
                        assert_eq!(e.uuid().try_read().unwrap(), 0x2A00);
                    }
                    _ => panic!("Expected Uuid16 discovered info"),
                }

                // 3. Discover descriptor for handle 2
                let mut rx_buf2 = [MaybeUninit::uninit(); 256];
                let info2 = client.find_information(h(2), h(2), &mut rx_buf2).await.unwrap();
                match info2 {
                    DiscoveredInformation::Uuid128(entries) => {
                        assert_eq!(entries.len(), 1);
                        let e = AttInformationData128::new(entries.get(0).unwrap());
                        assert_eq!(e.attribute_handle().try_read().unwrap(), 2);
                        let uuid_view = e.uuid().unwrap();
                        assert_eq!(uuid_view.element_count(), 16);
                        for i in 0..16 {
                            assert_eq!(
                                uuid_view.get(i).unwrap().try_read().unwrap(),
                                (i + 1) as u8
                            );
                        }
                    }
                    _ => panic!("Expected Uuid128 discovered info"),
                }
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_server_integration_find_by_type_value() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut db = MockDb::new();
            // Group 1: handles 1 to 5. Type 0x2800 (Primary Service), value 0x180D (Heart Rate Service)
            let svc1 = MockAttribute::new_grouped(Uuid::from_u16(0x2800), &[0x0D, 0x18], 5);
            let name = MockAttribute::new(Uuid::from_u16(0x2A00), b"Sunstone");
            // Group 2: handles 6 to 10. Type 0x2800, value 0x180A (Device Info Service)
            let svc2 = MockAttribute::new_grouped(Uuid::from_u16(0x2800), &[0x0A, 0x18], 10);

            db.insert(h(1), svc1);
            db.insert(h(2), name);
            db.insert(h(6), svc2);

            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                client_router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SERVER_MTU,
                db,
            );

            // Server task
            let server_handle = executor.spawn(async move {
                // 1. Handshake
                server.handle_request().await.unwrap();
                // 2. Find by Type Value (Group 1 query)
                server.handle_request().await.unwrap();
                // 3. Find by Type Value (Group 2 query)
                server.handle_request().await.unwrap();
            });

            // Client task
            let client_handle = executor.spawn(async move {
                client.exchange_mtu().await.unwrap();

                let mut rx_buf = [MaybeUninit::uninit(); 256];
                let results1 = client
                    .find_by_type_value(h(1), h(10), 0x2800, &[0x0D, 0x18], &mut rx_buf)
                    .await
                    .unwrap();
                assert_eq!(results1.len(), 1);
                let e1 = AttHandlesInformation::new(results1.get(0).unwrap());
                assert_eq!(e1.attribute_handle().try_read().unwrap(), 1);
                assert_eq!(e1.group_end_handle().try_read().unwrap(), 5);

                let mut rx_buf2 = [MaybeUninit::uninit(); 256];
                let results2 = client
                    .find_by_type_value(h(1), h(10), 0x2800, &[0x0A, 0x18], &mut rx_buf2)
                    .await
                    .unwrap();
                assert_eq!(results2.len(), 1);
                let e2 = AttHandlesInformation::new(results2.get(0).unwrap());
                assert_eq!(e2.attribute_handle().try_read().unwrap(), 6);
                assert_eq!(e2.group_end_handle().try_read().unwrap(), 10);
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_server_integration_read() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut db = MockDb::new();
            let long_val = b"012345678901234567890123456789"; // 30 bytes
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), long_val);
            db.insert(h(1), name_attr);

            // Server MTU is SMALL_TEST_MTU bytes (so max read response is SMALL_TEST_MTU - 1 bytes)
            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SMALL_TEST_MTU,
                db,
            );

            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                client_router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Server task
            let server_handle = executor.spawn(async move {
                // 1. MTU Exchange
                server.handle_request().await.unwrap();
                // 2. Read Request
                server.handle_request().await.unwrap();
            });

            // Client task
            let client_handle = executor.spawn(async move {
                // 1. MTU Exchange (negotiated MTU will be SMALL_TEST_MTU, since server MTU is SMALL_TEST_MTU)
                client.exchange_mtu().await.unwrap();
                assert_eq!(client.mtu(), SMALL_TEST_MTU);

                // 2. Read Request
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let val = client.read(h(1), &mut rx_buf).await.unwrap();
                // Max read response is SMALL_TEST_MTU - 1 bytes
                assert_eq!(val, &long_val[..(SMALL_TEST_MTU - 1) as usize]);
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_server_integration_read_entire_value() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut db = MockDb::new();
            let short_val = b"Sunstone"; // 8 bytes
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), short_val);
            db.insert(h(1), name_attr);

            // Server MTU is SMALL_TEST_MTU bytes (so max read response is SMALL_TEST_MTU - 1 bytes)
            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SMALL_TEST_MTU,
                db,
            );

            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                client_router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Server task
            let server_handle = executor.spawn(async move {
                // 1. MTU Exchange
                server.handle_request().await.unwrap();
                // 2. Read Request
                server.handle_request().await.unwrap();
            });

            // Client task
            let client_handle = executor.spawn(async move {
                // 1. MTU Exchange (negotiated MTU will be SMALL_TEST_MTU, since server MTU is SMALL_TEST_MTU)
                client.exchange_mtu().await.unwrap();
                assert_eq!(client.mtu(), SMALL_TEST_MTU);

                // 2. Read Request
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let val = client.read(h(1), &mut rx_buf).await.unwrap();

                // Value is smaller than SMALL_TEST_MTU - 1, so it is read entirely without truncation
                assert!(short_val.len() < (SMALL_TEST_MTU - 1) as usize);
                assert_eq!(val, short_val);
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_server_integration_read_blob() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut db = MockDb::new();
            let long_val = b"012345678901234567890123456789"; // 30 bytes
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), long_val);
            db.insert(h(1), name_attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SMALL_TEST_MTU,
                db,
            );

            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                client_router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Server task
            let server_handle = executor.spawn(async move {
                // 1. MTU Exchange
                server.handle_request().await.unwrap();
                // 2. Read Blob Request
                server.handle_request().await.unwrap();
            });

            // Client task
            let client_handle = executor.spawn(async move {
                // 1. MTU Exchange
                client.exchange_mtu().await.unwrap();

                // 2. Read Blob Request starting at offset
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let val = client.read_blob(h(1), READ_BLOB_OFFSET, &mut rx_buf).await.unwrap();
                // Remaining bytes fits in MTU - 1
                assert_eq!(val, &long_val[READ_BLOB_OFFSET as usize..]);
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_server_integration_read_blob_truncated() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut db = MockDb::new();
            let long_val = b"0123456789012345678901234567890123456789"; // 40 bytes
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), long_val);
            db.insert(h(1), name_attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SMALL_TEST_MTU,
                db,
            );

            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                client_router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Server task
            let server_handle = executor.spawn(async move {
                // 1. MTU Exchange
                server.handle_request().await.unwrap();
                // 2. Read Blob Request
                server.handle_request().await.unwrap();
            });

            // Client task
            let client_handle = executor.spawn(async move {
                // 1. MTU Exchange (negotiates MTU of 23)
                client.exchange_mtu().await.unwrap();
                assert_eq!(client.mtu(), SMALL_TEST_MTU);

                // 2. Read Blob Request starting at offset
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let val = client.read_blob(h(1), READ_BLOB_OFFSET, &mut rx_buf).await.unwrap();
                // Remaining bytes is truncated to MTU - 1
                let expected_len = (SMALL_TEST_MTU - 1) as usize;
                assert_eq!(val.len(), expected_len);
                assert_eq!(
                    val,
                    &long_val[READ_BLOB_OFFSET as usize..READ_BLOB_OFFSET as usize + expected_len]
                );
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_server_integration_read_by_type() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut db = MockDb::new();
            db.insert(h(2), MockAttribute::new(Uuid::from_u16(0x2A00), b"Sunstone"));
            db.insert(h(4), MockAttribute::new(Uuid::from_u16(0x2A00), b"Sapphire"));
            db.insert(h(6), MockAttribute::new(Uuid::from_u16(0x2A01), b"Other")); // different UUID
            db.insert(h(8), MockAttribute::new(Uuid::from_u16(0x2A00), b"Blue")); // different value size!

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SERVER_MTU,
                db,
            );

            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                client_router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Server task
            let server_handle = executor.spawn(async move {
                // 1. MTU Exchange
                server.handle_request().await.unwrap();
                // 2. Read By Type Request
                server.handle_request().await.unwrap();
            });

            // Client task
            let client_handle = executor.spawn(async move {
                // 1. MTU Exchange
                client.exchange_mtu().await.unwrap();

                // 2. Read By Type Request
                let mut rx_buf = [MaybeUninit::uninit(); CLIENT_PREFERRED_MTU as usize];
                let uuid = Uuid::from_u16(0x2A00);
                let results = client.read_by_type(h(1), h(10), &uuid, &mut rx_buf).await.unwrap();

                let mut iter = results.iter();
                let (h1, val1) = iter.next().unwrap();
                assert_eq!(h1, h(2));
                assert_eq!(val1, b"Sunstone");

                let (h2, val2) = iter.next().unwrap();
                assert_eq!(h2, h(4));
                assert_eq!(val2, b"Sapphire");

                assert!(iter.next().is_none());
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_server_integration_read_by_group_type() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut db = MockDb::new();
            // Primary service declarations (grouping type 0x2800)
            db.insert(h(1), MockAttribute::new_grouped(Uuid::from_u16(0x2800), b"\x01\x18", 5)); // Service 0x1801 (Generic Attribute), ends at handle 5
            db.insert(h(6), MockAttribute::new_grouped(Uuid::from_u16(0x2800), b"\x00\x18", 10)); // Service 0x1800 (Generic Access), ends at handle 10
            db.insert(h(11), MockAttribute::new(Uuid::from_u16(0x2A00), b"Device Name")); // Non-grouped attribute

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SERVER_MTU,
                db,
            );

            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                client_router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Server task
            let server_handle = executor.spawn(async move {
                // 1. MTU Exchange
                server.handle_request().await.unwrap();
                // 2. Read By Group Type Request
                server.handle_request().await.unwrap();
            });

            // Client task
            let client_handle = executor.spawn(async move {
                // 1. MTU Exchange
                client.exchange_mtu().await.unwrap();

                // 2. Read By Group Type Request
                let mut rx_buf = [MaybeUninit::uninit(); CLIENT_PREFERRED_MTU as usize];
                let group_uuid = Uuid::from_u16(0x2800);
                let results =
                    client.read_by_group_type(h(1), h(20), &group_uuid, &mut rx_buf).await.unwrap();

                let mut iter = results.iter();
                let (e1, val1) = iter.next().unwrap();
                assert_eq!(e1.attribute_handle().try_read().unwrap(), 1);
                assert_eq!(e1.end_group_handle().try_read().unwrap(), 5);
                assert_eq!(val1, b"\x01\x18");

                let (e2, val2) = iter.next().unwrap();
                assert_eq!(e2.attribute_handle().try_read().unwrap(), 6);
                assert_eq!(e2.end_group_handle().try_read().unwrap(), 10);
                assert_eq!(val2, b"\x00\x18");

                assert!(iter.next().is_none());
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_server_integration_write() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut db = MockDb::new();
            db.insert(h(10), MockAttribute::new(Uuid::from_u16(0x2A00), b"InitialValue"));

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SERVER_MTU,
                db,
            );

            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                client_router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Server task
            let server_handle = executor.spawn(async move {
                // 1. MTU Exchange
                server.handle_request().await.unwrap();
                // 2. Write Request
                server.handle_request().await.unwrap();
                // 3. Read Request to verify value
                server.handle_request().await.unwrap();
            });

            // Client task
            let client_handle = executor.spawn(async move {
                // 1. MTU Exchange
                client.exchange_mtu().await.unwrap();

                // 2. Write Request
                let mut rx_buf = [MaybeUninit::uninit(); CLIENT_PREFERRED_MTU as usize];
                client.write(h(10), b"Sunstone", &mut rx_buf).await.unwrap();

                // 3. Read Request to verify the written value
                let read_res = client.read(h(10), &mut rx_buf).await.unwrap();
                assert_eq!(read_res, b"Sunstone");
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_server_integration_write_command() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut db = MockDb::new();
            db.insert(h(10), MockAttribute::new(Uuid::from_u16(0x2A00), b"InitialValue"));

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SERVER_MTU,
                db,
            );

            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                client_router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Server task
            let server_handle = executor.spawn(async move {
                // 1. MTU Exchange
                server.handle_request().await.unwrap();
                // 2. Write Command
                server.handle_request().await.unwrap();
                // 3. Read Request
                server.handle_request().await.unwrap();
            });

            // Client task
            let client_handle = executor.spawn(async move {
                // 1. MTU Exchange
                client.exchange_mtu().await.unwrap();

                // 2. Write Command
                client.write_command(h(10), b"SunstoneCmd").await.unwrap();

                // 3. Read Request to verify the written value
                let mut rx_buf = [MaybeUninit::uninit(); CLIENT_PREFERRED_MTU as usize];
                let read_res = client.read(h(10), &mut rx_buf).await.unwrap();
                assert_eq!(read_res, b"SunstoneCmd");
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_server_integration_execute_write_commit() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut db = MockDb::new();
            db.insert(h(10), MockAttribute::new(Uuid::from_u16(0x2A00), b"InitialValue"));

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SERVER_MTU,
                db,
            );

            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                client_router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Server task
            let server_handle = executor.spawn(async move {
                // 1. MTU Exchange
                server.handle_request().await.unwrap();
                // 2. First Prepare Write
                server.handle_request().await.unwrap();
                // 3. Second Prepare Write
                server.handle_request().await.unwrap();
                // 4. Execute Write (Commit)
                server.handle_request().await.unwrap();
                // 5. Read Request to verify value
                server.handle_request().await.unwrap();
            });

            // Client task
            let client_handle = executor.spawn(async move {
                // 1. MTU Exchange
                client.exchange_mtu().await.unwrap();

                // 2. Prepare Write (Part 1)
                let mut rx_buf = [MaybeUninit::uninit(); CLIENT_PREFERRED_MTU as usize];
                client.prepare_write(h(10), 0, b"Hello", &mut rx_buf).await.unwrap();

                // 3. Prepare Write (Part 2)
                client.prepare_write(h(10), 5, b"World", &mut rx_buf).await.unwrap();

                // 4. Execute Write (Commit)
                client.execute_write(ExecuteWriteFlags::WRITE, &mut rx_buf).await.unwrap();

                // 5. Verify the updated attribute value
                let read_res = client.read(h(10), &mut rx_buf).await.unwrap();
                assert_eq!(read_res, b"HelloWorld");
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_server_integration_execute_write_cancel() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut db = MockDb::new();
            db.insert(h(10), MockAttribute::new(Uuid::from_u16(0x2A00), b"InitialValue"));

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SERVER_MTU,
                db,
            );

            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                client_router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let server_handle = executor.spawn(async move {
                server.handle_request().await.unwrap(); // MTU Exchange
                server.handle_request().await.unwrap(); // Prepare Write
                server.handle_request().await.unwrap(); // Execute Write (Cancel)
                server.handle_request().await.unwrap(); // Read Request
            });

            let client_handle = executor.spawn(async move {
                client.exchange_mtu().await.unwrap();

                let mut rx_buf = [MaybeUninit::uninit(); CLIENT_PREFERRED_MTU as usize];
                client.prepare_write(h(10), 0, b"Part1", &mut rx_buf).await.unwrap();
                client.execute_write(ExecuteWriteFlags::CANCEL, &mut rx_buf).await.unwrap();

                let read_res = client.read(h(10), &mut rx_buf).await.unwrap();
                assert_eq!(read_res, b"InitialValue"); // Value is unmodified!
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_server_integration_with_router() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut db = MockDb::new();
            let long_val = b"Sunstone Router Integration Test Value";
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), long_val);
            let handle_1 = h(1);
            db.insert(handle_1, name_attr);

            let client_app_tx_bearer = BearerTx::new(app_channel.sender);
            let server_tx_bearer = BearerTx::new(server_tx);

            let client_rx_handle = client_router.route_to(RouteFilter::Responses).unwrap();
            let mut event_stream =
                ServerEventStream::new(&client_router, client_app_tx_bearer.clone())
                    .expect("event stream created");

            let mut client =
                Client::new(client_app_tx_bearer, client_rx_handle, CLIENT_PREFERRED_MTU);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                server_tx_bearer,
                BearerRx::new(server_rx),
                SMALL_TEST_MTU,
                db,
            );

            let client_proc_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); MAX_SUPPORTED_MTU];
                let result = client.read(handle_1, &mut rx_buf).await.unwrap();
                let expected_len = (SMALL_TEST_MTU - 1) as usize;
                assert_eq!(result, &long_val[..min(long_val.len(), expected_len)]);
            });

            let notification_listener_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); MAX_SUPPORTED_MTU];
                let event = event_stream.next(&mut rx_buf).await.unwrap();
                assert_eq!(event.handle().get(), 0x1234);
                assert_eq!(event.value(), b"some value data");
            });

            let mut notifier = server.notifier();
            let server_proc_handle = executor.spawn(async move {
                let _ = server.handle_request().await;
                notifier.notify(0x1234, b"some value data").await.unwrap();
            });

            executor.run_until_stalled();

            assert!(client_proc_handle.is_finished());
            assert!(notification_listener_handle.is_finished());
            assert!(server_proc_handle.is_finished());
        });
    }

    #[test]
    fn test_client_server_integration_indication() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let client_router = BearerRouter::<_>::new(app_channel.receiver);
        let server_router = BearerRouter::<_>::new(server_rx);

        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let client_app_tx_bearer = BearerTx::new(app_channel.sender);

            let server_tx_bearer = BearerTx::new(server_tx);

            let client_rx_handle = client_router.route_to(RouteFilter::Responses).unwrap();
            let mut event_stream =
                ServerEventStream::new(&client_router, client_app_tx_bearer.clone())
                    .expect("event stream created");

            let _client = Client::new(client_app_tx_bearer, client_rx_handle, CLIENT_PREFERRED_MTU);

            let server = new_server(
                PeerId::new(1).unwrap(),
                server_tx_bearer,
                server_router.route_to(RouteFilter::Requests).unwrap(),
                SMALL_TEST_MTU,
                MockDb::new(),
            );

            let indication_listener_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); MAX_SUPPORTED_MTU];
                let event = event_stream.next(&mut rx_buf).await.unwrap();
                assert_eq!(event.handle().get(), 0x7777);
                assert_eq!(event.value(), b"indication payload");
            });

            let mut indicator =
                server.indicator(server_router.route_to(RouteFilter::Confirmations).unwrap());
            let server_proc_handle = executor.spawn(async move {
                indicator.indicate(0x7777, b"indication payload").await.unwrap();
            });

            executor.run_until_stalled();

            assert!(indication_listener_handle.is_finished());
            assert!(server_proc_handle.is_finished());
        });
    }
    mod proptests {
        use super::*;
        use crate::att::attribute::Attribute;
        use crate::att::attribute::testing::MockAttribute;
        use crate::att::bearer::MAX_ATTRIBUTE_SIZE;
        use crate::att::client::{ClientError, DiscoveredInformation};
        use crate::att::database::Database;
        use crate::att::pdu::{
            ATT_FIND_INFORMATION_REQ_SIZE, ATT_PREPARE_WRITE_HEADER_SIZE, ErrorCode, Opcode,
            Packet, UuidFormat, uuid_to_format,
        };
        use core::mem::MaybeUninit;
        use proptest::prelude::*;
        use sapphire_common::{PeerId, Uuid};
        use sapphire_emboss::att::{
            AttErrorRsp, AttFindInformationReqMut, AttHandlesInformation, AttInformationData16,
            AttInformationData128,
        };
        use zerocopy::{IntoBytes, TryFromBytes};

        fn setup_db() -> MockDb {
            let mut db = MockDb::new();
            // Handle 1: UUID 16-bit
            db.insert(h(1), MockAttribute::new(Uuid::from_u16(0x2A00), b"Value1"));
            // Handle 2: UUID 128-bit
            let custom_uuid =
                Uuid::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
            db.insert(h(2), MockAttribute::new(custom_uuid, b"Value2"));
            // Handle 10: UUID 16-bit
            db.insert(h(10), MockAttribute::new(Uuid::from_u16(0x2A01), b"Value10"));
            // Handle 11: UUID 16-bit
            db.insert(h(11), MockAttribute::new(Uuid::from_u16(0x2A02), b"Value11"));
            // Handle 20: UUID 128-bit
            let custom_uuid2 = Uuid::from_le_bytes([
                10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
            ]);
            db.insert(h(20), MockAttribute::new(custom_uuid2, b"Value20"));
            db
        }

        fn setup_group_db() -> MockDb {
            let mut db = MockDb::new();
            // Primary service declarations (grouping type 0x2800)
            db.insert(h(1), MockAttribute::new_grouped(Uuid::from_u16(0x2800), b"\x01\x18", 5));
            db.insert(h(6), MockAttribute::new_grouped(Uuid::from_u16(0x2800), b"\x00\x18", 10));
            // Non-grouped attribute
            db.insert(h(11), MockAttribute::new(Uuid::from_u16(0x2A00), b"Device Name"));
            db
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(100))]

            #[test]
            fn test_find_information_invalid_handles_zero(
                handle in 1..=0xFFFFu16,
            ) {
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        SERVER_MTU,
                        setup_db(),
                    );
                    let server_handle = executor.spawn(async move {
                        let _ = server.run().await;
                    });
                    let client_handle = executor.spawn(async move {
                        let mut rx_buf = [MaybeUninit::uninit(); 512];
                        let mut client_tx_bearer = BearerTx::new(app_channel.sender);
                        let mut client_rx_bearer = client_router.route_to(RouteFilter::Responses).unwrap();

                        // Test starting handle = 0
                        let mut req_buf = [0u8; ATT_FIND_INFORMATION_REQ_SIZE];
                        let mut view = AttFindInformationReqMut::new(&mut req_buf[..]);
                        view.attribute_opcode().try_write(Opcode::ATT_FIND_INFORMATION_REQ).unwrap();
                        view.starting_handle().try_write(0).unwrap();
                        view.ending_handle().try_write(handle).unwrap();
                        let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                        client_tx_bearer.send(tx_packet).await.unwrap();
                        let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                        assert_eq!(packet.opcode, Opcode::ATT_ERROR_RSP.into());
                        let err = AttErrorRsp::new(packet.as_bytes());
                        assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::INVALID_HANDLE);

                        let mut rx_buf2 = [MaybeUninit::uninit(); 512];
                        // Test ending handle = 0
                        let mut req_buf2 = [0u8; ATT_FIND_INFORMATION_REQ_SIZE];
                        let mut view2 = AttFindInformationReqMut::new(&mut req_buf2[..]);
                        view2.attribute_opcode().try_write(Opcode::ATT_FIND_INFORMATION_REQ).unwrap();
                        view2.starting_handle().try_write(handle).unwrap();
                        view2.ending_handle().try_write(0).unwrap();
                        let tx_packet2 = Packet::try_ref_from_bytes(&req_buf2[..]).unwrap();
                        client_tx_bearer.send(tx_packet2).await.unwrap();
                        let packet2 = client_rx_bearer.next_packet(&mut rx_buf2).await.unwrap();
                        assert_eq!(packet2.opcode, Opcode::ATT_ERROR_RSP.into());
                        let err2 = AttErrorRsp::new(packet2.as_bytes());
                        assert_eq!(err2.error_code().try_read().unwrap(), ErrorCode::INVALID_HANDLE);
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_find_information_end_handle_smaller_than_start_handle(
                (start, end) in (2..=0xFFFFu16).prop_flat_map(|s| {
                    (Just(AttributeHandle::new(s).unwrap()), (1..s).prop_map(|e| AttributeHandle::new(e).unwrap()))
                })
            ) {
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut client = Client::new(BearerTx::new(app_channel.sender), client_router.route_to(RouteFilter::Responses).unwrap(), CLIENT_PREFERRED_MTU);
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        SERVER_MTU,
                        setup_db(),
                    );
                    let server_handle = executor.spawn(async move {
                        let _ = server.run().await;
                    });
                    let client_handle = executor.spawn(async move {
                        let mut rx_buf = [MaybeUninit::uninit(); 512];
                        let result = client.find_information(start, end, &mut rx_buf).await;
                        assert_eq!(result, Err(ClientError::ErrorResponse(ErrorCode::INVALID_HANDLE)));
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_find_information_response_consistency(
                start in (1..=0xFFFFu16).prop_map(|v| AttributeHandle::new(v).unwrap()),
                end in (1..=0xFFFFu16).prop_map(|v| AttributeHandle::new(v).unwrap()),
            ) {
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut client = Client::new(BearerTx::new(app_channel.sender), client_router.route_to(RouteFilter::Responses).unwrap(), CLIENT_PREFERRED_MTU);
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        SERVER_MTU,
                        setup_db(),
                    );
                    let server_handle = executor.spawn(async move {
                        let _ = server.run().await;
                    });
                    let client_handle = executor.spawn(async move {
                        let mut rx_buf1 = [MaybeUninit::uninit(); 512];
                        let result1 = client.find_information(start, end, &mut rx_buf1).await;

                        let mut rx_buf2 = [MaybeUninit::uninit(); 512];
                        let result2 = client.find_information(start, end, &mut rx_buf2).await;

                        assert_eq!(result1, result2);
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_find_information_valid_range_contents(
                (start, end) in (1..=0xFFFFu16).prop_flat_map(|s| {
                    (Just(AttributeHandle::new(s).unwrap()), (s..=0xFFFFu16).prop_map(|e| AttributeHandle::new(e).unwrap()))
                })
            ) {
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut client = Client::new(BearerTx::new(app_channel.sender), client_router.route_to(RouteFilter::Responses).unwrap(), CLIENT_PREFERRED_MTU);
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        SERVER_MTU,
                        setup_db(),
                    );
                    let server_handle = executor.spawn(async move {
                        let _ = server.run().await;
                    });
                    let db = setup_db();
                    let client_handle = executor.spawn(async move {
                        let mut rx_buf = [MaybeUninit::uninit(); 512];
                        let result = client.find_information(start, end, &mut rx_buf).await;

                        match result {
                            Ok(DiscoveredInformation::Uuid16(entries)) => {
                                assert_ne!(entries.len(), 0);
                                for chunk in entries.iter() {
                                    let entry = AttInformationData16::new(chunk);
                                    let h = entry.attribute_handle().try_read().unwrap();
                                    assert!(h >= start.value() && h <= end.value());
                                    let handle = AttributeHandle::try_from(h).unwrap();
                                    let attr = db.find_attribute(handle).expect("attribute must exist in db");
                                    assert_eq!(uuid_to_format(attr.uuid()), UuidFormat::BIT16);
                                }
                            }
                            Ok(DiscoveredInformation::Uuid128(entries)) => {
                                assert_ne!(entries.len(), 0);
                                for chunk in entries.iter() {
                                    let entry = AttInformationData128::new(chunk);
                                    let h = entry.attribute_handle().try_read().unwrap();
                                    assert!(h >= start.value() && h <= end.value());
                                    let handle = AttributeHandle::try_from(h).unwrap();
                                    let attr = db.find_attribute(handle).expect("attribute must exist in db");
                                    assert_eq!(uuid_to_format(attr.uuid()), UuidFormat::BIT128);
                                }
                            }
                            Err(ClientError::ErrorResponse(ErrorCode::ATTRIBUTE_NOT_FOUND)) => {
                                assert!(!db.has_attributes_in_range(start.value(), end.value()));
                            }
                            other => panic!("Unexpected result: {:?}", other),
                        }
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_find_by_type_value_invalid_ranges(
                (start, end) in (2..=0xFFFFu16).prop_flat_map(|s| {
                    (Just(AttributeHandle::new(s).unwrap()), (1..s).prop_map(|e| AttributeHandle::new(e).unwrap()))
                }),
                target_type in 0..=0xFFFFu16,
                target_value in prop::collection::vec(any::<u8>(), 0..20),
            ) {
                let target_value: Vec<u8> = target_value;
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut client = Client::new(BearerTx::new(app_channel.sender), client_router.route_to(RouteFilter::Responses).unwrap(), CLIENT_PREFERRED_MTU);
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        SERVER_MTU,
                        setup_db(),
                    );
                    let server_handle = executor.spawn(async move {
                        let _ = server.run().await;
                    });
                    let client_handle = executor.spawn(async move {
                        client.exchange_mtu().await.unwrap();
                        let mut rx_buf = [MaybeUninit::uninit(); 512];
                        let result = client.find_by_type_value(start, end, target_type, &target_value, &mut rx_buf).await;
                        assert_eq!(result, Err(ClientError::ErrorResponse(ErrorCode::INVALID_HANDLE)));
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_find_by_type_value_response_consistency(
                start in (1..=0xFFFFu16).prop_map(|v| AttributeHandle::new(v).unwrap()),
                end in (1..=0xFFFFu16).prop_map(|v| AttributeHandle::new(v).unwrap()),
                target_type in 0..=0xFFFFu16,
                target_value in prop::collection::vec(any::<u8>(), 0..20),
            ) {
                let target_value: Vec<u8> = target_value;
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut client = Client::new(BearerTx::new(app_channel.sender), client_router.route_to(RouteFilter::Responses).unwrap(), CLIENT_PREFERRED_MTU);
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        SERVER_MTU,
                        setup_db(),
                    );
                    let server_handle = executor.spawn(async move {
                        let _ = server.run().await;
                    });
                    let client_handle = executor.spawn(async move {
                        client.exchange_mtu().await.unwrap();
                        let mut rx_buf1 = [MaybeUninit::uninit(); 512];
                        let result1 = client.find_by_type_value(start, end, target_type, &target_value, &mut rx_buf1).await;

                        let mut rx_buf2 = [MaybeUninit::uninit(); 512];
                        let result2 = client.find_by_type_value(start, end, target_type, &target_value, &mut rx_buf2).await;

                        assert_eq!(result1, result2);
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_find_by_type_value_valid_range(
                (start, end) in (1..=0xFFFFu16).prop_flat_map(|s| {
                    (Just(AttributeHandle::new(s).unwrap()), (s..=0xFFFFu16).prop_map(|e| AttributeHandle::new(e).unwrap()))
                }),
                use_existing in proptest::bool::weighted(0.5),
                random_type in 0..=0xFFFFu16,
                random_value in prop::collection::vec(any::<u8>(), 0..20),
            ) {
                let random_value: Vec<u8> = random_value;
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut client = Client::new(BearerTx::new(app_channel.sender), client_router.route_to(RouteFilter::Responses).unwrap(), CLIENT_PREFERRED_MTU);
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        SERVER_MTU,
                        setup_db(),
                    );
                    let server_handle = executor.spawn(async move {
                        let _ = server.run().await;
                    });

                    let (target_type, target_value) = if use_existing {
                        (0x2A01u16, b"Value10".to_vec())
                    } else {
                        (random_type, random_value)
                    };

                    let db = setup_db();
                    let client_handle = executor.spawn(async move {
                        client.exchange_mtu().await.unwrap();
                        let mut rx_buf = [MaybeUninit::uninit(); 512];
                        let result = client
                            .find_by_type_value(start, end, target_type, &target_value, &mut rx_buf)
                            .await;

                        match result {
                            Ok(entries) => {
                                assert!(!entries.is_empty());
                                for chunk in entries.iter() {
                                    let entry = AttHandlesInformation::new(chunk);
                                    let h = entry.attribute_handle().try_read().unwrap();
                                    let group_end = entry.group_end_handle().try_read().unwrap();
                                    assert!(h >= start.value() && h <= end.value());
                                    assert!(group_end >= h && group_end <= end.value());

                                    let handle = AttributeHandle::try_from(h).unwrap();
                                    let attr = db.find_attribute(handle).expect("attribute must exist in db");
                                    let matches_type = if let Ok(bytes16) = <[u8; 2]>::try_from(*attr.uuid()) {
                                        u16::from_le_bytes(bytes16) == target_type
                                    } else {
                                        false
                                    };
                                    assert!(matches_type);
                                }
                            }
                            Err(ClientError::ErrorResponse(ErrorCode::ATTRIBUTE_NOT_FOUND)) => {
                                // Verify that no attributes in the range match the type and value.
                                // Handle 1: type 0x2A00, value b"Value1"
                                // Handle 10: type 0x2A01, value b"Value10"
                                // Handle 11: type 0x2A02, value b"Value11"
                                let matches_1 = 1 >= start.value() && 1 <= end.value() && target_type == 0x2A00 && target_value == b"Value1";
                                let matches_10 = 10 >= start.value() && 10 <= end.value() && target_type == 0x2A01 && target_value == b"Value10";
                                let matches_11 = 11 >= start.value() && 11 <= end.value() && target_type == 0x2A02 && target_value == b"Value11";
                                assert!(!(matches_1 || matches_10 || matches_11));
                            }
                            other => panic!("Unexpected result: {:?}", other),
                        }
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_read_by_type_invalid_ranges(
                (start, end) in (2..=0xFFFFu16).prop_flat_map(|s| {
                    (Just(AttributeHandle::new(s).unwrap()), (1..s).prop_map(|e| AttributeHandle::new(e).unwrap()))
                }),
                random_uuid_16 in 0..=0xFFFFu16,
            ) {
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut client = Client::new(BearerTx::new(app_channel.sender), client_router.route_to(RouteFilter::Responses).unwrap(), CLIENT_PREFERRED_MTU);
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        SERVER_MTU,
                        setup_db(),
                    );
                    let server_handle = executor.spawn(async move {
                        let _ = server.run().await;
                    });
                    let client_handle = executor.spawn(async move {
                        client.exchange_mtu().await.unwrap();
                        let mut rx_buf = [MaybeUninit::uninit(); 512];
                        let uuid = Uuid::from_u16(random_uuid_16);
                        let result = client.read_by_type(start, end, &uuid, &mut rx_buf).await;
                        assert_eq!(result.err(), Some(ClientError::ErrorResponse(ErrorCode::INVALID_HANDLE)));
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_read_by_type_response_consistency(
                start in (1..=0xFFFFu16).prop_map(|v| AttributeHandle::new(v).unwrap()),
                end in (1..=0xFFFFu16).prop_map(|v| AttributeHandle::new(v).unwrap()),
                random_uuid_16 in 0..=0xFFFFu16,
            ) {
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut client = Client::new(BearerTx::new(app_channel.sender), client_router.route_to(RouteFilter::Responses).unwrap(), CLIENT_PREFERRED_MTU);
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        SERVER_MTU,
                        setup_db(),
                    );
                    let server_handle = executor.spawn(async move {
                        let _ = server.run().await;
                    });
                    let client_handle = executor.spawn(async move {
                        client.exchange_mtu().await.unwrap();
                        let uuid = Uuid::from_u16(random_uuid_16);
                        let mut rx_buf1 = [MaybeUninit::uninit(); 512];
                        let result1 = client.read_by_type(start, end, &uuid, &mut rx_buf1).await;

                        let mut rx_buf2 = [MaybeUninit::uninit(); 512];
                        let result2 = client.read_by_type(start, end, &uuid, &mut rx_buf2).await;

                        let r1 = result1.as_ref().map(|res| res.iter().collect::<Vec<_>>());
                        let r2 = result2.as_ref().map(|res| res.iter().collect::<Vec<_>>());
                        assert_eq!(r1, r2);
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_read_by_type_valid_range(
                (start, end) in (1..=0xFFFFu16).prop_flat_map(|s| {
                    (Just(AttributeHandle::new(s).unwrap()), (s..=0xFFFFu16).prop_map(|e| AttributeHandle::new(e).unwrap()))
                }),
                use_existing in proptest::bool::weighted(0.5),
                random_uuid_16 in 0..=0xFFFFu16,
            ) {
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut client = Client::new(BearerTx::new(app_channel.sender), client_router.route_to(RouteFilter::Responses).unwrap(), CLIENT_PREFERRED_MTU);
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        SERVER_MTU,
                        setup_db(),
                    );
                    let server_handle = executor.spawn(async move {
                        let _ = server.run().await;
                    });

                    let target_uuid = if use_existing {
                        Uuid::from_u16(0x2A01)
                    } else {
                        Uuid::from_u16(random_uuid_16)
                    };

                    let db = setup_db();
                    let client_handle = executor.spawn(async move {
                        client.exchange_mtu().await.unwrap();
                        let mut rx_buf = [MaybeUninit::uninit(); 512];
                        let result = client
                            .read_by_type(start, end, &target_uuid, &mut rx_buf)
                            .await;

                        match result {
                            Ok(results) => {
                                let mut count = 0;
                                for (handle, value) in results.iter() {
                                    count += 1;
                                    let h = handle.value();
                                    assert!(h >= start.value() && h <= end.value());

                                    let attr = db.find_attribute(handle).expect("attribute must exist in db");
                                    assert_eq!(attr.uuid(), &target_uuid);

                                    // Verify value matches DB
                                    let mut db_val = [0u8; 64];
                                    let db_val_len = attr.read_chunk(PeerId::new(1).unwrap(), 0, &mut db_val).await.unwrap();
                                    assert_eq!(value, &db_val[..db_val_len]);
                                }
                                assert!(count > 0);
                            }
                            Err(ClientError::ErrorResponse(ErrorCode::ATTRIBUTE_NOT_FOUND)) => {
                                // Verify that indeed no attributes in range [start, end] match target_uuid
                                for h_val in start.value()..=end.value() {
                                    if let Some(handle) = AttributeHandle::new(h_val) {
                                        if let Some(attr) = db.find_attribute(handle) {
                                            assert_ne!(attr.uuid(), &target_uuid);
                                        }
                                    }
                                }
                            }
                            other => panic!("Unexpected result: {:?}", other),
                        }
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_read_by_group_type_invalid_ranges(
                (start, end) in (2..=0xFFFFu16).prop_flat_map(|s| {
                    (Just(AttributeHandle::new(s).unwrap()), (1..s).prop_map(|e| AttributeHandle::new(e).unwrap()))
                }),
                random_uuid_16 in 0..=0xFFFFu16,
            ) {
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut client = Client::new(BearerTx::new(app_channel.sender), client_router.route_to(RouteFilter::Responses).unwrap(), CLIENT_PREFERRED_MTU);
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        SERVER_MTU,
                        setup_group_db(),
                    );
                    let server_handle = executor.spawn(async move {
                        let _ = server.run().await;
                    });
                    let client_handle = executor.spawn(async move {
                        client.exchange_mtu().await.unwrap();
                        let mut rx_buf = [MaybeUninit::uninit(); 512];
                        let uuid = Uuid::from_u16(random_uuid_16);
                        let result = client.read_by_group_type(start, end, &uuid, &mut rx_buf).await;
                        assert_eq!(result.err(), Some(ClientError::ErrorResponse(ErrorCode::INVALID_HANDLE)));
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_read_by_group_type_response_consistency(
                start in (1..=0xFFFFu16).prop_map(|v| AttributeHandle::new(v).unwrap()),
                end in (1..=0xFFFFu16).prop_map(|v| AttributeHandle::new(v).unwrap()),
                random_uuid_16 in 0..=0xFFFFu16,
            ) {
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut client = Client::new(BearerTx::new(app_channel.sender), client_router.route_to(RouteFilter::Responses).unwrap(), CLIENT_PREFERRED_MTU);
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        SERVER_MTU,
                        setup_group_db(),
                    );
                    let server_handle = executor.spawn(async move {
                        let _ = server.run().await;
                    });
                    let client_handle = executor.spawn(async move {
                        client.exchange_mtu().await.unwrap();
                        let uuid = Uuid::from_u16(random_uuid_16);
                        let mut rx_buf1 = [MaybeUninit::uninit(); 512];
                        let result1 = client.read_by_group_type(start, end, &uuid, &mut rx_buf1).await;

                        let mut rx_buf2 = [MaybeUninit::uninit(); 512];
                        let result2 = client.read_by_group_type(start, end, &uuid, &mut rx_buf2).await;

                        let r1 = result1.map(|res| res.iter().map(|(e, val)| {
                            (e.attribute_handle().try_read().unwrap(), e.end_group_handle().try_read().unwrap(), val.to_vec())
                        }).collect::<Vec<_>>());
                        let r2 = result2.map(|res| res.iter().map(|(e, val)| {
                            (e.attribute_handle().try_read().unwrap(), e.end_group_handle().try_read().unwrap(), val.to_vec())
                        }).collect::<Vec<_>>());
                        assert_eq!(r1, r2);
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_read_by_group_type_valid_range(
                (start, end) in (1..=0xFFFFu16).prop_flat_map(|s| {
                    (Just(AttributeHandle::new(s).unwrap()), (s..=0xFFFFu16).prop_map(|e| AttributeHandle::new(e).unwrap()))
                }),
                use_existing in proptest::bool::weighted(0.5),
                random_uuid_16 in 0..=0xFFFFu16,
            ) {
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut client = Client::new(BearerTx::new(app_channel.sender), client_router.route_to(RouteFilter::Responses).unwrap(), CLIENT_PREFERRED_MTU);
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        SERVER_MTU,
                        setup_group_db(),
                    );
                    let server_handle = executor.spawn(async move {
                        let _ = server.run().await;
                    });

                    let target_uuid = if use_existing {
                        Uuid::from_u16(0x2800) // Primary Service UUID
                    } else {
                        Uuid::from_u16(random_uuid_16)
                    };

                    let db = setup_group_db();
                    let client_handle = executor.spawn(async move {
                        client.exchange_mtu().await.unwrap();
                        let mut rx_buf = [MaybeUninit::uninit(); 512];
                        let result = client
                            .read_by_group_type(start, end, &target_uuid, &mut rx_buf)
                            .await;

                        match result {
                            Ok(results) => {
                                let mut count = 0;
                                for (entry, value) in results.iter() {
                                    count += 1;
                                    let h = entry.attribute_handle().try_read().unwrap();
                                    let end_h = entry.end_group_handle().try_read().unwrap();
                                    assert!(h >= start.value() && h <= end.value());

                                    let handle = AttributeHandle::try_from(h).unwrap();
                                    let attr = db.find_attribute(handle).expect("attribute must exist in db");
                                    assert_eq!(attr.uuid(), &target_uuid);
                                    assert_eq!(end_h, attr.group_end_handle().unwrap());

                                    let mut db_val = [0u8; 64];
                                    let db_val_len = attr.read_chunk(PeerId::new(1).unwrap(), 0, &mut db_val).await.unwrap();
                                    assert_eq!(value, &db_val[..db_val_len]);
                                }
                                assert!(count > 0);
                            }
                            Err(ClientError::ErrorResponse(ErrorCode::ATTRIBUTE_NOT_FOUND)) => {
                                for h_val in start.value()..=end.value() {
                                    if let Some(handle) = AttributeHandle::new(h_val) {
                                        if let Some(attr) = db.find_attribute(handle) {
                                            assert_ne!(attr.uuid(), &target_uuid);
                                        }
                                    }
                                }
                            }
                            Err(ClientError::ErrorResponse(ErrorCode::UNSUPPORTED_GROUP_TYPE)) => {
                                let mut has_non_grouping = false;
                                for h_val in start.value()..=end.value() {
                                    if let Some(handle) = AttributeHandle::new(h_val) {
                                        if let Some(attr) = db.find_attribute(handle) {
                                            if attr.uuid() == &target_uuid && attr.group_end_handle().is_none() {
                                                has_non_grouping = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                                assert!(has_non_grouping);
                            }
                            other => panic!("Unexpected result: {:?}", other),
                        }
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_prepare_write_consistency(
                handle_raw in 1..=25u16,
                offset in 0..=600u16,
                data in prop::collection::vec(any::<u8>(), 0..60),
                mtu in 23..=256u16,
            ) {
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        mtu,
                        setup_db(),
                    );
                    let mut client = Client::new(
                        BearerTx::new(app_channel.sender),
                        client_router.route_to(RouteFilter::Responses).unwrap(),
                        mtu,
                    );

                    let max_payload = (mtu as usize) - ATT_PREPARE_WRITE_HEADER_SIZE;
                    let test_data = if data.len() > max_payload { &data[..max_payload] } else { &data[..] };

                    let server_handle = executor.spawn(async move {
                        server.handle_request().await.unwrap(); // MTU Exchange
                        let _ = server.handle_request().await;
                    });

                    let client_handle = executor.spawn(async move {
                        client.exchange_mtu().await.unwrap();
                        let mut rx_buf = [MaybeUninit::uninit(); 512];
                        let handle = h(handle_raw);
                        let result = client.prepare_write(handle, offset, test_data, &mut rx_buf).await;

                        let db = setup_db();
                        let is_valid_handle = db.find_attribute(handle).is_some();
                        let initial_len = if is_valid_handle {
                            let attr = db.find_attribute(handle).expect("attribute must exist");
                            let mut temp_buf = [0u8; 512];
                            attr.read_chunk(PeerId::new(1).expect("valid PeerId"), 0, &mut temp_buf).await.expect("read must succeed")
                        } else {
                            0
                        };

                        match result {
                            Ok(()) => {
                                assert!(is_valid_handle);
                                assert!(offset as usize <= initial_len);
                                assert!(offset as usize + test_data.len() <= MAX_ATTRIBUTE_SIZE);
                            }
                            Err(ClientError::ErrorResponse(ErrorCode::INVALID_HANDLE)) => {
                                assert!(!is_valid_handle);
                            }
                            Err(ClientError::ErrorResponse(ErrorCode::INVALID_OFFSET)) => {
                                assert!(is_valid_handle);
                                assert!(offset as usize > initial_len);
                            }
                            Err(ClientError::ErrorResponse(ErrorCode::INVALID_ATTRIBUTE_VALUE_LENGTH)) => {
                                assert!(is_valid_handle);
                                assert!(offset as usize + test_data.len() > MAX_ATTRIBUTE_SIZE);
                            }
                            other => panic!("Unexpected result: {:?}", other),
                        }
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }

            #[test]
            fn test_execute_write_consistency(
                h1_raw in prop::sample::select(vec![1u16, 10u16]),
                h2_raw in prop::sample::select(vec![1u16, 10u16]),
                offset1 in 0..=20u16,
                offset2 in 0..=20u16,
                data1 in prop::collection::vec(any::<u8>(), 0..30),
                data2 in prop::collection::vec(any::<u8>(), 0..30),
                flags in prop::sample::select(vec![ExecuteWriteFlags::CANCEL, ExecuteWriteFlags::WRITE]),
                mtu in 23..=256u16,
            ) {
                let (app_channel, server_tx, server_rx) = setup_mock_channel();
                let client_router = BearerRouter::<_>::new(app_channel.receiver);
                BoundedExecutor::new(TestExecutor::new(), |executor| {
                    let mut server = new_server(
                        PeerId::new(1).unwrap(),
                        BearerTx::new(server_tx),
                        BearerRx::new(server_rx),
                        mtu,
                        setup_db(),
                    );
                    let mut client = Client::new(
                        BearerTx::new(app_channel.sender),
                        client_router.route_to(RouteFilter::Responses).unwrap(),
                        mtu,
                    );

                    // Truncate payloads to fit within negotiated MTU to prevent client panics.
                    let max_payload = (mtu as usize) - ATT_PREPARE_WRITE_HEADER_SIZE;
                    let test_data1 = if data1.len() > max_payload { &data1[..max_payload] } else { &data1[..] };
                    let test_data2 = if data2.len() > max_payload { &data2[..max_payload] } else { &data2[..] };

                    // Run the server request loop continuously until the client drops.
                    let server_handle = executor.spawn(async move {
                        loop {
                            if server.handle_request().await.is_err() {
                                break;
                            }
                        }
                    });

                    // Maintain expected database state.
                    let mut val1_expected = b"Value1".to_vec();
                    let mut val10_expected = b"Value10".to_vec();

                    let client_handle = executor.spawn(async move {
                        client.exchange_mtu().await.expect("MTU exchange failed");
                        let mut rx_buf = [MaybeUninit::uninit(); 512];

                        let h1 = h(h1_raw);
                        let h2 = h(h2_raw);

                        // Send Prepare Write 1.
                        let r1 = client.prepare_write(h1, offset1, test_data1, &mut rx_buf).await;
                        let p1_ok = r1.is_ok();

                        // Send Prepare Write 2 only if the first request was accepted.
                        let mut p2_ok = false;
                        if p1_ok {
                            let r2 = client.prepare_write(h2, offset2, test_data2, &mut rx_buf).await;
                            p2_ok = r2.is_ok();
                        }

                        // If both prepares succeeded, trigger and verify Execute Write.
                        if p1_ok && p2_ok {
                            let exec_res = client.execute_write(flags, &mut rx_buf).await;
                            match flags {
                                ExecuteWriteFlags::CANCEL => {
                                    // CANCEL always succeeds and leaves values unchanged.
                                    assert!(exec_res.is_ok());
                                }
                                ExecuteWriteFlags::WRITE => {
                                    let mut val1 = val1_expected.clone();
                                    let mut val10 = val10_expected.clone();

                                    // Simulate Write 1. Should succeed since prepare succeeded.
                                    let w1_ok = if h1_raw == 1 {
                                        offset1 as usize <= val1.len()
                                    } else {
                                        offset1 as usize <= val10.len()
                                    };

                                    if w1_ok {
                                        if h1_raw == 1 {
                                            val1.truncate(offset1 as usize);
                                            val1.extend_from_slice(test_data1);
                                        } else {
                                            val10.truncate(offset1 as usize);
                                            val10.extend_from_slice(test_data1);
                                        }

                                        // Simulate Write 2 against the database state modified by Write 1.
                                        let w2_ok = if h2_raw == 1 {
                                            offset2 as usize <= val1.len()
                                        } else {
                                            offset2 as usize <= val10.len()
                                        };

                                        if w2_ok {
                                            // Both writes succeed during execution.
                                            if h2_raw == 1 {
                                                val1.truncate(offset2 as usize);
                                                val1.extend_from_slice(test_data2);
                                            } else {
                                                val10.truncate(offset2 as usize);
                                                val10.extend_from_slice(test_data2);
                                            }
                                            assert!(exec_res.is_ok());
                                            val1_expected = val1;
                                            val10_expected = val10;
                                        } else {
                                            // Write 2 fails due to truncation from Write 1.
                                            assert_eq!(
                                                exec_res.expect_err("expected execution failure"),
                                                ClientError::ErrorResponse(ErrorCode::INVALID_OFFSET)
                                            );
                                            val1_expected = val1;
                                            val10_expected = val10;
                                        }
                                    } else {
                                        panic!("Write 1 failed during execution but prepare succeeded!");
                                    }
                                }
                            }
                        }

                        // Read back values to verify database matches expected state.
                        let val1_read = client.read(h(1), &mut rx_buf).await.expect("read h(1) failed");
                        let max_read_len = (mtu - 1) as usize;
                        assert_eq!(val1_read, &val1_expected[..min(val1_expected.len(), max_read_len)]);

                        let val10_read = client.read(h(10), &mut rx_buf).await.expect("read h(10) failed");
                        assert_eq!(val10_read, &val10_expected[..min(val10_expected.len(), max_read_len)]);
                    });

                    executor.run_until_stalled();
                    assert!(client_handle.is_finished());
                    assert!(server_handle.is_finished());
                });
            }
        }
    }
}
