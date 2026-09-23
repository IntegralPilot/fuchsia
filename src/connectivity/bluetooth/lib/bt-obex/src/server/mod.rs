// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_bluetooth::types::Channel;
use futures::future::Future;
use futures::stream::StreamExt;
use log::{info, trace, warn};
use packet_encoding::{Decodable, Encodable};

use crate::error::{Error, PacketError};
use crate::header::{
    ConnectionIdentifier, Header, HeaderIdentifier, HeaderSet, SingleResponseMode,
};
use crate::operation::{OpCode, RequestPacket, ResponseCode, ResponsePacket, SetPathFlags};
pub use crate::transport::TransportType;
use crate::transport::max_packet_size_from_transport;
use futures::SinkExt;

/// Defines an interface for handling OBEX requests. All profiles & services should implement this
/// interface.
mod handler;
pub use handler::{ObexOperationError, ObexServerHandler, new_operation_error};

/// Implements the OBEX GET operation.
mod get;
use get::GetOperation;

/// Implements the OBEX PUT operation.
mod put;
use put::PutOperation;

/// Represents a request to be handled by the OBEX Server during a multi-step operation.
#[derive(Debug)]
pub enum OperationRequest {
    /// Request to send response packets to the remote peer.
    SendPackets(Vec<ResponsePacket>),
    /// Request to get informational headers describing a payload from the upper layer application
    /// -- occurs in a GET operation.
    GetApplicationInfo(HeaderSet),
    /// Request to get the payload from the upper layer application -- occurs in a GET operation.
    GetApplicationData(HeaderSet),
    /// Request to give the payload to the upper layer application -- occurs in a PUT operation.
    PutApplicationData(Vec<u8>, HeaderSet),
    /// Request to delete the payload in the upper layer application -- occurs in a PUT operation.
    DeleteApplicationData(HeaderSet),
    /// No action needed.
    None,
}

impl OperationRequest {
    pub fn single_response(packet: ResponsePacket) -> Self {
        Self::SendPackets(vec![packet])
    }
}

/// Represents a response from the upper layer application during a multi-step operation.
#[derive(Debug)]
pub enum ApplicationResponse {
    /// The application responded successfully to get GET information request by providing
    /// informational headers.
    GetInfo(HeaderSet),
    /// The application responded successfully to the GET request by providing the data payload
    /// and informational headers.
    GetData((Vec<u8>, HeaderSet)),
    /// The application responded successfully to the PUT request.
    Put,
}

impl ApplicationResponse {
    #[cfg(test)]
    fn accept_get(data: Vec<u8>, headers: HeaderSet) -> Result<Self, ObexOperationError> {
        Ok(ApplicationResponse::GetData((data, headers)))
    }

    #[cfg(test)]
    fn accept_get_info(headers: HeaderSet) -> Result<Self, ObexOperationError> {
        Ok(ApplicationResponse::GetInfo(headers))
    }

    #[cfg(test)]
    fn accept_put() -> Result<Self, ObexOperationError> {
        Ok(ApplicationResponse::Put)
    }
}

/// An interface for implementing a multi-step OBEX operation. Currently, the only two such
/// operations are GET and PUT.
/// See OBEX 1.5 Sections 3.4.3 & 3.4.4.
pub trait ServerOperation {
    /// Returns the current SRM mode of the operation.
    fn srm_status(&self) -> SingleResponseMode;

    /// Checks the provided `headers` for the SRM flag and returns the negotiated SRM mode if
    /// present, None otherwise.
    fn check_headers_for_srm(
        srm_supported_locally: bool,
        headers: &HeaderSet,
    ) -> Option<SingleResponseMode>
    where
        Self: Sized,
    {
        let Some(Header::SingleResponseMode(srm)) =
            headers.get(&HeaderIdentifier::SingleResponseMode)
        else {
            trace!("No SRM header in request");
            return None;
        };

        // If both parties support SRM, then it can be enabled.
        if srm_supported_locally && *srm == SingleResponseMode::Enable {
            Some(SingleResponseMode::Enable)
        } else {
            // Otherwise, either we don't support it locally, or the peer requested to disable it.
            Some(SingleResponseMode::Disable)
        }
    }

    /// Returns true if the operation is complete (e.g. all response packets have been sent).
    fn is_complete(&self) -> bool;

    /// Handle a `request` packet received from the OBEX client.
    /// Returns an `OperationRequest` to be handled by the OBEX server on success, Error if the
    /// request was invalid or couldn't be handled.
    fn handle_peer_request(&mut self, request: RequestPacket) -> Result<OperationRequest, Error>;

