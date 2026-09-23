// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_helpers::maybe_stream::MaybeStream;
use core::pin::Pin;
use core::task::{Context, Poll};
use fidl_fuchsia_bluetooth::ChannelParameters;
use fidl_fuchsia_bluetooth_bredr::{
    ProfileProxy, ProtocolDescriptor, ProtocolIdentifier, ServiceDefinition,
};
use fuchsia_bluetooth::types::{Channel, PeerId, Uuid};
use futures::stream::{FusedStream, Stream, StreamExt};
use futures::{SinkExt, ready};
use log::{info, trace, warn};
use packet_encoding::Encodable;
use profile_client::{ProfileClient, ProfileEvent};

use crate::types::Error;
use crate::types::packets::MessageStreamPacket;

/// The service UUID associated with the Fast Pair service.
/// "df21fe2c-2515-4fdb-8886-f12c4d67927c"
/// See https://developers.google.com/nearby/fast-pair/specifications/extensions/messagestream#MessageStream
const FASTPAIR_SERVICE_UUID: Uuid = Uuid::from_bytes([
    0x7c, 0x92, 0x67, 0x4d, 0x2c, 0xf1, 0x86, 0x88, 0xdb, 0x4f, 0x15, 0x25, 0x2c, 0xfe, 0x21, 0xdf,
]);

/// The `MessageStream` manages the Fast Pair advertisement of RFCOMM and a potential RFCOMM
/// connection to the remote peer. It processes events over the `fuchsia.bluetooth.bredr.Profile`
/// API and handles incoming RFCOMM connections. Only one RFCOMM connection can be active at a time.
/// Subsequent connections will overwrite the existing connection.
///
/// `MessageStream` implements Stream where each item is a notification of an incoming RFCOMM
/// connection. The object _must_ be polled in order to receive these events and process data
/// received over the channel.
pub struct MessageStream {
    profile: ProfileClient,
    rfcomm: MaybeStream<Channel>,
}

impl MessageStream {
    pub fn new(profile: ProfileProxy) -> Self {
        // Failure to advertise is not fatal. Fast Pair still works without RFCOMM.
        let profile = match Self::advertise(&profile) {
            Ok(p) => p,
            Err(e) => {
                warn!(e:?; "Couldn't advertise the RFCOMM service");
                ProfileClient::new(profile)
            }
        };
        Self { profile, rfcomm: MaybeStream::default() }
    }

