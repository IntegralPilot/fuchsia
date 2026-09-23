// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_bluetooth::types::Channel;
use futures::SinkExt;
use log::{trace, warn};

pub use crate::client::get::GetOperation;
pub use crate::client::put::PutOperation;
use crate::error::Error;
use crate::header::{
    ConnectionIdentifier, Header, HeaderIdentifier, HeaderSet, SingleResponseMode,
};
use crate::operation::{OpCode, RequestPacket, ResponseCode, ResponsePacket, SetPathFlags};
pub use crate::transport::TransportType;
use crate::transport::{ObexTransportManager, max_packet_size_from_transport};
use fuchsia_sync::Mutex;

/// Implements the OBEX PUT operation.
mod put;

/// Implements the OBEX GET operation.
mod get;

/// An interface for the Single Response Mode (SRM) feature for an OBEX client operation.
pub(crate) trait SrmOperation {
    const OPERATION_TYPE: OpCode;

    /// Returns the current SRM mode.
    fn get_srm(&self) -> SingleResponseMode;

    /// Sets SRM to the provided `mode`.
    fn set_srm(&mut self, mode: SingleResponseMode);

    /// Attempts to enable SRM for the operation by updating the provided `headers` with the SRM
    /// header.
    /// Returns Error if `headers` couldn't be updated with SRM, Ok otherwise.
    fn try_enable_srm(&mut self, headers: &mut HeaderSet) -> Result<(), Error> {
        let requested_srm = headers.try_add_srm(self.get_srm())?;
        self.set_srm(requested_srm);
        trace!(operation:? = Self::OPERATION_TYPE; "Requesting SRM {requested_srm:?}");
        Ok(())
    }

