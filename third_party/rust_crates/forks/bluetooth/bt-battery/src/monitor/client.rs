// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::packet_encoding::Decodable;
use bt_gatt::client::{CharacteristicNotification, PeerService, ServiceCharacteristic};
use bt_gatt::types::{CharacteristicProperty, Error as GattLibraryError, Handle};
use bt_gatt::GattTypes;
use futures::stream::{BoxStream, FusedStream, SelectAll, Stream, StreamExt};
use std::task::Poll;

use crate::error::{Error, ServiceError};
use crate::types::{BatteryLevel, BATTERY_LEVEL_UUID, READ_CHARACTERISTIC_BUFFER_SIZE};

/// Represents the termination status of a Stream.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
enum TerminatedState {
    #[default]
    NotTerminated,
    /// Stream termination is in progress. We have returned an Err, but not a
    /// None yet.
    Terminating,
    /// We have returned a None and therefore the stream is terminated. It
    /// should not be polled again.
    Terminated,
}

/// A stream of GATT notifications received from the peer's battery service
/// characteristics. This stream must be polled in order to receive updates
/// about the battery service.
pub struct BatteryMonitorEventStream {
    /// Notification streams received from the characteristics.
    notification_streams:
        SelectAll<BoxStream<'static, Result<CharacteristicNotification, GattLibraryError>>>,
    /// The current termination status of the stream.
    /// `TerminatedState::Terminated` when the stream is exhausted and
    /// should not be polled thereafter.
    terminated: TerminatedState,
}

impl BatteryMonitorEventStream {
    fn new(
        notification_streams: SelectAll<
            BoxStream<'static, Result<CharacteristicNotification, GattLibraryError>>,
        >,
    ) -> Self {
        Self { notification_streams, terminated: TerminatedState::default() }
    }
}

impl Stream for BatteryMonitorEventStream {
    // TODO(b/335259516): Update return type to accommodate other characteristics.
    type Item = Result<BatteryLevel, Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        match &self.terminated {
            TerminatedState::NotTerminated => {}
            TerminatedState::Terminating => {
                self.terminated = TerminatedState::Terminated;
                return Poll::Ready(None);
            }
            TerminatedState::Terminated => {
                panic!("polling terminated stream");
            }
        }

        let result = futures::ready!(self.notification_streams.poll_next_unpin(cx));
        match result {
            Some(Ok(notification)) => {
                let battery_level_result = BatteryLevel::decode(&notification.value[..]).0?;
                Poll::Ready(Some(Ok(battery_level_result)))
            }
            Some(Err(e)) => {
                // GATT Errors are not fatal and will be relayed to the stream.
                // All other errors are considered fatal and will result in stream termination.
                if !matches!(e, GattLibraryError::Gatt(_)) {
                    self.terminated = TerminatedState::Terminating;
                }
                Poll::Ready(Some(Err(e.into())))
            }
            None => {
                self.terminated = TerminatedState::Terminating;
                Poll::Ready(Some(Err(Error::Service(ServiceError::NotificationStreamClosed))))
            }
        }
    }
}

impl FusedStream for BatteryMonitorEventStream {
    fn is_terminated(&self) -> bool {
        self.terminated == TerminatedState::Terminated
    }
}

/// Implements the Battery Service client role.
/// Reads the relevant characteristics on a GATT connection to a remote peer's
/// Battery Service and provides a mechanism for subscribing to and receiving
/// notifications on the service.
pub struct BatteryMonitorClient<T: GattTypes> {
    /// Represents the underlying GATT LE connection. Kept alive to maintain the
    /// connection to the peer.
    _client: T::Client,
    /// GATT client interface for interacting with the peer's battery service.
    #[allow(unused)]
    gatt_client: T::PeerService,
    /// GATT Handles associated with the peer's one or more Battery Level
    /// characteristics. The first `Handle` in this list is expected to be
    /// the "primary" one.
    // TODO(b/335259516): Save Handles for optional characteristics that are discovered.
    #[allow(unused)]
    battery_level_handles: Vec<Handle>,
    /// The current battery level reported by the peer's battery server.
    #[allow(unused)]
    battery_level: BatteryLevel,
    /// Collection of streams containing notifications from the peer's battery
    /// characteristics.
    notification_streams:
        Option<SelectAll<BoxStream<'static, Result<CharacteristicNotification, GattLibraryError>>>>,
}

