// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_bluetooth::types::Channel;
use futures::stream::{FusedStream, TryStreamExt};
use log::{info, trace};
use packet_encoding::Encodable;
use std::cell::{RefCell, RefMut};
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::error::{Error, PacketError};
use crate::operation::{MAX_PACKET_SIZE, MIN_MAX_PACKET_SIZE, OpCode, ResponsePacket};

/// Returns the maximum packet size that will be used for the OBEX session.
/// `transport_max` is the maximum size that the underlying transport (e.g. L2CAP, RFCOMM) supports.
pub fn max_packet_size_from_transport(transport_max: usize) -> u16 {
    let bounded = transport_max.clamp(MIN_MAX_PACKET_SIZE, MAX_PACKET_SIZE);
    bounded.try_into().expect("bounded by u16 max")
}

/// The underlying communication protocol used for the OBEX transport.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TransportType {
    L2cap,
    Rfcomm,
}

impl TransportType {
    pub fn srm_supported(&self) -> bool {
        match &self {
            // Per GOEP Section 7.1, SRM can be used with the L2CAP transport.
            Self::L2cap => true,
            // Neither the OBEX nor GOEP specifications explicitly state that SRM cannot be used
            // with the RFCOMM transport. However, all qualification tests and spec language
            // suggest that SRM is to be used only on the L2CAP transport.
            Self::Rfcomm => false,
        }
    }
}

/// Holds the underlying RFCOMM or L2CAP transport for an OBEX operation.
#[derive(Debug)]
pub struct ObexTransport<'a> {
    /// A mutable reference to the permit given to the operation.
    /// The L2CAP or RFCOMM connection to the remote peer.
    channel: RefMut<'a, Channel>,
    /// The type of transport used in the OBEX connection.
    type_: TransportType,
}

impl<'a> ObexTransport<'a> {
    pub fn new(channel: RefMut<'a, Channel>, type_: TransportType) -> Self {
        Self { channel, type_ }
    }

    /// Returns true if this transport supports the Single Response Mode (SRM) feature.
    pub fn srm_supported(&self) -> bool {
        self.type_.srm_supported()
    }

    /// Attempts to receive and parse an OBEX response packet from the `channel`.
    /// Returns the parsed packet on success, Error otherwise.
    // TODO(https://fxbug.dev/42076096): Make this more generic to decode either request or response packets
    // when OBEX Server functionality is needed.
    pub async fn receive_response(&mut self, code: OpCode) -> Result<ResponsePacket, Error> {
        if self.channel.is_terminated() {
            return Err(Error::PeerDisconnected);
        }

        match self.channel.try_next().await? {
            Some(raw_data) => {
                let decoded = ResponsePacket::decode(&raw_data[..], code)?;
                trace!("Received response: {decoded:?}");
                Ok(decoded)
            }
            None => {
                info!("OBEX transport closed");
                Err(Error::PeerDisconnected)
            }
        }
    }
}

impl<'a, T> futures::sink::Sink<T> for ObexTransport<'a>
where
    T: Encodable<Error = PacketError>,
{
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        Pin::new(&mut *this.channel).poll_ready(cx).map_err(Into::into)
    }

    fn start_send(self: Pin<&mut Self>, data: T) -> Result<(), Self::Error> {
        let mut buf = vec![0; data.encoded_len()];
        data.encode(&mut buf[..])?;
        let this = self.get_mut();
        Pin::new(&mut *this.channel).start_send(buf).map_err(Into::into)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        Pin::new(&mut *this.channel).poll_flush(cx).map_err(Into::into)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        Pin::new(&mut *this.channel).poll_close(cx).map_err(Into::into)
    }
}

/// Manages the transport connection (L2CAP/RFCOMM) to a remote peer.
/// Provides a reservation system for acquiring the transport for an in-progress OBEX operation.
#[derive(Debug)]
pub struct ObexTransportManager {
    /// Holds the underlying transport. The type of transport is indicated by the `type_` field.
    /// There can only be one operation outstanding at any time. A mutable reference to the
    /// `Channel` will be held by the `ObexTransport` during an ongoing operation and is
    /// assigned using `ObexTransportManager::try_new_operation`. On operation termination (e.g.
    /// `ObexTransport` is dropped), the `Channel` will be available for subsequent mutable access.
    channel: RefCell<Channel>,
    /// The transport type (L2CAP or RFCOMM) for the `channel`.
    type_: TransportType,
}