    /// Handle a response received from the upper layer application profile.
    /// `response` is Ok<T> if the application accepted the GET or PUT request.
    /// `response` is Err<E> if the application rejected the GET or PUT request.
    /// Returns response packets to be sent to the remote peer if the application `response` was
    /// successfully handled.
    /// Returns Error if there was an internal operation error.
    fn handle_application_response(
        &mut self,
        response: Result<ApplicationResponse, ObexOperationError>,
    ) -> Result<Vec<ResponsePacket>, Error>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum ConnectionStatus {
    /// The transport is created but the CONNECT operation has not been completed.
    #[default]
    Initialized,
    /// The transport is connected and the CONNECT operation has been completed.
    /// `id` contains the optional identifier for this connection. It is typically set when the
    /// OBEX client requests a directed OBEX connection to a specific Service by including the
    /// `Target` header in the CONNECT request.
    Connected { id: Option<ConnectionIdentifier> },
    /// The transport is connected but a DISCONNECT request has been received. The `ObexServer`
    /// will no longer process requests from the remote peer.
    DisconnectReceived,
}

impl ConnectionStatus {
    #[cfg(test)]
    fn connected_no_id() -> Self {
        Self::Connected { id: None }
    }
}

/// Implements the Server role for the OBEX protocol.
/// Provides an interface for receiving and responding to OBEX requests made by a remote OBEX client
/// service. Supports the operations defined in OBEX 1.5.
pub struct ObexServer {
    /// The current connection status of the server.
    connected: ConnectionStatus,
    /// The maximum OBEX packet length for this OBEX session.
    max_packet_size: u16,
    /// The active OBEX operation. The only two supported multi-step operations are GET and PUT.
    /// This is Some<T> when an operation is in progress, and None otherwise. There can only be one
    /// active multi-step operation. An operation is considered complete when
    /// `ServerOperation::is_complete` returns true.
    /// The active operation is cleaned up lazily -- when a request to start a new operation is
    /// received, the previously finished operation is removed.
    active_operation: Option<Box<dyn ServerOperation>>,
    /// The data channel that is used to read & write OBEX packets.
    channel: Channel,
    /// The type of transport used for the OBEX connection (RFCOMM or L2CAP).
    type_: TransportType,
    /// The handler provided by the application profile. This handler should implement the
    /// operations defined in OBEX 1.5 and will be used to provide a response to an incoming
    /// request made by the remote OBEX client.
    handler: Box<dyn ObexServerHandler>,
}

impl ObexServer {
    /// The default Connection Identifier used for directed OBEX connections.
    /// Because a single `ObexServer` services a single transport (L2CAP or RFCOMM), this
    /// identifier does not multiplex anything and is only sent in the CONNECT response.
    /// This value is arbitrarily chosen and will be included in all subsequent requests made by
    /// the remote OBEX Client.
    const DIRECTED_CONNECTION_ID: ConnectionIdentifier = ConnectionIdentifier(1);

    pub fn new(
        channel: Channel,
        type_: TransportType,
        handler: Box<dyn ObexServerHandler>,
    ) -> Self {
        let max_packet_size = max_packet_size_from_transport(channel.max_tx_size());
        Self {
            connected: ConnectionStatus::default(),
            max_packet_size,
            active_operation: None,
            channel,
            type_,
            handler,
        }
    }

    /// Returns `true` if the OBEX connection is currently active (e.g. CONNECT operation done).
    fn is_connected(&self) -> bool {
        matches!(self.connected, ConnectionStatus::Connected { .. })
    }

    fn set_connection_status(&mut self, status: ConnectionStatus) {
        self.connected = status;
    }

    fn set_max_packet_size(&mut self, peer_max_packet_size: u16) {
        // Use the smaller of the peer max and local max for maximum compatibility.
        let min_ = std::cmp::min(peer_max_packet_size, self.max_packet_size);
        self.max_packet_size = min_;
        trace!("Max packet size set to {}", self.max_packet_size);
    }

    /// Encodes and sends the OBEX `data` to the remote peer.
    /// Returns Error if the send operation could not be completed.
    async fn send(&mut self, data: impl Encodable<Error = PacketError>) -> Result<(), Error> {
        let mut buf = vec![0; data.encoded_len()];
        if buf.len() > self.max_packet_size as usize {
            return Err(Error::packet_too_large(buf.len(), self.max_packet_size as usize));
        }

        data.encode(&mut buf[..])?;
        self.channel.send(buf).await?;
        Ok(())
    }