impl<T: GattTypes> BatteryMonitorClient<T> {
    pub(crate) async fn create(
        _client: T::Client,
        gatt_client: T::PeerService,
    ) -> Result<Self, Error>
    where
        <T as bt_gatt::GattTypes>::NotificationStream: std::marker::Send,
    {
        // All battery services must contain at least one battery level characteristic.
        let battery_level_characteristics =
            ServiceCharacteristic::<T>::find(&gatt_client, BATTERY_LEVEL_UUID).await?;

        if battery_level_characteristics.is_empty() {
            return Err(Error::Service(ServiceError::MissingCharacteristic));
        }
        // It is valid to have multiple Battery Level Characteristics. If multiple
        // exist, the primary (main) characteristic has a Description field of
        // "main". For now, we assume the first such characteristic is the
        // primary. See BAS 1.1 Section 3.1.2.1.
        // TODO(b/335246946): Check for Characteristic Presentation Format descriptor if
        // multiple characteristics are present. Use this to infer the "primary".
        let primary_battery_level_characteristic =
            battery_level_characteristics.first().expect("nonempty");
        let battery_level_handles: Vec<Handle> =
            battery_level_characteristics.iter().map(|c| *c.handle()).collect();
        // Get the current battery level of the primary Battery Level characteristic.
        let (battery_level, _decoded_bytes) = {
            let mut buf = vec![0; READ_CHARACTERISTIC_BUFFER_SIZE];
            let read_bytes = primary_battery_level_characteristic.read(&mut buf[..]).await?;
            BatteryLevel::decode(&buf[0..read_bytes])
        };
        let battery_level = battery_level?;

        // Subscribe to notifications on the battery level characteristic if it
        // is supported.
        let mut notification_streams = None;
        if primary_battery_level_characteristic
            .characteristic()
            .supports_property(&CharacteristicProperty::Notify)
        {
            let mut streams = SelectAll::new();
            streams
                .push(gatt_client.subscribe(primary_battery_level_characteristic.handle()).boxed());
            notification_streams = Some(streams);
        }
        // TODO(b/335259516): Subscribe to notifications from optional characteristics
        // if they are present.

        Ok(Self {
            _client,
            gatt_client,
            battery_level_handles,
            battery_level,
            notification_streams,
        })
    }

    /// Returns a stream of battery events that represent notifications on the
    /// remote peer's Battery Service.
    /// The returned Stream _must_ be polled in order to receive the relevant
    /// notification and indications.
    /// This method should only be called once. Subsequent calls will return
    /// None. Returns Some<T> if a stream of notifications is available,
    /// None otherwise.
    pub fn take_event_stream(&mut self) -> Option<BatteryMonitorEventStream> {
        self.notification_streams.take().map(|s| BatteryMonitorEventStream::new(s))
    }