    /// Checks the provided response `headers` for the SRM flag and updates the local SRM state
    /// for the operation.
    fn check_response_for_srm(&mut self, headers: &HeaderSet) {
        let srm_response = if let Some(Header::SingleResponseMode(srm)) =
            headers.get(&HeaderIdentifier::SingleResponseMode)
        {
            *srm
        } else {
            // No SRM indication from peer defaults to disabled.
            trace!(operation:? = Self::OPERATION_TYPE; "Response doesn't contain SRM header");
            SingleResponseMode::Disable
        };

        trace!(current_status:? = self.get_srm(), operation:? = Self::OPERATION_TYPE; "Peer responded with {srm_response:?}");
        match (srm_response, self.get_srm()) {
            (SingleResponseMode::Enable, SingleResponseMode::Disable) => {
                warn!("SRM stays disabled");
            }
            (SingleResponseMode::Disable, SingleResponseMode::Enable) => {
                trace!("SRM is disabled");
                self.set_srm(SingleResponseMode::Disable);
            }
            _ => {} // Otherwise, both sides agree on the SRM status.
        }
        trace!(status:? = self.get_srm(), operation:? = Self::OPERATION_TYPE; "SRM status");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
enum ConnectionStatus {
    /// The transport is created but the CONNECT operation has not been completed.
    #[default]
    Initialized,
    /// The CONNECT operation has been completed and the transport is considered connected.
    /// `id` contains the optional identifier for this connection. This is configured during the
    /// CONNECT operation and is a unique value assigned by the remote OBEX server.
    Connected { id: Option<ConnectionIdentifier> },
    /// The transport is considered disconnected -- the OBEX client has Disconnected the session.
    Disconnected,
}

impl ConnectionStatus {
    #[cfg(test)]
    fn connected_no_id() -> Self {
        Self::Connected { id: None }
    }
}

/// The Client role for an OBEX session.
/// Provides an interface for connecting to a remote OBEX server and initiating the operations
/// specified in OBEX 1.5.
#[derive(Debug)]
pub struct ObexClient {
    /// Whether the CONNECT operation has completed.
    connected: Mutex<ConnectionStatus>,
    /// The maximum OBEX packet length for this OBEX session.
    max_packet_size: Mutex<u16>,
    /// Manages the RFCOMM or L2CAP transport and provides a reservation system for starting
    /// new operations.
    transport: ObexTransportManager,
}

impl ObexClient {
    pub fn new(channel: Channel, type_: TransportType) -> Self {
        let max_packet_size = max_packet_size_from_transport(channel.max_tx_size());
        let transport = ObexTransportManager::new(channel, type_);
        Self {
            connected: Mutex::new(ConnectionStatus::default()),
            max_packet_size: Mutex::new(max_packet_size),
            transport,
        }
    }

    pub fn is_transport_connected(&self) -> bool {
        !self.transport.is_transport_closed()
    }

    fn set_connection_status(&self, status: ConnectionStatus) {
        *self.connected.lock() = status;
    }

    fn connection_status(&self) -> ConnectionStatus {
        *self.connected.lock()
    }

    pub fn is_connected(&self) -> bool {
        matches!(*self.connected.lock(), ConnectionStatus::Connected { .. })
    }

    pub fn connection_id(&self) -> Option<ConnectionIdentifier> {
        match self.connection_status() {
            ConnectionStatus::Connected { id } => id.clone(),
            _ => None,
        }
    }

    fn set_max_packet_size(&self, peer_max_packet_size: u16) {
        // We have no opinion on the preferred max packet size, so just use the peer's.
        *self.max_packet_size.lock() = peer_max_packet_size;
        trace!("Max packet size set to {peer_max_packet_size}");
    }

    fn max_packet_size(&self) -> u16 {
        *self.max_packet_size.lock()
    }

    fn handle_connect_response(&self, response: ResponsePacket) -> Result<HeaderSet, Error> {
        let request = OpCode::Connect;
        let response = response.expect_code(request, ResponseCode::Ok)?;

        // Expect the 4 bytes of additional data. We negotiate the max packet length based on what
        // the peer requests. See OBEX 1.5 Section 3.4.1.
        if response.data().len() != request.response_data_length() {
            return Err(Error::response(request, "Invalid CONNECT data"));
        }
        let peer_max_packet_size = u16::from_be_bytes(response.data()[2..4].try_into().unwrap());
        self.set_max_packet_size(peer_max_packet_size);

        // Check to see if the response headers contains a Connection Identifier. If so, this should
        // be included in all subsequent operations.
        let headers: HeaderSet = response.into();
        if let Some(Header::ConnectionId(id)) = headers.get(&HeaderIdentifier::ConnectionId) {
            trace!(id:? = id; "Found Connection Identifier in CONNECT response");
            self.set_connection_status(ConnectionStatus::Connected { id: Some(*id) });
        }
        Ok(headers)
    }

    /// Initiates a CONNECT request to the remote peer.
    /// Returns the Headers associated with the response on success.
    /// Returns Error if the CONNECT operation could not be completed.
    pub async fn connect(&self, headers: HeaderSet) -> Result<HeaderSet, Error> {
        if self.is_connected() {
            return Err(Error::operation(OpCode::Connect, "already connected"));
        }

        let response = {
            let request = RequestPacket::new_connect(self.max_packet_size(), headers);
            let mut transport = self.transport.try_new_operation()?;
            transport.send(request).await?;
            trace!("Successfully made CONNECT request");
            transport.receive_response(OpCode::Connect).await?
        };

        let response_headers = self.handle_connect_response(response)?;
        Ok(response_headers)
    }

    /// Initiates a DISCONNECT request to the remote peer.
    /// Returns the Headers associated with the response on success.
    /// Returns Error if the DISCONNECT operation couldn't be completed or was rejected by the peer.
    /// The OBEX Session with the peer is considered terminated, regardless.
    pub async fn disconnect(self, mut headers: HeaderSet) -> Result<HeaderSet, Error> {
        let opcode = OpCode::Disconnect;
        if !self.is_connected() {
            return Err(Error::operation(opcode, "session not connected"));
        }
        headers.try_add_connection_id(&self.connection_id())?;
        let response = {
            let request = RequestPacket::new_disconnect(headers);
            let mut transport = self.transport.try_new_operation()?;
            trace!("Making outgoing DISCONNECT request: {request:?}");
            transport.send(request).await?;
            trace!("Successfully made DISCONNECT request");
            transport.receive_response(opcode).await?
        };
        self.set_connection_status(ConnectionStatus::Disconnected);
        response.expect_code(opcode, ResponseCode::Ok).map(Into::into)
    }

    /// Initializes a GET Operation to retrieve data from the remote OBEX Server.
    /// Returns a `GetOperation` on success, Error if the new operation couldn't be started.
    pub fn get(&self) -> Result<GetOperation<'_>, Error> {
        // A GET can only be initiated after the OBEX session is connected.
        if !self.is_connected() {
            return Err(Error::operation(OpCode::Get, "session not connected"));
        }

        let mut headers = HeaderSet::new();
        headers.try_add_connection_id(&self.connection_id())?;

        // Only one operation can be active at a time.
        let transport = self.transport.try_new_operation()?;
        Ok(GetOperation::new(headers, transport))
    }

    /// Initializes a PUT Operation to write data to the remote OBEX Server.
    /// Returns a `PutOperation` on success, Error if the new operation couldn't be started.
    pub fn put(&self) -> Result<PutOperation<'_>, Error> {
        // A PUT can only be initiated after the OBEX session is connected.
        if !self.is_connected() {
            return Err(Error::operation(OpCode::Put, "session not connected"));
        }

        let mut headers = HeaderSet::new();
        headers.try_add_connection_id(&self.connection_id())?;

        // Only one operation can be active at a time.
        let transport = self.transport.try_new_operation()?;
        Ok(PutOperation::new(headers, transport))
    }