    async fn connect_request(&mut self, request: RequestPacket) -> Result<ResponsePacket, Error> {
        // Parse the additional data first - the data length is already validated during decoding.
        let data = request.data();
        let version = data[0];
        let flags = data[1];
        let peer_max_packet_size = u16::from_be_bytes(data[2..4].try_into().unwrap());
        trace!(version, flags, peer_max_packet_size; "Additional data in CONNECT request");
        self.set_max_packet_size(peer_max_packet_size);

        let headers = HeaderSet::from(request);

        // The connection can optionally be considered "directed" if the Client provides a Target
        // UUID identifying the service.
        let id = if headers.contains_header(&HeaderIdentifier::Target) {
            Some(Self::DIRECTED_CONNECTION_ID)
        } else {
            None
        };
        let (code, response_headers) = match self.handler.connect(headers).await {
            Ok(mut headers) => {
                trace!("Application accepted CONNECT request");
                let _ = headers.try_add_connection_id(&id);
                self.set_connection_status(ConnectionStatus::Connected { id });
                (ResponseCode::Ok, headers)
            }
            Err(reject_parameters) => {
                trace!("Application rejected CONNECT request");
                reject_parameters
            }
        };
        let response_packet =
            ResponsePacket::new_connect(code, self.max_packet_size, response_headers);
        Ok(response_packet)
    }

    /// Handles a Disconnect request made by the remote OBEX client.
    /// Returns a response packet to be sent on success, Error if the request couldn't be handled.
    async fn disconnect_request(
        &mut self,
        request: RequestPacket,
    ) -> Result<ResponsePacket, Error> {
        let headers = HeaderSet::from(request);
        let response_headers = self.handler.disconnect(headers).await;
        let response_packet = ResponsePacket::new_disconnect(response_headers);
        self.set_connection_status(ConnectionStatus::DisconnectReceived);
        Ok(response_packet)
    }

    /// Handles a SetPath request made by the remote OBEX client.
    /// Returns a response packet to be sent on success, Error if the request couldn't be handled.
    async fn setpath_request(&mut self, request: RequestPacket) -> Result<ResponsePacket, Error> {
        if !self.is_connected() {
            return Err(Error::operation(OpCode::SetPath, "CONNECT not completed"));
        }
        // Parse the additional data first - the data length is already validated during decoding.
        // Only the `flags` field is used in OBEX 1.5. `constants` is RFA.
        let data = request.data();
        let flags = SetPathFlags::from_bits_truncate(data[0]);
        let backup = flags.contains(SetPathFlags::BACKUP);
        let create = !flags.contains(SetPathFlags::DONT_CREATE);

        let headers = HeaderSet::from(request);
        let (code, response_headers) = match self.handler.set_path(headers, backup, create).await {
            Ok(headers) => {
                trace!("Application accepted SETPATH request");
                (ResponseCode::Ok, headers)
            }
            Err(reject_parameters) => {
                trace!("Application rejected SETPATH request");
                reject_parameters
            }
        };
        let response_packet = ResponsePacket::new_setpath(code, response_headers);
        Ok(response_packet)
    }

    /// Potentially initializes a new multi-step operation.
    /// Returns true if a new operation was initialized, false otherwise.
    fn maybe_start_new_operation(&mut self, code: &OpCode) -> bool {
        if self.active_operation.as_ref().is_some_and(|o| !o.is_complete()) {
            return false;
        }

        let op: Box<dyn ServerOperation> = match code {
            OpCode::Get | OpCode::GetFinal => {
                Box::new(GetOperation::new(self.max_packet_size, self.type_.srm_supported()))
            }
            OpCode::Put | OpCode::PutFinal => {
                Box::new(PutOperation::new(self.type_.srm_supported()))
            }
            _ => unreachable!("only called from `Self::multistep_request`"),
        };
        trace!("Started new operation ({code:?})");
        self.active_operation = Some(op);
        return true;
    }

