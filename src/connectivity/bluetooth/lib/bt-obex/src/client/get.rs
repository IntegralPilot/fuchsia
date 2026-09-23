// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::SinkExt;
use log::trace;

use crate::client::SrmOperation;
use crate::error::Error;
use crate::header::{HeaderSet, SingleResponseMode};
use crate::operation::{OpCode, RequestPacket, ResponseCode, ResponsePacket};
use crate::transport::ObexTransport;

/// Represents an in-progress GET Operation.
///
/// Example Usage:
/// ```
/// let obex_client = ObexClient::new(..);
/// let get_operation = obex_client.get()?;
/// // Optionally make request for information about the payload.
/// let initial_headers: HeaderSet = ...;
/// let received_headers = get_operation.get_information(initial_headers).await?;
/// // Process `received_headers` and make a subsequent request for more information.
/// let additional_headers: HeaderSet = ...;
/// let additional_received_headers = get_operation.get_information(additional_headers).await?;
/// // Make the "final" request to get data.
/// let body = get_operation.get_data(HeaderSet::new()).await?;
/// // Process `body` - `get_operation` is consumed and the operation is assumed to be complete.
/// ```
#[must_use]
#[derive(Debug)]
pub struct GetOperation<'a> {
    /// The L2CAP or RFCOMM connection to the remote peer.
    transport: ObexTransport<'a>,
    /// The initial set of headers used in the GET operation. This is Some<T> when `is_started` is
    /// false, and None when `is_started` is true.
    headers: Option<HeaderSet>,
    /// Whether the operation is in-progress or not - one or more GET requests have been made.
    is_started: bool,
    /// The status of SRM for this operation. By default, SRM will be enabled if the transport
    /// supports it. However, it may be disabled if the peer requests to disable it.
    srm: SingleResponseMode,
}

impl<'a> GetOperation<'a> {
    pub fn new(headers: HeaderSet, transport: ObexTransport<'a>) -> Self {
        let srm = transport.srm_supported().into();
        Self { transport, headers: Some(headers), is_started: false, srm }
    }

    fn set_started(&mut self) {
        let _ = self.headers.take().unwrap();
        self.is_started = true;
    }

    /// Attempts to update the `application_headers` if the operation hasn't started.
    /// Returns Ok on a successful update or if the operation has already started, Error otherwise.
    fn update_headers_before_start(
        &mut self,
        application_headers: &mut HeaderSet,
    ) -> Result<(), Error> {
        if self.is_started {
            return Ok(());
        }
        // Combine the set of application headers with any initial headers.
        let initial = self.headers.replace(HeaderSet::new()).unwrap();
        application_headers.try_append(initial)?;
        // Try to enable SRM since this is the first packet of the operation.
        self.try_enable_srm(application_headers)?;
        Ok(())
    }

    /// Processes the `response` to a GET request and returns the set of headers on success, Error
    /// otherwise.
    fn handle_get_response(response: ResponsePacket) -> Result<HeaderSet, Error> {
        response.expect_code(OpCode::Get, ResponseCode::Continue).map(Into::into)
    }

    /// Processes the `response` to a GETFINAL request and returns a flag indicating the
    /// terminal user data payload, responder headers, and the user data payload.
    fn handle_get_final_response(
        response: ResponsePacket,
    ) -> Result<(bool, HeaderSet, Vec<u8>), Error> {
        // If the response code is OK then this is the final body packet of the GET request.
        if *response.code() == ResponseCode::Ok {
            // The EndOfBody Header contains the user data.
            let mut headers = HeaderSet::from(response);
            return headers.remove_body(true).map(|eob| (true, headers, eob));
        }

        // Otherwise, a Continue means there are more body packets left.
        let mut headers =
            response.expect_code(OpCode::GetFinal, ResponseCode::Continue).map(HeaderSet::from)?;
        // The Body Header contains the user data.
        headers.remove_body(false).map(|b| (false, headers, b))
    }