impl ObexTransportManager {
    pub fn new(channel: Channel, type_: TransportType) -> Self {
        Self { channel: RefCell::new(channel), type_ }
    }

    fn new_permit(&self) -> Result<RefMut<'_, Channel>, Error> {
        self.channel.try_borrow_mut().map_err(|_| Error::OperationInProgress)
    }

    pub fn is_transport_closed(&self) -> bool {
        self.channel.try_borrow().map_or(false, |chan| chan.is_closed())
    }

    pub fn try_new_operation(&self) -> Result<ObexTransport<'_>, Error> {
        // Only one operation can be outstanding at a time.
        let channel = self.new_permit()?;
        Ok(ObexTransport::new(channel, self.type_))
    }
}

#[cfg(test)]
pub(crate) mod test_utils {
    use super::*;
    use bt_channel_test_support::{Transport, create_test_channels};
    use futures::SinkExt;

    use async_test_helpers::expect_stream_item;
    use fuchsia_async as fasync;
    use packet_encoding::Decodable;

    use crate::operation::RequestPacket;

    /// Set `srm_supported` to true to build a transport that supports the OBEX SRM feature.
    pub(crate) fn new_manager(
        transport: Transport,
        srm_supported: bool,
    ) -> (ObexTransportManager, Channel) {
        let (local, remote) = create_test_channels(transport);
        let type_ = if srm_supported { TransportType::L2cap } else { TransportType::Rfcomm };
        let manager = ObexTransportManager::new(local, type_);
        (manager, remote)
    }

    #[derive(Clone)]
    pub struct TestPacket(pub u8);

    impl Encodable for TestPacket {
        type Error = PacketError;
        fn encoded_len(&self) -> usize {
            1
        }
        fn encode(&self, buf: &mut [u8]) -> Result<(), Self::Error> {
            buf[0] = self.0;
            Ok(())
        }
    }

    impl Decodable for TestPacket {
        type Error = PacketError;
        fn decode(buf: &[u8]) -> Result<Self, Self::Error> {
            Ok(TestPacket(buf[0]))
        }
    }

    #[track_caller]
    fn assert_write_initiated(res: Poll<Result<(), zx::Status>>) {
        match res {
            Poll::Ready(Err(e)) => panic!("Write failed: {:?}", e),
            _ => {}
        }
    }

    #[track_caller]
    pub fn reply(exec: &mut fasync::TestExecutor, channel: &mut Channel, response: ResponsePacket) {
        let mut response_buf = vec![0; response.encoded_len()];
        response.encode(&mut response_buf[..]).expect("can encode response");
        let mut fut = channel.send(response_buf.to_vec());
        assert_write_initiated(exec.run_until_stalled(&mut fut));
    }

    #[track_caller]
    pub fn send_packet<T>(exec: &mut fasync::TestExecutor, channel: &mut Channel, packet: T)
    where
        T: Encodable,
        <T as Encodable>::Error: std::fmt::Debug,
    {
        let mut buf = vec![0; packet.encoded_len()];
        packet.encode(&mut buf[..]).expect("can encode packet");
        let mut fut = channel.send(buf.to_vec());
        assert_write_initiated(exec.run_until_stalled(&mut fut));
    }

    #[track_caller]
    pub fn expect_request<F>(exec: &mut fasync::TestExecutor, channel: &mut Channel, expectation: F)
    where
        F: FnOnce(RequestPacket),
    {
        let request_raw = expect_stream_item(exec, channel).expect("request");
        let request = RequestPacket::decode(&request_raw[..]).expect("can decode request");
        expectation(request);
    }

    #[track_caller]
    pub fn expect_response<F>(
        exec: &mut fasync::TestExecutor,
        channel: &mut Channel,
        expectation: F,
        opcode: OpCode,
    ) where
        F: FnOnce(ResponsePacket),
    {
        let request_raw = expect_stream_item(exec, channel).expect("request");
        let request = ResponsePacket::decode(&request_raw[..], opcode).expect("can decode request");
        expectation(request);
    }

    /// Expects a request packet on the `channel` and validates the contents with the provided
    /// `expectation`. Sends a `response` back on the channel.
    #[track_caller]
    pub fn expect_request_and_reply<F>(
        exec: &mut fasync::TestExecutor,
        channel: &mut Channel,
        expectation: F,
        response: ResponsePacket,
    ) where
        F: FnOnce(RequestPacket),
    {
        expect_request(exec, channel, expectation);
        reply(exec, channel, response)
    }