    /// Handles a request made by the remote OBEX client for a potentially multi-step
    /// operation (PUT or GET).
    /// Returns response packets to be sent to the peer on success, Error if the request can't
    /// be handled or is invalid.
    async fn multistep_request(
        &mut self,
        request: RequestPacket,
    ) -> Result<Vec<ResponsePacket>, Error> {
        let _ = self.maybe_start_new_operation(request.code());
        let operation = self.active_operation.as_mut().expect("just initialized");

        let application_response = match operation.handle_peer_request(request) {
            Ok(OperationRequest::SendPackets(responses)) => return Ok(responses),
            Ok(OperationRequest::GetApplicationInfo(info_headers)) => {
                self.handler.get_info(info_headers).await.map(|x| ApplicationResponse::GetInfo(x))
            }
            Ok(OperationRequest::GetApplicationData(request_headers)) => self
                .handler
                .get_data(request_headers)
                .await
                .map(|x| ApplicationResponse::GetData(x)),
            Ok(OperationRequest::PutApplicationData(data, request_headers)) => {
                self.handler.put(data, request_headers).await.map(|_| ApplicationResponse::Put)
            }
            Ok(OperationRequest::DeleteApplicationData(request_headers)) => {
                self.handler.delete(request_headers).await.map(|_| ApplicationResponse::Put)
            }
            Ok(OperationRequest::None) => return Ok(vec![]),
            Err(e) => {
                warn!("Internal error in operation: {e:?}");
                return Ok(vec![ResponsePacket::new_no_data(
                    ResponseCode::InternalServerError,
                    HeaderSet::new(),
                )]);
            }
        };

        operation.handle_application_response(application_response)
    }

    /// Processes a raw data `packet` received from the remote peer acting as an OBEX client.
    /// Returns a list of `ResponsePacket`s to be sent to the peer on success, Error if the request
    /// was invalid or couldn't be handled.
    async fn receive_packet(&mut self, packet: Vec<u8>) -> Result<Vec<ResponsePacket>, Error> {
        if packet.len() > self.max_packet_size as usize {
            warn!(
                "Received packet size ({}) exceeds negotiated max_packet_size ({})",
                packet.len(),
                self.max_packet_size
            );
            return Ok(vec![ResponsePacket::new_no_data(
                ResponseCode::BadRequest,
                HeaderSet::new(),
            )]);
        }

        let decoded = RequestPacket::decode(&packet[..])?;
        trace!(packet:? = decoded; "Received request from OBEX client");
        let response = match decoded.code() {
            OpCode::Connect => self.connect_request(decoded).await?,
            OpCode::Disconnect => self.disconnect_request(decoded).await?,
            OpCode::SetPath => self.setpath_request(decoded).await?,
            OpCode::Put | OpCode::PutFinal | OpCode::Get | OpCode::GetFinal => {
                return self.multistep_request(decoded).await;
            }
            _code => todo!("Support other OBEX requests"),
        };
        Ok(vec![response])
    }

    pub fn run(mut self) -> impl Future<Output = Result<(), Error>> {
        async move {
            while let Some(packet) = self.channel.next().await {
                match packet {
                    Ok(bytes) => {
                        let responses = self.receive_packet(bytes).await?;
                        for response in responses {
                            self.send(response).await?;
                        }

                        // The OBEX Client requested to close the OBEX connection.
                        if self.connected == ConnectionStatus::DisconnectReceived {
                            trace!("Disconnect request - closing transport");
                            return Ok(());
                        }
                    }
                    Err(e) => warn!("Error reading data from transport: {e:?}"),
                }
            }
            info!("Peer disconnected transport");
            Ok(())
        }
    }
}

#[cfg(test)]
pub(crate) mod test_utils {
    use super::*;