    /// Request information about the payload.
    /// Returns the informational headers from the peer response on success, Error otherwise.
    ///
    /// Only one such request can be outstanding at a time. `headers` must be nonempty. If no
    /// additional information is needed, use `GetOperation::get_data` instead.
    pub async fn get_information(&mut self, mut headers: HeaderSet) -> Result<HeaderSet, Error> {
        // Potentially add initial headers if this is the first request in the operation.
        self.update_headers_before_start(&mut headers)?;

        // For non-final requests, we expect at least one informational header to be sent.
        if headers.is_empty() {
            return Err(Error::operation(OpCode::Get, "missing headers"));
        }

        // SRM is considered active if this is a subsequent GET request & the transport supports it.
        let srm_active = self.is_started && self.get_srm() == SingleResponseMode::Enable;

        let request = RequestPacket::new_get(headers);
        trace!(request:?; "Making outgoing GET request");
        self.transport.send(request).await?;
        trace!("Successfully made GET request");

        // Only expect a response if SRM is inactive.
        let response_headers = if !srm_active {
            let response = self.transport.receive_response(OpCode::Get).await?;
            Self::handle_get_response(response)?
        } else {
            HeaderSet::new()
        };
        if !self.is_started {
            self.check_response_for_srm(&response_headers);
            self.set_started();
        }
        Ok(response_headers)
    }

    /// Request the user data payload from the remote OBEX server.
    /// Returns the byte payload on success, Error otherwise.
    ///
    /// The GET operation is considered complete after this.
    pub async fn get_data(mut self, mut headers: HeaderSet) -> Result<Vec<u8>, Error> {
        // Potentially add initial headers if this is the first request in the operation.
        self.update_headers_before_start(&mut headers)?;

        let mut request = RequestPacket::new_get_final(headers);
        let mut first_request = true;
        let mut body = vec![];
        loop {
            // We always make an initial `GetFinal` request. Subsequent requests are only made if
            // SRM is disabled.
            if first_request || self.srm != SingleResponseMode::Enable {
                trace!(request:?; "Making outgoing GET final request");
                self.transport.send(request.clone()).await?;
                trace!("Successfully made GET final request");
                // All subsequent GetFinal requests will not contain any headers.
                request = RequestPacket::new_get_final(HeaderSet::new());
                first_request = false;
            }
            let response = self.transport.receive_response(OpCode::GetFinal).await?;
            let (final_packet, response_headers, mut response_body) =
                Self::handle_get_final_response(response)?;
            body.append(&mut response_body);

            // Check to see if peer wants SRM for this single-packet request.
            if !self.is_started {
                self.check_response_for_srm(&response_headers);
                self.set_started();
            }

            if final_packet {
                trace!("Found terminal GET final packet");
                break;
            }
        }
        Ok(body)
    }

    /// Request to terminate a multi-packet GET request early.
    /// Returns the informational headers from the peer response on success, Error otherwise.
    /// If Error is returned, there are no guarantees about the synchronization between the local
    /// OBEX client and remote OBEX server.
    pub async fn terminate(mut self, headers: HeaderSet) -> Result<HeaderSet, Error> {
        let opcode = OpCode::Abort;
        if !self.is_started {
            return Err(Error::operation(opcode, "can't abort when not started"));
        }

        let request = RequestPacket::new_abort(headers);
        trace!(request:?; "Making outgoing {opcode:?} request");
        self.transport.send(request).await?;
        trace!("Successfully made {opcode:?} request");
        let response = self.transport.receive_response(opcode).await?;
        response.expect_code(opcode, ResponseCode::Ok).map(Into::into)
    }
}