    #[cfg(test)]
    fn battery_level(&self) -> BatteryLevel {
        self.battery_level
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use bt_common::packet_encoding::Error as PacketError;
    use bt_common::Uuid;
    use bt_gatt::test_utils::{FakeClient, FakePeerService, FakeTypes};
    use bt_gatt::types::{
        AttributePermissions, Characteristic, CharacteristicProperties, GattError,
    };
    use futures::{pin_mut, FutureExt};

    pub(crate) const BATTERY_LEVEL_HANDLE: Handle = Handle(0x1);
    pub(crate) fn fake_battery_service(battery_level: u8) -> FakePeerService {
        let mut peer_service = FakePeerService::new();
        peer_service.add_characteristic(
            Characteristic {
                handle: BATTERY_LEVEL_HANDLE,
                uuid: BATTERY_LEVEL_UUID,
                properties: CharacteristicProperties(vec![
                    CharacteristicProperty::Read,
                    CharacteristicProperty::Notify,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![battery_level],
        );
        peer_service
    }

    /// Builds a `BatteryMonitorClient` that is connected to a fake GATT service
    /// with a Battery Service.
    fn setup_client(battery_level: u8) -> (BatteryMonitorClient<FakeTypes>, FakePeerService) {
        // Constructs a FakePeerService with a battery level characteristic.
        let fake_peer_service = fake_battery_service(battery_level);
        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let create_result =
            BatteryMonitorClient::<FakeTypes>::create(FakeClient::new(), fake_peer_service.clone());
        pin_mut!(create_result);
        let polled = create_result.poll_unpin(&mut noop_cx);
        let Poll::Ready(Ok(client)) = polled else {
            panic!("Expected BatteryMonitorClient to be successfully created");
        };

        (client, fake_peer_service)
    }

    #[test]
    fn create_client_and_read_battery_level_success() {
        let initial_battery_level = 20;
        let (monitor, _fake_peer_service) = setup_client(initial_battery_level);
        assert_eq!(monitor.battery_level(), BatteryLevel(initial_battery_level));
    }

    #[test]
    fn minimal_battery_service_has_no_stream() {
        let mut fake_peer_service = FakePeerService::new();
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: BATTERY_LEVEL_HANDLE,
                uuid: BATTERY_LEVEL_UUID,
                properties: CharacteristicProperties(vec![
                    CharacteristicProperty::Read, // Only read is mandatory
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![10], // Initial battery level
        );

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let create_result =
            BatteryMonitorClient::<FakeTypes>::create(FakeClient::new(), fake_peer_service.clone());
        pin_mut!(create_result);
        let Poll::Ready(Ok(mut monitor)) = create_result.poll_unpin(&mut noop_cx) else {
            panic!("Expected BatteryMonitorClient to be successfully created");
        };
        assert_eq!(monitor.battery_level(), BatteryLevel(10));

        // No event stream should be available since the service does not
        // support notifications on any characteristics.
        assert!(monitor.take_event_stream().is_none());
    }

    #[test]
    fn empty_battery_service_is_error() {
        let fake_peer_service = FakePeerService::new();
        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let create_result =
            BatteryMonitorClient::<FakeTypes>::create(FakeClient::new(), fake_peer_service);
        pin_mut!(create_result);
        let polled = create_result.poll_unpin(&mut noop_cx);
        let Poll::Ready(Err(Error::Service(ServiceError::MissingCharacteristic))) = polled else {
            panic!("Expected BatteryMonitorClient failure");
        };
    }

    #[test]
    fn service_missing_battery_level_characteristic_is_error() {
        let mut fake_peer_service = FakePeerService::new();
        // Battery Level characteristic is invalidly formatted.
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: BATTERY_LEVEL_HANDLE,
                uuid: Uuid::from_u16(0x1234), // Random UUID, not Battery Level
                properties: CharacteristicProperties(vec![
                    CharacteristicProperty::Read,
                    CharacteristicProperty::Notify,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let create_result =
            BatteryMonitorClient::<FakeTypes>::create(FakeClient::new(), fake_peer_service);
        pin_mut!(create_result);
        let polled = create_result.poll_unpin(&mut noop_cx);
        let Poll::Ready(Err(Error::Service(ServiceError::MissingCharacteristic))) = polled else {
            panic!("Expected BatteryMonitorClient failure");
        };
    }

    #[test]
    fn invalid_battery_level_value_is_error() {
        // Battery Level characteristic has an empty battery level value.
        let mut fake_peer_service = FakePeerService::new();
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: BATTERY_LEVEL_HANDLE,
                uuid: BATTERY_LEVEL_UUID,
                properties: CharacteristicProperties(vec![
                    CharacteristicProperty::Read,
                    CharacteristicProperty::Notify,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );
        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let create_result =
            BatteryMonitorClient::<FakeTypes>::create(FakeClient::new(), fake_peer_service);
        pin_mut!(create_result);
        let polled = create_result.poll_unpin(&mut noop_cx);
        let Poll::Ready(Err(Error::Packet(PacketError::UnexpectedDataLength))) = polled else {
            panic!("Expected BatteryMonitorClient to be successfully created");
        };
    }

    #[test]
    fn battery_level_notifications() {
        let initial_battery_level = 20;
        let (mut monitor, fake_peer_service) = setup_client(initial_battery_level);

        let mut notification_stream =
            monitor.take_event_stream().expect("contains notification stream");
        // Trying to grab it again should be handled gracefully and yield no stream.
        assert!(monitor.take_event_stream().is_none());
        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        assert!(notification_stream.poll_next_unpin(&mut noop_cx).is_pending());

        // Simulate a notification on the battery level characteristic.
        let new_battery_level = 88;
        let notification = CharacteristicNotification {
            handle: BATTERY_LEVEL_HANDLE,
            value: vec![new_battery_level],
            maybe_truncated: false,
        };
        fake_peer_service.notify(&BATTERY_LEVEL_HANDLE, Ok(notification));
        let Poll::Ready(Some(Ok(received_battery_level))) =
            notification_stream.poll_next_unpin(&mut noop_cx)
        else {
            panic!("expected battery level change");
        };
        assert_eq!(received_battery_level, BatteryLevel(new_battery_level));

        let new_battery_level2 = 76;
        let notification2 = CharacteristicNotification {
            handle: BATTERY_LEVEL_HANDLE,
            value: vec![new_battery_level2],
            maybe_truncated: false,
        };
        fake_peer_service.notify(&BATTERY_LEVEL_HANDLE, Ok(notification2));
        let Poll::Ready(Some(Ok(received_battery_level2))) =
            notification_stream.poll_next_unpin(&mut noop_cx)
        else {
            panic!("expected battery level change");
        };
        assert_eq!(received_battery_level2, BatteryLevel(new_battery_level2));

        // A GATT error should still be propagated to the stream.
        let error = GattError::UnlikelyError.into();
        fake_peer_service.notify(&BATTERY_LEVEL_HANDLE, Err(error));
        let Poll::Ready(Some(Err(Error::GattLibrary(_)))) =
            notification_stream.poll_next_unpin(&mut noop_cx)
        else {
            panic!("expected GATT library error");
        };
        // Stream should still be active.
        assert!(notification_stream.poll_next_unpin(&mut noop_cx).is_pending());
        assert!(!notification_stream.is_terminated());
    }

    #[test]
    fn notification_stream_error_terminates_event_stream() {
        let (mut monitor, fake_peer_service) = setup_client(10);

        let mut notification_stream =
            monitor.take_event_stream().expect("contains notification stream");
        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        assert!(notification_stream.poll_next_unpin(&mut noop_cx).is_pending());

        let error = GattLibraryError::Other("random error".into());
        fake_peer_service.notify(&BATTERY_LEVEL_HANDLE, Err(error));
        let Poll::Ready(Some(Err(Error::GattLibrary(_)))) =
            notification_stream.poll_next_unpin(&mut noop_cx)
        else {
            panic!("expected GATT library error");
        };
        // The stream should be staged for shutdown since a fatal error was received.
        let Poll::Ready(None) = notification_stream.poll_next_unpin(&mut noop_cx) else {
            panic!("expected notification stream termination");
        };
        assert!(notification_stream.is_terminated());
    }

    #[test]
    fn terminating_notification_stream_terminates_event_stream() {
        let (mut monitor, fake_peer_service) = setup_client(10);

        let mut notification_stream =
            monitor.take_event_stream().expect("contains notification stream");
        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        assert!(notification_stream.poll_next_unpin(&mut noop_cx).is_pending());

        fake_peer_service.clear_notifier(&BATTERY_LEVEL_HANDLE);
        let Poll::Ready(Some(Err(Error::Service(_)))) =
            notification_stream.poll_next_unpin(&mut noop_cx)
        else {
            panic!("expected service error");
        };
        // The stream should be staged for shutdown since there are no more active
        // notification streams.
        let Poll::Ready(None) = notification_stream.poll_next_unpin(&mut noop_cx) else {
            panic!("expected notification stream termination");
        };
        assert!(notification_stream.is_terminated());
    }
}