    #[track_caller]
    pub fn expect_single_packet(request: OperationRequest) -> ResponsePacket {
        let OperationRequest::SendPackets(mut packets) = request else {
            panic!("Expected outgoing packet request, got: {request:?}");
        };
        assert_eq!(packets.len(), 1);
        packets.pop().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_channel_test_support::{Transport, create_test_channels};
    use test_case::test_case;

    use assert_matches::assert_matches;
    use async_test_helpers::expect_stream_pending;
    use async_utils::PollExt;
    use fuchsia_async as fasync;
    use std::pin::pin;

    use crate::header::header_set::{expect_body, expect_end_of_body};
    use crate::server::handler::test_utils::TestApplicationProfile;
    use crate::transport::test_utils::{expect_response, send_packet};

    /// Returns an ObexServer, a test only object representing an upper layer profile, & the remote
    /// peer's side of the transport.
    fn new_obex_server(
        transport: Transport,
        srm: bool,
    ) -> (ObexServer, TestApplicationProfile, Channel) {
        let (local, remote) = create_test_channels(transport);
        let app = TestApplicationProfile::new();
        let type_ = if srm { TransportType::L2cap } else { TransportType::Rfcomm };
        let obex_server = ObexServer::new(local, type_, Box::new(app.clone()));
        (obex_server, app, remote)
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn obex_server_terminates_when_channel_closes(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (obex_server, _test_app, remote) = new_obex_server(transport, /*srm=*/ false);

        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server still active");

        drop(remote);
        let result = exec.run_until_stalled(&mut server_fut).expect("server finished");
        assert_matches!(result, Ok(_));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn connect_accepted_by_app_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (obex_server, test_app, mut remote) = new_obex_server(transport, /*srm=*/ false);
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        let connect_request = RequestPacket::new_connect(500, HeaderSet::new());
        send_packet(&mut exec, &mut remote, connect_request);

        // Expect the ObexServer to receive the request, parse it, ask the application, and reply.
        // Simulate application accepting the request.
        let headers = HeaderSet::from_header(Header::Description("foo".into()));
        test_app.set_response(Ok(headers));
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        // Expect the remote peer to receive our CONNECT response. Our response shouldn't contain
        // a `ConnectionIdentifier` since the request didn't contain a `Target` header.
        let expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Ok);
            assert_eq!(response.data(), &[0x10, 0, 0x01, 0xf4]);
            assert!(response.headers().contains_header(&HeaderIdentifier::Description));
            assert!(!response.headers().contains_header(&HeaderIdentifier::ConnectionId));
        };
        expect_response(&mut exec, &mut remote, expectation, OpCode::Connect);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn directed_connect_accepted_by_app_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (obex_server, test_app, mut remote) = new_obex_server(transport, /*srm=*/ false);
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        let request_headers = HeaderSet::from_header(Header::Target(vec![5, 6]));
        let connect_request = RequestPacket::new_connect(500, request_headers);
        send_packet(&mut exec, &mut remote, connect_request);

        // Expect the ObexServer to receive the request, parse it, ask the application, and reply.
        // Simulate application accepting the request.
        let headers = HeaderSet::from_header(Header::name("foo"));
        test_app.set_response(Ok(headers));
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        // Expect the remote peer to receive our CONNECT response. Our response should contain a
        // connection identifier because the `request_headers` contains a `Target` header.
        let expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Ok);
            assert_eq!(response.data(), &[0x10, 0, 0x01, 0xf4]);
            assert!(response.headers().contains_header(&HeaderIdentifier::Name));
            assert!(response.headers().contains_header(&HeaderIdentifier::ConnectionId));
        };
        expect_response(&mut exec, &mut remote, expectation, OpCode::Connect);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn connect_rejected_by_app_is_ok(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (obex_server, test_app, mut remote) = new_obex_server(transport, /*srm=*/ false);
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        let connect_request = RequestPacket::new_connect(255, HeaderSet::new());
        send_packet(&mut exec, &mut remote, connect_request);

        // The ObexServer should receive the request and hand it to the profile - profile rejects.
        test_app.set_response(Err((ResponseCode::Forbidden, HeaderSet::new())));
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        // Expect the remote peer to receive our negative CONNECT response.
        let expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Forbidden);
            assert_eq!(response.data(), &[0x10, 0, 0x00, 0xff]);
            let headers = HeaderSet::from(response);
            assert!(headers.is_empty());
        };
        expect_response(&mut exec, &mut remote, expectation, OpCode::Connect);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn invalid_connect_request_is_error(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (obex_server, _test_app, mut remote) = new_obex_server(transport, /*srm=*/ false);

        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server still active");

        // Invalid CONNECT request. Missing the 2 byte max packet size field.
        let bytes = [0x80, 0x00, 0x05, 0x00, 0x00];
        let mut fut = remote.send(bytes.to_vec());
        exec.run_until_stalled(&mut fut).expect("can send data").expect("write success");

        let result = exec.run_until_stalled(&mut server_fut).expect("terminate due to error");
        assert_matches!(result, Err(Error::Packet(_)));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn peer_disconnect_request_terminates_server(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (obex_server, test_app, mut remote) = new_obex_server(transport, /*srm=*/ false);
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        let headers = HeaderSet::from_header(Header::Description("done".into()));
        let disconnect_request = RequestPacket::new_disconnect(headers);
        send_packet(&mut exec, &mut remote, disconnect_request);

        // Expect the ObexServer to receive the request, parse it, get response headers from the
        // application, and reply. Because this is a Disconnect request, the server run loop
        // should finish.
        let headers = HeaderSet::from_header(Header::Description("disconnecting".into()));
        test_app.set_response(Ok(headers));
        let result =
            exec.run_until_stalled(&mut server_fut).expect("server terminated from disconnect");
        assert_matches!(result, Ok(_));

        // Expect the remote peer to receive our DISCONNECT response.
        let expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Ok);
            let headers = HeaderSet::from(response);
            assert!(headers.contains_header(&HeaderIdentifier::Description));
        };
        expect_response(&mut exec, &mut remote, expectation, OpCode::Disconnect);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn setpath_request_accepted_by_app_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut obex_server, test_app, mut remote) =
            new_obex_server(transport, /*srm=*/ false);
        // Set to the Connected state to bypass CONNECT operation.
        obex_server.set_connection_status(ConnectionStatus::connected_no_id());
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        let headers = HeaderSet::from_header(Header::name("folder1"));
        let setpath_request =
            RequestPacket::new_set_path(SetPathFlags::all(), headers).expect("valid request");
        send_packet(&mut exec, &mut remote, setpath_request);

        // The ObexServer should receive the request and hand it to the profile - profile accepts.
        test_app.set_response(Ok(HeaderSet::new()));
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        // Expect the remote peer to receive our SETPATH response.
        let expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Ok);
            let headers = HeaderSet::from(response);
            assert!(headers.is_empty());
        };
        expect_response(&mut exec, &mut remote, expectation, OpCode::SetPath);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn setpath_request_rejected_by_app_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut obex_server, test_app, mut remote) =
            new_obex_server(transport, /*srm=*/ false);
        // Set to the Connected state to bypass CONNECT operation.
        obex_server.set_connection_status(ConnectionStatus::connected_no_id());
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        let setpath_request = RequestPacket::new_set_path(SetPathFlags::BACKUP, HeaderSet::new())
            .expect("valid request");
        send_packet(&mut exec, &mut remote, setpath_request);

        // The ObexServer should receive the request and hand it to the profile - profile rejects.
        test_app.set_response(Err((ResponseCode::Forbidden, HeaderSet::new())));
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        // Expect the remote peer to receive our negative SETPATH response.
        let expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Forbidden);
            let headers = HeaderSet::from(response);
            assert!(headers.is_empty());
        };
        expect_response(&mut exec, &mut remote, expectation, OpCode::SetPath);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn setpath_request_before_connect_is_error(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (obex_server, _test_app, mut remote) = new_obex_server(transport, /*srm=*/ false);
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        let setpath_request = RequestPacket::new_set_path(SetPathFlags::BACKUP, HeaderSet::new())
            .expect("valid request");
        send_packet(&mut exec, &mut remote, setpath_request);
        let result = exec
            .run_until_stalled(&mut server_fut)
            .expect("server terminated from invalid setpath");
        assert_matches!(result, Err(Error::OperationError { operation: OpCode::SetPath, .. }));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn get_request_accepted_by_app_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut obex_server, test_app, mut remote) =
            new_obex_server(transport, /*srm=*/ false);
        // Set to the Connected state to bypass CONNECT operation.
        obex_server.set_connection_status(ConnectionStatus::connected_no_id());
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        // Remote asks for information about the payload. The ObexServer should receive the request
        // and ask the application for the response.
        let get_request1 =
            RequestPacket::new_get(HeaderSet::from_header(Header::name("random object")));
        send_packet(&mut exec, &mut remote, get_request1);
        // Simulate the application responding with the size of the object.
        test_app.set_response(Ok(HeaderSet::from_header(Header::Length(0x10))));
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        // The remote peer should receive the info response.
        let expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Continue);
            assert!(response.headers().contains_header(&HeaderIdentifier::Length));
        };
        expect_response(&mut exec, &mut remote, expectation, OpCode::Get);

        // Remote sends a GET_FINAL request indicating that it is ready to receive the data payload.
        let get_request2 = RequestPacket::new_get_final(HeaderSet::new());
        send_packet(&mut exec, &mut remote, get_request2);

        // The ObexServer should receive the request and hand it to the profile. Set the profile
        // handler to return a static buffer.
        let application_response_buf = vec![1, 2, 3, 4, 5, 6];
        let response_headers = HeaderSet::from_header(Header::Description("foo".into()));
        test_app.set_get_response((application_response_buf.clone(), response_headers));
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        let expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Ok);
            let mut headers = HeaderSet::from(response);
            assert!(headers.contains_header(&HeaderIdentifier::Description));
            let received_body = headers.remove_body(/*final_=*/ true).expect("contains body");
            assert_eq!(received_body, application_response_buf);
        };
        expect_response(&mut exec, &mut remote, expectation, OpCode::GetFinal);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn get_request_rejected_by_app_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut obex_server, _test_app, mut remote) =
            new_obex_server(transport, /*srm=*/ false);
        obex_server.set_connection_status(ConnectionStatus::connected_no_id());
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        // Send an example GET_FINAL request with a header describing the name of the object.
        let headers = HeaderSet::from_header(Header::name("random object123"));
        let get_request = RequestPacket::new_get_final(headers);
        send_packet(&mut exec, &mut remote, get_request);

        // The ObexServer receives request and hands to application. By default, it rejects the
        // request.
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");
        // Expect the peer to received the rejection code.
        let expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::NotImplemented);
            assert!(response.headers().is_empty());
        };
        expect_response(&mut exec, &mut remote, expectation, OpCode::GetFinal);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn get_request_with_srm_enabled_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut obex_server, test_app, mut remote) =
            new_obex_server(transport, /*srm=*/ true);
        obex_server.set_connection_status(ConnectionStatus::connected_no_id());
        obex_server.set_max_packet_size(20); // Set max to something small.
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        // First request asks for information & SRM. Server should receive the request, ask the
        // application, and negotiate SRM.
        let headers1 =
            HeaderSet::from_headers(vec![Header::name("a"), SingleResponseMode::Enable.into()])
                .unwrap();
        let get_request1 = RequestPacket::new_get(headers1);
        send_packet(&mut exec, &mut remote, get_request1);
        test_app.set_response(Ok(HeaderSet::from_header(Header::Length(0x20))));
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");
        // Expect the response packet to the peer to negotiate SRM and contain the application
        // response.
        let expectation1 = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Continue);
            let Header::SingleResponseMode(SingleResponseMode::Enable) =
                response.headers().get(&HeaderIdentifier::SingleResponseMode).unwrap()
            else {
                panic!("Expected SRM enable in response");
            };
            assert!(response.headers().contains_header(&HeaderIdentifier::Length));
        };
        expect_response(&mut exec, &mut remote, expectation1, OpCode::Get);
        // At this point, SRM is considered active for the operation.

        // Second (final) request to get the payload.
        let get_request2 = RequestPacket::new_get_final(HeaderSet::new());
        send_packet(&mut exec, &mut remote, get_request2);
        // The ObexServer should receive the request and hand it to the profile. Set the profile
        // handler to return a static buffer that must be split across multiple payloads.
        let application_response_buf = (0..50).collect::<Vec<u8>>();
        test_app.set_get_response((application_response_buf, HeaderSet::new()));
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        // Since SRM is enabled, we expect consecutive packets containing the payload - no requests
        // made by the remote in between.
        let expected_bufs = vec![
            (0..14).collect::<Vec<u8>>(),
            (14..28).collect::<Vec<u8>>(),
            (28..42).collect::<Vec<u8>>(),
        ];
        for expected_buf in expected_bufs {
            let expectation = |response: ResponsePacket| {
                assert_eq!(*response.code(), ResponseCode::Continue);
                expect_body(response.headers(), expected_buf);
            };
            expect_response(&mut exec, &mut remote, expectation, OpCode::Get);
        }

        // Final packet has remaining bytes and operation is complete.
        let final_expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Ok);
            expect_end_of_body(response.headers(), (42..50).collect::<Vec<u8>>());
        };
        expect_response(&mut exec, &mut remote, final_expectation, OpCode::GetFinal);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn put_request_accepted_by_app_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut obex_server, test_app, mut remote) =
            new_obex_server(transport, /*srm=*/ false);
        // Set to the Connected state to bypass CONNECT operation.
        obex_server.set_connection_status(ConnectionStatus::connected_no_id());
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        let headers = HeaderSet::from_headers(vec![
            Header::name("random object"),
            Header::EndOfBody(vec![1, 2, 3, 4, 5]),
        ])
        .unwrap();
        let put_request = RequestPacket::new_put_final(headers);
        send_packet(&mut exec, &mut remote, put_request);

        // The ObexServer should receive the request and hand it to the profile. Profile accepts.
        test_app.set_put_response(Ok(()));
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");
        // Verify profile received correct data.
        let (rec_data, rec_headers) = test_app.put_data();
        assert_eq!(rec_data, vec![1, 2, 3, 4, 5]);
        assert!(rec_headers.contains_header(&HeaderIdentifier::Name));

        let expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Ok);
            assert!(response.headers().is_empty());
        };
        expect_response(&mut exec, &mut remote, expectation, OpCode::PutFinal);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn put_request_with_srm_enabled_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut obex_server, test_app, mut remote) =
            new_obex_server(transport, /*srm=*/ true);
        obex_server.set_connection_status(ConnectionStatus::connected_no_id());
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        // First request asks to enable SRM and provides some info.
        let headers1 = HeaderSet::from_headers(vec![
            Header::name("my file"),
            SingleResponseMode::Enable.into(),
        ])
        .unwrap();
        let put_request1 = RequestPacket::new_put(headers1);
        send_packet(&mut exec, &mut remote, put_request1);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");
        // Expect the Obex Server to positively respond, and enable SRM. Subsequent requests won't
        // receive a response.
        let expectation1 = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Continue);
            let Header::SingleResponseMode(SingleResponseMode::Enable) =
                response.headers().get(&HeaderIdentifier::SingleResponseMode).unwrap()
            else {
                panic!("Expected SRM enable in response");
            };
        };
        expect_response(&mut exec, &mut remote, expectation1, OpCode::Put);

        // Next request sends over some data. Don't expect any response on the remote.
        let headers2 = HeaderSet::from_header(Header::Body(vec![1, 2, 3, 4, 5]));
        let put_request2 = RequestPacket::new_put(headers2);
        send_packet(&mut exec, &mut remote, put_request2);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");
        expect_stream_pending(&mut exec, &mut remote);

        // Next (final) request sends over some data. Expect a response since this is the final
        // packet.
        let headers3 = HeaderSet::from_header(Header::EndOfBody(vec![6, 7, 8, 9, 10]));
        let put_request3 = RequestPacket::new_put_final(headers3);
        send_packet(&mut exec, &mut remote, put_request3);

        // The entire request is complete and the Obex Server should hand it to the application.
        // Verify profile received correct data.
        test_app.set_put_response(Ok(()));
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");
        let (rec_data, rec_headers) = test_app.put_data();
        assert_eq!(rec_data, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert!(rec_headers.contains_header(&HeaderIdentifier::Name));

        let expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Ok);
            assert!(response.headers().is_empty());
        };
        expect_response(&mut exec, &mut remote, expectation, OpCode::PutFinal);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn delete_request_accepted_by_app_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut obex_server, test_app, mut remote) =
            new_obex_server(transport, /*srm=*/ false);
        obex_server.set_connection_status(ConnectionStatus::connected_no_id());
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        let headers = HeaderSet::from_header(Header::name("foo.txt"));
        let put_request = RequestPacket::new_put_final(headers);
        send_packet(&mut exec, &mut remote, put_request);

        // The ObexServer should receive the request and hand it to the profile. Profile accepts.
        test_app.set_put_response(Ok(()));
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        let expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::Ok);
            assert!(response.headers().is_empty());
        };
        expect_response(&mut exec, &mut remote, expectation, OpCode::PutFinal);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn receive_packet_exceeds_max_packet_size_rejected(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut obex_server, _test_app, mut remote) =
            new_obex_server(transport, /*srm=*/ false);
        obex_server.set_connection_status(ConnectionStatus::connected_no_id());
        // The default max_packet_size before CONNECT is clamped to
        // MIN_MAX_PACKET_SIZE (255) minimum. Set max_packet_size explicitly to
        // 255.
        obex_server.set_max_packet_size(255);
        let server_fut = obex_server.run();
        let mut server_fut = pin!(server_fut);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        let mut oversized_buf = vec![0u8; 300];
        oversized_buf[0] = (&OpCode::Put).into();
        oversized_buf[1..3].copy_from_slice(&(300u16).to_be_bytes());

        let mut fut = remote.send(oversized_buf);
        exec.run_until_stalled(&mut fut).expect("write to channel success").expect("write success");
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server active");

        // The ObexServer should reject the oversized packet with BadRequest (0xC0).
        let expectation = |response: ResponsePacket| {
            assert_eq!(*response.code(), ResponseCode::BadRequest);
        };
        expect_response(&mut exec, &mut remote, expectation, OpCode::Put);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    async fn send_exceeds_max_packet_size_is_error(transport: Transport) {
        let (mut obex_server, _test_app, _remote) = new_obex_server(transport, /*srm=*/ false);
        obex_server.set_max_packet_size(255);

        // Attempt to send a response packet whose encoded length exceeds 255
        // bytes.
        let large_header = Header::Description("A".repeat(300).into());
        let response =
            ResponsePacket::new_no_data(ResponseCode::Ok, HeaderSet::from_header(large_header));

        let send_result = obex_server.send(response).await;
        assert_matches!(send_result, Err(Error::PacketTooLarge { size: 608, max: 255 }));
    }
}