    /// Initializes a SETPATH Operation to set the current folder on the remote OBEX Server.
    /// Returns the Headers associated with the response on success.
    /// Returns `Error::NotImplemented` if the remote server does not support SETPATH.
    /// Returns Error for all other errors.
    pub async fn set_path(
        &self,
        flags: SetPathFlags,
        mut headers: HeaderSet,
    ) -> Result<HeaderSet, Error> {
        let opcode = OpCode::SetPath;
        // A SETPATH can only be initiated after the OBEX session is connected.
        if !self.is_connected() {
            return Err(Error::operation(opcode, "session not connected"));
        }
        headers.try_add_connection_id(&self.connection_id())?;
        let request = RequestPacket::new_set_path(flags, headers)?;
        let response = {
            let mut transport = self.transport.try_new_operation()?;
            trace!("Making outgoing SETPATH request: {request:?}");
            transport.send(request).await?;
            trace!("Successfully made SETPATH request");
            transport.receive_response(opcode).await?
        };

        // Per OBEX Section 3.4.6, the server may respond with BadRequest or Forbidden if it does
        // not support the operation.
        if *response.code() == ResponseCode::BadRequest
            || *response.code() == ResponseCode::Forbidden
        {
            return Err(Error::not_implemented(opcode));
        }
        response.expect_code(opcode, ResponseCode::Ok).map(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_channel_test_support::{Transport, create_test_channels};
    use test_case::test_case;

    use assert_matches::assert_matches;
    use async_utils::PollExt;
    use fuchsia_async as fasync;
    use std::pin::pin;

    use crate::transport::test_utils::{expect_code, expect_request_and_reply};

    #[fuchsia::test]
    fn max_packet_size_calculation() {
        // Value in [255, 65535] should be kept.
        let transport_max = 1000;
        assert_eq!(max_packet_size_from_transport(transport_max), 1000);

        // Lower bound should be enforced.
        let transport_max_small = 40;
        assert_eq!(max_packet_size_from_transport(transport_max_small), 255);

        // Upper bound should be enforced.
        let transport_max_large = 700000;
        assert_eq!(max_packet_size_from_transport(transport_max_large), u16::MAX);
    }

    /// Returns a new ObexClient and the remote end of the transport.
    /// If `connected` is set, returns an ObexClient in the connected state, indicating the
    /// completion of the OBEX CONNECT procedure.
    fn new_obex_client(connected: ConnectionStatus, transport: Transport) -> (ObexClient, Channel) {
        let (local, remote) = create_test_channels(transport);
        let client = ObexClient::new(local, TransportType::Rfcomm);
        client.set_connection_status(connected);
        (client, remote)
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn client_connect_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (client, mut remote) = new_obex_client(ConnectionStatus::default(), transport);

        assert!(!client.is_connected());
        assert_eq!(client.max_packet_size(), Channel::DEFAULT_MAX_TX as u16);
        assert_eq!(client.connection_id(), None);

        {
            let connect_fut = client.connect(HeaderSet::new());
            let mut connect_fut = pin!(connect_fut);
            exec.run_until_stalled(&mut connect_fut).expect_pending("waiting for response");

            // Expect the Connect request on the remote and reply positively.
            let response_headers =
                HeaderSet::from_headers(vec![Header::ConnectionId(ConnectionIdentifier(1))])
                    .unwrap();
            let response = ResponsePacket::new(
                ResponseCode::Ok,
                vec![0x10, 0x00, 0xff, 0xff], // Version = 1.0, Flags = 0, Max packet = 0xffff
                response_headers.clone(),
            );
            expect_request_and_reply(
                &mut exec,
                &mut remote,
                expect_code(OpCode::Connect),
                response,
            );

            let connect_result = exec
                .run_until_stalled(&mut connect_fut)
                .expect("received response")
                .expect("response is ok");
            assert_eq!(connect_result, response_headers);
        }

        // Should be connected with the max packet size specified by the peer.
        assert!(client.is_connected());
        assert_eq!(client.max_packet_size(), 0xffff);
        assert_eq!(client.connection_id(), Some(ConnectionIdentifier(1)));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    async fn multiple_connect_is_error(transport: Transport) {
        let (client, _remote) = new_obex_client(ConnectionStatus::connected_no_id(), transport);

        // Trying to connect again is an Error since it can only be done once.
        let result = client.connect(HeaderSet::new()).await;
        assert_matches!(result, Err(Error::OperationError { .. }));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn get_before_connect_is_error(transport: Transport) {
        let _exec = fasync::TestExecutor::new();
        let (client, _remote) = new_obex_client(ConnectionStatus::default(), transport);

        let get_result = client.get();
        assert_matches!(get_result, Err(Error::OperationError { .. }));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn sequential_get_operations_is_ok(transport: Transport) {
        let _exec = fasync::TestExecutor::new();
        let (client, _remote) = new_obex_client(ConnectionStatus::connected_no_id(), transport);

        // Creating the first GET operation should succeed.
        let _get_operation1 = client.get().expect("can initialize first get");

        // After the first one "completes" (e.g. no longer held), it's okay to initiate a GET.
        drop(_get_operation1);
        let _get_operation2 = client.get().expect("can initialize second get");
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn disconnect_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (client, mut remote) = new_obex_client(ConnectionStatus::connected_no_id(), transport);

        let headers = HeaderSet::from_header(Header::Description("finished".into()));
        let disconnect_fut = client.disconnect(headers);
        let mut disconnect_fut = pin!(disconnect_fut);
        exec.run_until_stalled(&mut disconnect_fut).expect_pending("waiting for response");

        // Expect the Disconnect request on the remote. The typical response is a positive `Ok`.
        let response_headers = HeaderSet::from_header(Header::Description("accepted".into()));
        let response = ResponsePacket::new(ResponseCode::Ok, vec![], response_headers.clone());
        expect_request_and_reply(&mut exec, &mut remote, expect_code(OpCode::Disconnect), response);

        let disconnect_result = exec
            .run_until_stalled(&mut disconnect_fut)
            .expect("received response")
            .expect("response is ok");
        assert_eq!(disconnect_result, response_headers);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    async fn disconnect_before_connect_error(transport: Transport) {
        let (client, _remote) = new_obex_client(ConnectionStatus::default(), transport);

        let headers = HeaderSet::from_header(Header::Description("finished".into()));
        let disconnect_result = client.disconnect(headers).await;
        assert_matches!(disconnect_result, Err(Error::OperationError { .. }))
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn disconnect_error_response_error(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (client, mut remote) = new_obex_client(ConnectionStatus::connected_no_id(), transport);

        let disconnect_fut = client.disconnect(HeaderSet::new());
        let mut disconnect_fut = pin!(disconnect_fut);
        exec.run_until_stalled(&mut disconnect_fut).expect_pending("waiting for response");

        // Expect the Disconnect request on the remote. An Error response still results in
        // disconnection.
        let response_headers = HeaderSet::from_header(Header::Description("accepted".into()));
        let response = ResponsePacket::new(
            ResponseCode::InternalServerError,
            vec![],
            response_headers.clone(),
        );
        expect_request_and_reply(&mut exec, &mut remote, expect_code(OpCode::Disconnect), response);

        let disconnect_result =
            exec.run_until_stalled(&mut disconnect_fut).expect("received response");
        assert_matches!(disconnect_result, Err(Error::PeerRejected { .. }));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn setpath_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (client, mut remote) = new_obex_client(ConnectionStatus::connected_no_id(), transport);

        let headers = HeaderSet::from_header(Header::name("myfolder"));
        let setpath_fut = client.set_path(SetPathFlags::empty(), headers);
        let mut setpath_fut = pin!(setpath_fut);
        exec.run_until_stalled(&mut setpath_fut).expect_pending("waiting for response");

        // Expect the SetPath request on the remote. The typical response is a positive `Ok`.
        let response_headers =
            HeaderSet::from_header(Header::Description("updated current folder".into()));
        let response = ResponsePacket::new(ResponseCode::Ok, vec![], response_headers.clone());
        let expectation = |request: RequestPacket| {
            assert_eq!(*request.code(), OpCode::SetPath);
            assert_eq!(request.data(), &[0, 0]);
        };
        expect_request_and_reply(&mut exec, &mut remote, expectation, response);

        let setpath_result = exec
            .run_until_stalled(&mut setpath_fut)
            .expect("received response")
            .expect("response is ok");
        assert_eq!(setpath_result, response_headers);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn setpath_error_response_is_error(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (client, mut remote) = new_obex_client(ConnectionStatus::connected_no_id(), transport);

        // Peer doesn't support SetPath.
        {
            let setpath_fut = client.set_path(SetPathFlags::BACKUP, HeaderSet::new());
            let mut setpath_fut = pin!(setpath_fut);
            exec.run_until_stalled(&mut setpath_fut).expect_pending("waiting for response");

            // Expect the SetPath request on the remote - peer doesn't support SetPath.
            let response_headers =
                HeaderSet::from_header(Header::Description("not implemented".into()));
            let response = ResponsePacket::new(ResponseCode::BadRequest, vec![], response_headers);
            expect_request_and_reply(
                &mut exec,
                &mut remote,
                expect_code(OpCode::SetPath),
                response,
            );

            let setpath_result =
                exec.run_until_stalled(&mut setpath_fut).expect("received response");
            assert_matches!(setpath_result, Err(Error::NotImplemented { .. }));
        }

        // Peer rejects SetPath.
        let headers = HeaderSet::from_header(Header::name("file"));
        let setpath_fut = client.set_path(SetPathFlags::DONT_CREATE, headers);
        let mut setpath_fut = pin!(setpath_fut);
        exec.run_until_stalled(&mut setpath_fut).expect_pending("waiting for response");

        // Expect the SetPath request on the remote - peer responds with error.
        let response_headers =
            HeaderSet::from_header(Header::Description("not implemented".into()));
        let response =
            ResponsePacket::new(ResponseCode::InternalServerError, vec![], response_headers);
        expect_request_and_reply(&mut exec, &mut remote, expect_code(OpCode::SetPath), response);

        let setpath_result = exec.run_until_stalled(&mut setpath_fut).expect("received response");
        assert_matches!(setpath_result, Err(Error::PeerRejected { .. }));
    }
}