impl SrmOperation for GetOperation<'_> {
    const OPERATION_TYPE: OpCode = OpCode::Get;

    fn get_srm(&self) -> SingleResponseMode {
        self.srm
    }

    fn set_srm(&mut self, mode: SingleResponseMode) {
        self.srm = mode;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_channel_test_support::Transport;
    use test_case::test_case;

    use assert_matches::assert_matches;
    use async_test_helpers::expect_stream_pending;
    use async_utils::PollExt;
    use fuchsia_async as fasync;
    use std::pin::pin;

    use crate::error::PacketError;
    use crate::header::{Header, HeaderIdentifier};
    use crate::transport::ObexTransportManager;
    use crate::transport::test_utils::{
        expect_code, expect_request, expect_request_and_reply, new_manager, reply,
    };

    fn setup_get_operation(mgr: &ObexTransportManager, initial: HeaderSet) -> GetOperation<'_> {
        let transport = mgr.try_new_operation().expect("can start operation");
        GetOperation::new(initial, transport)
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn get_operation(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (manager, mut remote) = new_manager(transport, /* srm_supported */ false);
        let mut operation = setup_get_operation(&manager, HeaderSet::new());
        assert!(!operation.is_started);

        // The initial GET request should succeed and return the set of Headers specified by the
        // peer.
        {
            let info_headers = HeaderSet::from_header(Header::name("text"));
            let info_fut = operation.get_information(info_headers);
            let mut info_fut = pin!(info_fut);
            exec.run_until_stalled(&mut info_fut).expect_pending("waiting for peer response");
            let response_headers = HeaderSet::from_header(Header::name("bar"));
            let response = ResponsePacket::new_no_data(ResponseCode::Continue, response_headers);
            let expectation = |request: RequestPacket| {
                assert_eq!(*request.code(), OpCode::Get);
                let headers = HeaderSet::from(request);
                assert!(headers.contains_header(&HeaderIdentifier::Name));
            };
            expect_request_and_reply(&mut exec, &mut remote, expectation, response);
            let received_headers = exec
                .run_until_stalled(&mut info_fut)
                .expect("response received")
                .expect("valid response");
            assert!(received_headers.contains_header(&HeaderIdentifier::Name));
        }
        assert!(operation.is_started);

        // A subsequent request for more information should return the set of additional Headers
        // specified by the peer.
        {
            let info_headers = HeaderSet::from_header(Header::Type("file".into()));
            let info_fut = operation.get_information(info_headers);
            let mut info_fut = pin!(info_fut);
            exec.run_until_stalled(&mut info_fut).expect_pending("waiting for peer response");
            let response_headers = HeaderSet::from_header(Header::Description("big file".into()));
            let response = ResponsePacket::new_no_data(ResponseCode::Continue, response_headers);
            expect_request_and_reply(&mut exec, &mut remote, expect_code(OpCode::Get), response);
            let received_headers = exec
                .run_until_stalled(&mut info_fut)
                .expect("response received")
                .expect("valid response");
            assert!(received_headers.contains_header(&HeaderIdentifier::Description));
        }

        // The multi-packet user payload should be returned at the end of the operation. Because
        // the operation is taken by value, it is considered complete after this step and resources
        // are freed.
        let data_fut = operation.get_data(HeaderSet::new());
        let mut data_fut = pin!(data_fut);
        exec.run_until_stalled(&mut data_fut).expect_pending("waiting for peer response");
        let response_headers1 = HeaderSet::from_header(Header::Body(vec![1, 2, 3]));
        let response1 = ResponsePacket::new_no_data(ResponseCode::Continue, response_headers1);
        expect_request_and_reply(&mut exec, &mut remote, expect_code(OpCode::GetFinal), response1);
        exec.run_until_stalled(&mut data_fut)
            .expect_pending("waiting for additional peer responses");
        // The OK response code indicates the last user data packet.
        let response_headers2 = HeaderSet::from_header(Header::EndOfBody(vec![4, 5, 6]));
        let response2 = ResponsePacket::new_no_data(ResponseCode::Ok, response_headers2);
        expect_request_and_reply(&mut exec, &mut remote, expect_code(OpCode::GetFinal), response2);
        // All user data packets are received and the concatenated data object is returned.
        let user_data = exec
            .run_until_stalled(&mut data_fut)
            .expect("received all responses")
            .expect("valid user data");
        assert_eq!(user_data, vec![1, 2, 3, 4, 5, 6]);
    }

    #[fuchsia::test]
    fn get_operation_terminate_success() {
        let mut exec = fasync::TestExecutor::new();
        let (manager, mut remote) = new_manager(Transport::Socket, /* srm_supported */ false);
        let initial = HeaderSet::from_header(Header::name("foo"));
        let mut operation = setup_get_operation(&manager, initial);

        // Start the GET operation.
        operation.set_started();

        // Terminating early is OK. It should consume the operation and be considered complete.
        let headers = HeaderSet::from_header(Header::name("terminated"));
        let terminate_fut = operation.terminate(headers);
        let mut terminate_fut = pin!(terminate_fut);
        let _ =
            exec.run_until_stalled(&mut terminate_fut).expect_pending("waiting for peer response");
        let response = ResponsePacket::new_no_data(ResponseCode::Ok, HeaderSet::new());
        expect_request_and_reply(&mut exec, &mut remote, expect_code(OpCode::Abort), response);
    }

    #[fuchsia::test]
    fn get_operation_srm() {
        let mut exec = fasync::TestExecutor::new();
        let (manager, mut remote) = new_manager(Transport::Socket, /* srm_supported */ true);
        let mut operation = setup_get_operation(&manager, HeaderSet::new());

        // Making the initial GET request should automatically try to include SRM. Peer responds
        // positively to SRM, so it is enabled hereafter.
        {
            let info_headers = HeaderSet::from_header(Header::name("foo"));
            let info_fut = operation.get_information(info_headers);
            let mut info_fut = pin!(info_fut);
            exec.run_until_stalled(&mut info_fut).expect_pending("waiting for peer response");
            let response_headers = HeaderSet::from_headers(vec![
                Header::name("bar"),
                SingleResponseMode::Enable.into(),
            ])
            .unwrap();
            let response = ResponsePacket::new_no_data(ResponseCode::Continue, response_headers);
            let expectation = |request: RequestPacket| {
                assert_eq!(*request.code(), OpCode::Get);
                let headers = HeaderSet::from(request);
                assert!(headers.contains_header(&HeaderIdentifier::Name));
                assert!(headers.contains_header(&HeaderIdentifier::SingleResponseMode));
            };
            expect_request_and_reply(&mut exec, &mut remote, expectation, response);
            let _received_headers = exec
                .run_until_stalled(&mut info_fut)
                .expect("response received")
                .expect("valid response");
        }
        assert!(operation.is_started);
        assert_eq!(operation.srm, SingleResponseMode::Enable);

        // A subsequent request with additional headers should resolve immediately as SRM is
        // enabled.
        // Any peer response headers will only be returned after the GetFinal request.
        {
            let info_headers = HeaderSet::from_header(Header::Type("file".into()));
            let info_fut = operation.get_information(info_headers);
            let mut info_fut = pin!(info_fut);
            let received_headers = exec
                .run_until_stalled(&mut info_fut)
                .expect("ready without peer response")
                .expect("successful request");
            assert_eq!(received_headers, HeaderSet::new());
            let expectation = |request: RequestPacket| {
                assert_eq!(*request.code(), OpCode::Get);
                let headers = HeaderSet::from(request);
                assert!(headers.contains_header(&HeaderIdentifier::Type));
                assert!(!headers.contains_header(&HeaderIdentifier::SingleResponseMode));
            };
            expect_request(&mut exec, &mut remote, expectation);
        }

        // The 3-packet user payload should be returned at the end of the operation. Because
        // SRM is enabled, only one GetFinal request is issued.
        let data_fut = operation.get_data(HeaderSet::new());
        let mut data_fut = pin!(data_fut);
        exec.run_until_stalled(&mut data_fut).expect_pending("waiting for peer response");
        let response_headers1 = HeaderSet::from_header(Header::Body(vec![1, 2, 3]));
        let response1 = ResponsePacket::new_no_data(ResponseCode::Continue, response_headers1);
        expect_request_and_reply(&mut exec, &mut remote, expect_code(OpCode::GetFinal), response1);
        exec.run_until_stalled(&mut data_fut)
            .expect_pending("waiting for additional peer responses");
        // The peer will keep sending responses until the user payload has been completely sent.
        let response_headers2 = HeaderSet::from_header(Header::Body(vec![4, 5, 6]));
        let response2 = ResponsePacket::new_no_data(ResponseCode::Continue, response_headers2);
        expect_stream_pending(&mut exec, &mut remote);
        reply(&mut exec, &mut remote, response2);
        // Final user data packet.
        let response_headers3 = HeaderSet::from_header(Header::EndOfBody(vec![7, 8, 9]));
        let response3 = ResponsePacket::new_no_data(ResponseCode::Ok, response_headers3);
        expect_stream_pending(&mut exec, &mut remote);
        reply(&mut exec, &mut remote, response3);
        // All user data packets are received and the concatenated data object is returned.
        let user_data = exec
            .run_until_stalled(&mut data_fut)
            .expect("received all responses")
            .expect("valid user data");
        assert_eq!(user_data, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[fuchsia::test]
    fn client_disable_srm_mid_get_is_ignored() {
        let mut exec = fasync::TestExecutor::new();
        let (manager, mut remote) = new_manager(Transport::Socket, /* srm_supported */ true);
        let transport = manager.try_new_operation().expect("can start operation");
        let mut operation = GetOperation::new(HeaderSet::new(), transport);
        // Bypass start and enable SRM.
        operation.set_started();
        assert_eq!(operation.srm, SingleResponseMode::Enable);

        // A subsequent request with additional headers disabling SRM should not result in SRM
        // being disabled.
        {
            let info_headers = HeaderSet::from_header(SingleResponseMode::Disable.into());
            let info_fut = operation.get_information(info_headers);
            let mut info_fut = pin!(info_fut);
            let received_headers = exec
                .run_until_stalled(&mut info_fut)
                .expect("ready without peer response")
                .expect("successful request");
            assert_eq!(received_headers, HeaderSet::new());
            expect_request(&mut exec, &mut remote, expect_code(OpCode::Get));
        }
        assert_eq!(operation.srm, SingleResponseMode::Enable);
    }

    #[fuchsia::test]
    fn get_operation_information_error() {
        let mut exec = fasync::TestExecutor::new();
        let (manager, _remote) = new_manager(Transport::Socket, /* srm_supported */ false);
        let initial = HeaderSet::from_header(Header::name("foo"));
        let mut operation = setup_get_operation(&manager, initial);

        // Set started.
        operation.set_started();

        // Trying to get additional information without providing any headers is an Error.
        let get_info_fut = operation.get_information(HeaderSet::new());
        let mut get_info_fut = pin!(get_info_fut);
        let get_info_result =
            exec.run_until_stalled(&mut get_info_fut).expect("resolves with error");
        assert_matches!(get_info_result, Err(Error::OperationError { .. }));
    }

    #[fuchsia::test]
    fn get_operation_data_before_start_is_ok() {
        let mut exec = fasync::TestExecutor::new();
        let (manager, mut remote) = new_manager(Transport::Socket, /* srm_supported */ false);
        let initial = HeaderSet::from_header(Header::name("foo"));
        let operation = setup_get_operation(&manager, initial);

        // Trying to get the user data directly is OK.
        let get_data_fut = operation.get_data(HeaderSet::new());
        let mut get_data_fut = pin!(get_data_fut);
        exec.run_until_stalled(&mut get_data_fut).expect_pending("waiting for peer response");
        let response_headers = HeaderSet::from_header(Header::EndOfBody(vec![1, 2, 3]));
        let response = ResponsePacket::new_no_data(ResponseCode::Ok, response_headers);
        let expectation = |request: RequestPacket| {
            assert_eq!(*request.code(), OpCode::GetFinal);
            let headers = HeaderSet::from(request);
            assert!(headers.contains_header(&HeaderIdentifier::Name));
        };
        expect_request_and_reply(&mut exec, &mut remote, expectation, response);
        let user_data = exec
            .run_until_stalled(&mut get_data_fut)
            .expect("received all responses")
            .expect("valid user data");
        assert_eq!(user_data, vec![1, 2, 3]);
    }

    #[fuchsia::test]
    fn get_operation_data_before_start_with_srm_is_ok() {
        let mut exec = fasync::TestExecutor::new();
        let (manager, mut remote) = new_manager(Transport::Socket, /* srm_supported */ true);
        let operation = setup_get_operation(&manager, HeaderSet::new());

        // Trying to get the user data directly is OK.
        let get_data_fut = operation.get_data(HeaderSet::new());
        let mut get_data_fut = pin!(get_data_fut);
        exec.run_until_stalled(&mut get_data_fut).expect_pending("waiting for peer response");
        let response_headers1 = HeaderSet::from_headers(vec![
            Header::Body(vec![1, 1]),
            Header::SingleResponseMode(SingleResponseMode::Enable.into()),
        ])
        .unwrap();
        let response1 = ResponsePacket::new_no_data(ResponseCode::Continue, response_headers1);
        let expectation = |request: RequestPacket| {
            assert_eq!(*request.code(), OpCode::GetFinal);
            let headers = HeaderSet::from(request);
            assert!(headers.contains_header(&HeaderIdentifier::SingleResponseMode));
        };
        expect_request_and_reply(&mut exec, &mut remote, expectation, response1);
        exec.run_until_stalled(&mut get_data_fut)
            .expect_pending("waiting for additional peer responses");

        // The peer will keep sending responses until the user payload has been completely sent.
        let response_headers2 = HeaderSet::from_header(Header::Body(vec![2, 2]));
        let response2 = ResponsePacket::new_no_data(ResponseCode::Continue, response_headers2);
        expect_stream_pending(&mut exec, &mut remote);
        reply(&mut exec, &mut remote, response2);

        // Final user data packet.
        let response_headers3 = HeaderSet::from_header(Header::EndOfBody(vec![3, 3]));
        let response3 = ResponsePacket::new_no_data(ResponseCode::Ok, response_headers3);
        expect_stream_pending(&mut exec, &mut remote);
        reply(&mut exec, &mut remote, response3);
        // All user data packets are received and the concatenated data object is returned.
        let user_data = exec
            .run_until_stalled(&mut get_data_fut)
            .expect("received all responses")
            .expect("valid user data");
        assert_eq!(user_data, vec![1, 1, 2, 2, 3, 3]);
    }

    #[fuchsia::test]
    fn get_operation_data_peer_disconnect_is_error() {
        let mut exec = fasync::TestExecutor::new();
        let (manager, remote) = new_manager(Transport::Socket, /* srm_supported */ false);
        let initial = HeaderSet::from_header(Header::name("foo"));
        let mut operation = setup_get_operation(&manager, initial);
        // Bypass initial SRM setup by marking this operation as started.
        operation.set_started();

        // Before the client can get the user data, the peer disconnects.
        drop(remote);
        let get_data_fut = operation.get_data(HeaderSet::new());
        let mut get_data_fut = pin!(get_data_fut);
        let get_data_result =
            exec.run_until_stalled(&mut get_data_fut).expect("resolves with error");
        assert_matches!(get_data_result, Err(Error::IOError(_)));
    }

    #[fuchsia::test]
    async fn get_operation_terminate_before_start_error() {
        let (manager, _remote) = new_manager(Transport::Socket, /* srm_supported */ false);
        let initial = HeaderSet::from_header(Header::name("bar"));
        let operation = setup_get_operation(&manager, initial);

        // The GET operation is not in progress yet so trying to terminate will fail.
        let terminate_result = operation.terminate(HeaderSet::new()).await;
        assert_matches!(terminate_result, Err(Error::OperationError { .. }));
    }

    #[fuchsia::test]
    fn handle_get_response_success() {
        let headers = HeaderSet::from_header(Header::name("foo"));
        let response = ResponsePacket::new_no_data(ResponseCode::Continue, headers.clone());
        let result = GetOperation::handle_get_response(response).expect("valid response");
        assert_eq!(result, headers);
    }

    #[fuchsia::test]
    fn handle_get_response_error() {
        let headers = HeaderSet::from_header(Header::name("foo"));
        // Expect the Continue, not Ok.
        let response1 = ResponsePacket::new_no_data(ResponseCode::Ok, headers.clone());
        assert_matches!(
            GetOperation::handle_get_response(response1),
            Err(Error::PeerRejected { .. })
        );

        // Expect the Continue, not anything else.
        let response1 = ResponsePacket::new_no_data(ResponseCode::NotFound, headers);
        assert_matches!(
            GetOperation::handle_get_response(response1),
            Err(Error::PeerRejected { .. })
        );
    }

    #[fuchsia::test]
    fn handle_get_final_response_success() {
        let headers = HeaderSet::from_header(Header::EndOfBody(vec![1, 2]));
        let response1 = ResponsePacket::new_no_data(ResponseCode::Ok, headers);
        let result1 = GetOperation::handle_get_final_response(response1).expect("valid response");
        assert_eq!(result1, (true, HeaderSet::new(), vec![1, 2]));

        let headers = HeaderSet::from_header(Header::Body(vec![1, 3, 5]));
        let response2 = ResponsePacket::new_no_data(ResponseCode::Continue, headers);
        let result2 = GetOperation::handle_get_final_response(response2).expect("valid response");
        assert_eq!(result2, (false, HeaderSet::new(), vec![1, 3, 5]));
    }

    #[fuchsia::test]
    fn get_final_response_error() {
        // A non-success error code is an Error.
        let headers = HeaderSet::from_header(Header::EndOfBody(vec![1, 2]));
        let response1 = ResponsePacket::new_no_data(ResponseCode::Forbidden, headers);
        assert_matches!(
            GetOperation::handle_get_final_response(response1),
            Err(Error::PeerRejected { .. })
        );

        // A final packet with no EndOfBody payload is an Error.
        let response2 = ResponsePacket::new_no_data(ResponseCode::Ok, HeaderSet::new());
        assert_matches!(
            GetOperation::handle_get_final_response(response2),
            Err(Error::Packet(PacketError::Data(_)))
        );

        // A non-final packet with no Body payload is an Error.
        let response3 = ResponsePacket::new_no_data(ResponseCode::Continue, HeaderSet::new());
        assert_matches!(
            GetOperation::handle_get_final_response(response3),
            Err(Error::Packet(PacketError::Data(_)))
        );
    }
}