    pub fn expect_code(code: OpCode) -> impl FnOnce(RequestPacket) {
        let f = move |request: RequestPacket| {
            assert_eq!(*request.code(), code);
        };
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_channel_test_support::Transport;
    use futures::SinkExt;
    use test_case::test_case;

    use assert_matches::assert_matches;

    use async_utils::PollExt;
    use fuchsia_async as fasync;
    use std::pin::pin;

    use crate::header::HeaderSet;
    use crate::operation::{RequestPacket, ResponseCode};
    use crate::transport::test_utils::{
        TestPacket, expect_code, expect_request_and_reply, new_manager,
    };

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn transport_manager_new_operation(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (manager, _remote) = new_manager(transport, /* srm_supported */ false);

        // Nothing should be in progress.
        assert_matches!(manager.new_permit(), Ok(_));

        // Should be able to start a new operation.
        let transport1 = manager.try_new_operation().expect("can start operation");
        // Trying to start another should be an Error.
        assert_matches!(manager.try_new_operation(), Err(Error::OperationInProgress));

        // Once the first finishes, another can be claimed.
        drop(transport1);
        let mut transport2 = manager.try_new_operation().expect("can start another operation");
        let request = RequestPacket::new_connect(100, HeaderSet::new());
        let mut send_fut = pin!(transport2.send(request));
        exec.run_until_stalled(&mut send_fut)
            .expect("send result ready")
            .expect("can send request");
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn send_and_receive(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (manager, mut remote) = new_manager(transport, /* srm_supported */ false);
        let mut transport = manager.try_new_operation().expect("can start operation");

        // Local makes a request
        let request = RequestPacket::new_connect(100, HeaderSet::new());
        {
            let mut send_fut = pin!(transport.send(request));
            exec.run_until_stalled(&mut send_fut)
                .expect("send result ready")
                .expect("can send request");
        }
        // Remote end should receive it - send an example response back.
        let peer_response =
            ResponsePacket::new(ResponseCode::Ok, vec![0x10, 0x00, 0x00, 0xff], HeaderSet::new());
        expect_request_and_reply(
            &mut exec,
            &mut remote,
            expect_code(OpCode::Connect),
            peer_response,
        );
        // Expect it on the ObexTransport
        let receive_fut = transport.receive_response(OpCode::Connect);
        let mut receive_fut = pin!(receive_fut);
        let received_response = exec
            .run_until_stalled(&mut receive_fut)
            .expect("stream item from response")
            .expect("valid response");
        assert_eq!(*received_response.code(), ResponseCode::Ok);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    async fn send_while_channel_closed_is_error(transport: Transport) {
        let (manager, remote) = new_manager(transport, /* srm_supported */ false);
        let mut transport = manager.try_new_operation().expect("can start operation");
        drop(remote);

        let data = TestPacket(10);
        let send_result = transport.send(data.clone()).await;
        assert_matches!(send_result, Err(Error::IOError(_)));
        // Trying again is still an Error.
        let send_result = transport.send(data.clone()).await;
        assert_matches!(send_result, Err(Error::IOError(_)));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn is_transport_closed(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (manager, remote) = new_manager(transport, /* srm_supported */ false);
        assert!(!manager.is_transport_closed());

        {
            let _transport = manager.try_new_operation().expect("can start operation");
            assert!(!manager.is_transport_closed());

            // Even when the remote end is dropped, transport is deemed
            // as active since there is currently an ongoing operation.
            drop(remote);
            let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
            assert!(!manager.is_transport_closed());
        }

        // When transport goes out of scope, finally transport is
        // considered fully closed.
        assert!(manager.is_transport_closed());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    async fn receive_while_channel_closed_is_error(transport: Transport) {
        let (manager, remote) = new_manager(transport, /* srm_supported */ false);
        let mut transport = manager.try_new_operation().expect("can start operation");
        drop(remote);

        let receive_result = transport.receive_response(OpCode::Connect).await;
        assert_matches!(receive_result, Err(Error::PeerDisconnected));
        // Trying again is handled gracefully - still an Error.
        let receive_result = transport.receive_response(OpCode::Connect).await;
        assert_matches!(receive_result, Err(Error::PeerDisconnected));
    }
}