    fn advertise(profile: &ProfileProxy) -> Result<ProfileClient, Error> {
        let service = ServiceDefinition {
            service_class_uuids: Some(vec![FASTPAIR_SERVICE_UUID.into()]),
            protocol_descriptor_list: Some(vec![
                ProtocolDescriptor {
                    protocol: Some(ProtocolIdentifier::L2Cap),
                    params: Some(vec![]),
                    ..Default::default()
                },
                ProtocolDescriptor {
                    protocol: Some(ProtocolIdentifier::Rfcomm),
                    params: Some(vec![]),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        ProfileClient::advertise(profile.clone(), vec![service], ChannelParameters::default())
            .map_err(Into::into)
    }

    #[cfg(test)]
    pub fn rfcomm_connected(&self) -> bool {
        self.rfcomm.is_some()
    }

    /// Processes an event from the `bredr.Profile` protocol.
    /// Returns a PeerId if the `event` is an incoming connection from a remote peer.
    fn handle_profile_event(&mut self, event: ProfileEvent) -> Option<PeerId> {
        let ProfileEvent::PeerConnected { id, protocol, channel } = event else {
            return None;
        };

        if !protocol
            .iter()
            .any(|descriptor| descriptor.protocol == Some(ProtocolIdentifier::Rfcomm))
        {
            warn!(id:%; "Received non-RFCOMM connection. Ignoring");
            return None;
        }

        if self.rfcomm.is_some() {
            trace!(id:%; "RFCOMM connection already exists. Overwriting");
        }
        self.rfcomm.set(channel);
        Some(id)
    }

    fn handle_rfcomm_data(&mut self, data: Vec<u8>) {
        info!(data:?; "Received Message Stream request");
        // We don't support any Message Stream requests.
        // TODO(https://fxbug.dev/42062580): Parse and handle Message Stream requests.
    }
}

impl futures::sink::Sink<MessageStreamPacket> for MessageStream {
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        let Some(rfcomm) = this.rfcomm.inner_mut() else {
            return Poll::Ready(Err(Error::internal("No RFCOMM connection available")));
        };
        rfcomm.poll_ready_unpin(cx).map_err(|e| Error::internal(&format!("{e:?}")))
    }

    fn start_send(self: Pin<&mut Self>, packet: MessageStreamPacket) -> Result<(), Self::Error> {
        let this = self.get_mut();
        let Some(rfcomm) = this.rfcomm.inner_mut() else {
            return Err(Error::internal("No RFCOMM connection available"));
        };
        let mut buf = vec![0; packet.encoded_len()];
        packet.encode(&mut buf[..]).expect("valid packet");
        rfcomm.start_send_unpin(buf).map_err(|e| Error::internal(&format!("{e:?}")))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        let Some(rfcomm) = this.rfcomm.inner_mut() else {
            return Poll::Ready(Err(Error::internal("No RFCOMM connection available")));
        };
        rfcomm.poll_flush_unpin(cx).map_err(|e| Error::internal(&format!("{e:?}")))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        let Some(rfcomm) = this.rfcomm.inner_mut() else {
            return Poll::Ready(Err(Error::internal("No RFCOMM connection available")));
        };
        rfcomm.poll_close_unpin(cx).map_err(|e| Error::internal(&format!("{e:?}")))
    }
}

impl Stream for MessageStream {
    type Item = PeerId;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.is_terminated() {
            panic!("Cannot poll a terminated stream");
        }

        // Check the active RFCOMM advertisement.
        let profile_event = self.profile.poll_next_unpin(cx);
        match profile_event {
            Poll::Pending => (),
            Poll::Ready(Some(Ok(event))) => {
                if let Some(id) = self.handle_profile_event(event) {
                    return Poll::Ready(Some(id));
                }
            }
            Poll::Ready(Some(Err(e))) => {
                warn!(e:?; "Error in ProfileClient stream");
            }
            Poll::Ready(None) => {
                info!("ProfileClient stream terminated");
                return Poll::Ready(None);
            }
        }

        // Check the active RFCOMM connection - this will loop until `self.rfcomm` returns
        // Poll::Pending.
        loop {
            let rfcomm_event = ready!(self.rfcomm.poll_next_unpin(cx));
            match rfcomm_event {
                Some(Ok(data)) => self.handle_rfcomm_data(data),
                Some(Err(e)) => {
                    warn!(e:?; "Error in RFCOMM channel");
                    let _ = MaybeStream::take(&mut self.rfcomm);
                }
                None => {
                    info!("RFCOMM connection closed");
                    let _ = MaybeStream::take(&mut self.rfcomm);
                }
            }
        }
    }
}

impl FusedStream for MessageStream {
    fn is_terminated(&self) -> bool {
        self.profile.is_terminated()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::types::ModelId;
    use assert_matches::assert_matches;
    use bt_channel_test_support::{Transport, create_test_channels};
    use fidl_fuchsia_bluetooth_bredr::{DataElement, ProfileMarker};
    use fuchsia_bluetooth::types::Address;
    use futures::FutureExt;
    use test_case::test_case;

    async fn expect_data(remote: &mut Channel, expected_data: Vec<u8>) {
        let read = match remote.next().await {
            Some(Ok(data)) => data,
            x => panic!("Expected data from the peer, got {x:?}"),
        };
        assert_eq!(read.len(), expected_data.len());
        assert_eq!(read, expected_data);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    async fn message_stream_receives_data(transport: Transport) {
        let (profile, mut profile_server) =
            fidl::endpoints::create_proxy_and_stream::<ProfileMarker>();
        let mut message_stream = MessageStream::new(profile);

        // Expect the RFCOMM advertisement.
        let profile_event = profile_server.select_next_some().await.expect("FIDL response");
        let (request, _responder) = profile_event.into_advertise().expect("Advertise request");
        let connect_proxy = request.receiver.unwrap().into_proxy();

        // Remote peer connection.
        let id = PeerId(123);
        let (local, mut remote) = create_test_channels(transport);
        let protocol = &[
            ProtocolDescriptor {
                protocol: Some(ProtocolIdentifier::L2Cap),
                params: Some(vec![]),

                ..Default::default()
            },
            ProtocolDescriptor {
                protocol: Some(ProtocolIdentifier::Rfcomm),
                params: Some(vec![DataElement::Uint8(1)]),
                ..Default::default()
            },
        ];
        connect_proxy.connected(&id.into(), local.try_into().unwrap(), protocol).unwrap();

        // Expect the MessageStream to produce the connected event.
        let connected_id = message_stream.select_next_some().await;
        assert_eq!(connected_id, id);
        assert!(message_stream.rfcomm_connected());

        // Remote can write data - it's just logged.
        remote.send(vec![0, 1, 2, 3]).await.expect("can write data");
        assert_matches!(message_stream.select_next_some().now_or_never(), None);

        // Local can send Model ID and local address.
        let model_id_packet =
            MessageStreamPacket::new_model_id(ModelId::try_from(0x123456).expect("valid"));
        message_stream.send(model_id_packet).await.expect("can send model id");
        expect_data(&mut remote, vec![0x03, 0x01, 0x00, 0x03, 0x12, 0x34, 0x56]).await;
        let address_packet = MessageStreamPacket::new_address(Address::Public([1, 2, 3, 4, 5, 6]));
        message_stream.send(address_packet).await.expect("can send address");
        expect_data(&mut remote, vec![0x03, 0x02, 0x00, 0x06, 0x6, 0x5, 0x4, 0x3, 0x2, 0x1]).await;

        // Remote disconnects RFCOMM - MessageStream itself should still be active.
        drop(remote);
        assert_matches!(message_stream.next().now_or_never(), None);
        assert!(!message_stream.is_terminated());

        // If the upstream Profile Server closes the advertisement, then the message stream will
        // terminate.
        drop(connect_proxy);
        drop(_responder);
        assert_matches!(message_stream.next().await, None);
        assert!(message_stream.is_terminated());
    }
}
