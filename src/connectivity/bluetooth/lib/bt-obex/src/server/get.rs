// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use log::{trace, warn};
use packet_encoding::Encodable;
use std::collections::VecDeque;

use crate::error::Error;
use crate::header::{Header, HeaderSet, SingleResponseMode};
use crate::operation::{OpCode, RequestPacket, ResponseCode, ResponsePacket};
use crate::server::handler::ObexOperationError;
use crate::server::{ApplicationResponse, OperationRequest, ServerOperation};

/// All Body & EndOfBody headers have 3 bytes (1 byte HI, 2 bytes Length) preceding the payload.
const BODY_HEADER_PREFIX_LENGTH_BYTES: usize = 3;

/// A collection that maintains the staged data during a GET operation.
#[derive(Debug, PartialEq)]
struct StagedData {
    /// The first chunk of user data.
    /// This is Some<T> when the first chunk is available and can be taken with `next_response`
    /// and None otherwise.
    /// In some cases, no user data can fit in the first chunk and so this will be Some<None>.
    first: Option<Option<Vec<u8>>>,
    /// The remaining chunks of user data to be sent.
    /// The `StagedData` is considered exhausted and complete when both `first` and `rest` are
    /// empty.
    rest: VecDeque<Vec<u8>>,
}

impl StagedData {
    fn new(first: Option<Vec<u8>>, rest: VecDeque<Vec<u8>>) -> Self {
        Self { first: Some(first), rest }
    }

    fn empty() -> Self {
        Self { first: None, rest: VecDeque::new() }
    }

    /// Builds and stages the `data` into chunks of Headers (Body/EndOfBody) that conform to the
    /// provided `max_headers_size`.
    /// `header_size` is the size (in bytes) of the informational headers describing the `data`.
    /// Returns Ok if the data was successfully divided, Error if the data could not be chunked or
    /// if the provided `max_headers_size` or `headers_size` is invalid.
    fn from_data(
        mut data: Vec<u8>,
        max_headers_size: u16,
        headers_size: usize,
    ) -> Result<Self, Error> {
        let max_headers_size = max_headers_size as usize;
        // To many headers provided.
        if headers_size > max_headers_size {
            warn!("Too many headers in GET");
            // TODO(https://fxbug.dev/42082648): It's probably reasonable to support this case by splitting
            // the headers across multiple responses.
            return Err(Error::operation(OpCode::Get, "too many headers"));
        }

        // The maximum header size must be able to fit at least a Body/EndOfBody Header with one
        // byte of data. In practice, this max is in the range of [255, u16::MAX], but this
        // function is resilient to smaller values.
        if max_headers_size <= BODY_HEADER_PREFIX_LENGTH_BYTES {
            return Err(Error::operation(OpCode::Get, "max_headers_size too small"));
        }

        // The maximum size of the first packet is special as it may contain non-Body/EndOfBody
        // `headers` and a chunk of user data in the Body/EndOfBody.
        let max_first_data_packet_size = max_headers_size - headers_size;

        // Check if the entire payload can fit in a single packet.
        let data_encoded_len = data.len() + BODY_HEADER_PREFIX_LENGTH_BYTES;
        if data_encoded_len <= max_first_data_packet_size {
            return Ok(Self::new(Some(data), VecDeque::new()));
        }

        // Otherwise, we'll need more than one packet. Chunk the special first packet.
        let first_chunk_size =
            max_first_data_packet_size.checked_sub(BODY_HEADER_PREFIX_LENGTH_BYTES);
        let (first, remaining) = if let Some(max) = first_chunk_size {
            let remaining = data.split_off(max);
            (Some(data), remaining)
        } else {
            // No space in first packet for any user data.
            (None, data)
        };

        // The maximum size of all other packets where only data is present.
        let max_data_packet_size = max_headers_size - BODY_HEADER_PREFIX_LENGTH_BYTES;
        // Chunk up the rest.
        let mut chunks = VecDeque::new();
        for chunk in remaining.chunks(max_data_packet_size as usize) {
            chunks.push_back(chunk.to_vec());
        }
        Ok(Self::new(first, chunks))
    }

    /// Returns true if the first response exists and can be taken.
    #[cfg(test)]
    fn is_first_response(&self) -> bool {
        self.first.is_some()
    }

    /// Returns true if the first response and all staged data has been taken.
    fn is_complete(&self) -> bool {
        self.first.is_none() && self.rest.is_empty()
    }

    /// Returns the response packet for the next chunk of data in the staged payload.
    /// Returns Error if all the data has already been returned - namely, `Self::is_complete` is
    /// true or if the response packet couldn't be built with the provided `headers`.
    fn next_response(&mut self, mut headers: HeaderSet) -> Result<ResponsePacket, Error> {
        if self.is_complete() {
            return Err(Error::operation(OpCode::Get, "staged data is already complete"));
        }

        let chunk = if let Some(first_packet) = self.first.take() {
            // First chunk, which may be None if no user data can fit in the first packet.
            first_packet
        } else {
            // Otherwise, subsequent chunk. This must always be populated, even if empty.
            Some(self.rest.pop_front().unwrap_or_default())
        };

        // If `self.rest` is empty after grabbing the next chunk, then this is the final chunk of
        // the payload. An EndOfBody header is used instead of Body for the chunk.
        let (code, h) = if self.rest.is_empty() {
            (ResponseCode::Ok, chunk.map(|p| Header::EndOfBody(p)))
        } else {
            (ResponseCode::Continue, chunk.map(|p| Header::Body(p)))
        };

        if let Some(header) = h {
            headers.add(header)?;
        }
        Ok(ResponsePacket::new_get(code, headers))
    }

    /// Returns response packets for all of the staged data.
    /// Returns Error if all of the data has already been returned.
    fn all_responses(
        &mut self,
        mut initial_headers: HeaderSet,
    ) -> Result<Vec<ResponsePacket>, Error> {
        let mut responses = Vec::new();
        while !self.is_complete() {
            // Only the first packet will contain the `initial_headers`. Subsequent responses will
            // not have any informational headers.
            let headers = std::mem::replace(&mut initial_headers, HeaderSet::new());
            let response = self.next_response(headers)?;
            responses.push(response);
        }
        Ok(responses)
    }
}

/// The current state of the GET operation.
#[derive(Debug)]
enum State {
    /// The request phase of the operation in which the remote OBEX client sends informational
    /// headers describing the payload. This can be spread out over multiple packets.
    /// `headers` contain staged informational headers to be relayed to the upper application
    /// profile.
    Request { headers: HeaderSet },
    /// The request phase of the operation is complete (GET_FINAL has been received) and we are
    /// waiting for the profile application.
    /// The response phase is started by calling `GetOperation::handle_application_response` with
    /// the profile application's accepting or rejecting of the request.
    RequestPhaseComplete,
    /// The profile application has accepted the GET request.
    /// The response phase of the operation in which the local OBEX server sends the payload over
    /// potentially multiple packets.
    Response { staged_data: StagedData },
    /// The response phase of the operation is complete (all data packets have been relayed). The
    /// entire GET operation is considered complete.
    Complete,
}

/// The current SRM status for the GET operation.
enum SrmState {
    /// SRM has not been negotiated yet.
    /// `srm_supported` is true if SRM is supported locally.
    NotNegotiated { srm_supported: bool },
    /// SRM is currently negotiating and therefore we need to send a SRM response to the peer.
    /// `negotiated_srm` is the negotiated SRM value that will be sent to the peer.
    Negotiating { negotiated_srm: SingleResponseMode },
    /// SRM has been negotiated. If `srm` is `SingleResponseMode::Enable`, then SRM is considered
    /// active for the duration of this GET operation.
    Negotiated { srm: SingleResponseMode },
}

/// Represents an in-progress GET operation.
pub struct GetOperation {
    /// The maximum number of bytes that can be allocated to headers in the GetOperation. This
    /// includes informational headers and data headers.
    max_headers_size: u16,
    /// The current SRM status for this operation.
    /// SRM may not necessarily be negotiated during a GET operation in which case is defaulted
    /// to disabled.
    srm_state: SrmState,
    /// Current state of the GET operation.
    state: State,
}

impl GetOperation {
    /// `max_packet_size` is the max number of bytes that can fit in a single packet.
    pub fn new(max_packet_size: u16, srm_supported: bool) -> Self {
        let max_headers_size = max_packet_size - ResponsePacket::MIN_PACKET_SIZE as u16;
        Self {
            max_headers_size,
            srm_state: SrmState::NotNegotiated { srm_supported },
            state: State::Request { headers: HeaderSet::new() },
        }
    }

    #[cfg(test)]
    fn new_at_state(max_packet_size: u16, state: State) -> Self {
        let max_headers_size = max_packet_size - ResponsePacket::MIN_PACKET_SIZE as u16;
        Self {
            max_headers_size,
            srm_state: SrmState::NotNegotiated { srm_supported: false },
            state,
        }
    }

    /// Checks if the operation is complete and updates the state.
    fn check_complete_and_update_state(&mut self) {
        let State::Response { staged_data } = &self.state else { return };

        if staged_data.is_complete() {
            self.state = State::Complete;
        }
    }

    /// Attempts to add the SRM response to the provided `headers`.
    /// Returns Ok(true) if SRM is negotiating and the header was added.
    /// Returns Ok(false) if SRM is not negotiating and the header wasn't added.
    /// Returns Error if the SRM response couldn't be added to `headers`.
    fn maybe_add_srm_header(&mut self, headers: &mut HeaderSet) -> Result<bool, Error> {
        if let SrmState::Negotiating { negotiated_srm } = self.srm_state {
            headers.add(negotiated_srm.into())?;
            self.srm_state = SrmState::Negotiated { srm: negotiated_srm };
            return Ok(true);
        }
        Ok(false)
    }
}

impl ServerOperation for GetOperation {
    fn srm_status(&self) -> SingleResponseMode {
        // Defaults to disabled if SRM has not been negotiated.
        match self.srm_state {
            SrmState::NotNegotiated { .. } | SrmState::Negotiating { .. } => {
                SingleResponseMode::Disable
            }
            SrmState::Negotiated { srm } => srm,
        }
    }

    fn is_complete(&self) -> bool {
        matches!(self.state, State::Complete)
    }

    fn handle_peer_request(&mut self, request: RequestPacket) -> Result<OperationRequest, Error> {
        let code = *request.code();
        // The current SRM mode before processing the peer's `request`, which can contain a SRM
        // request. If SRM is not negotiated, or negotiation is in progress, this will default to
        // `Disable` since SRM is not considered active.
        let current_srm_mode = self.srm_status();
        match &mut self.state {
            State::Request { headers } if code == OpCode::Get => {
                let request_headers = HeaderSet::from(request);
                // The response to the `request` depends on the current SRM status.
                // If SRM is enabled, then no response is needed.
                // If SRM is disabled or is being negotiated, then we expect to reply with the
                // application's response headers.
                // If SRM hasn't been negotiated yet, we check to see if the peer requests it
                // and include the negotiated SRM header in the next response.
                match self.srm_state {
                    SrmState::Negotiated { srm: SingleResponseMode::Enable } => {
                        // Stage the request headers to be given to the application.
                        headers.try_append(request_headers)?;
                        return Ok(OperationRequest::None);
                    }
                    SrmState::Negotiated { srm: SingleResponseMode::Disable }
                    | SrmState::Negotiating { .. } => {}
                    SrmState::NotNegotiated { srm_supported } => {
                        // SRM hasn't been negotiated. Check if the peer is requesting it.
                        if let Some(negotiated_srm) =
                            Self::check_headers_for_srm(srm_supported, &request_headers)
                        {
                            // The peer is requesting to update the SRM status. We need to
                            // include it in the next response packet.
                            self.srm_state = SrmState::Negotiating { negotiated_srm };
                        }
                    }
                };
                Ok(OperationRequest::GetApplicationInfo(request_headers))
            }
            State::Request { headers } if code == OpCode::GetFinal => {
                headers.try_append(HeaderSet::from(request))?;
                // Update the current SRM status if it hasn't been negotiated yet.
                if let SrmState::NotNegotiated { srm_supported } = self.srm_state {
                    if let Some(negotiated_srm) =
                        Self::check_headers_for_srm(srm_supported, &headers)
                    {
                        // The peer is requesting to update the SRM status. We need to
                        // include it in the next response packet.
                        self.srm_state = SrmState::Negotiating { negotiated_srm };
                    }
                }

                let request_headers = std::mem::replace(headers, HeaderSet::new());
                // This is the final request packet. All request headers have been received and we
                // are ready to get the payload from the application.
                self.state = State::RequestPhaseComplete;
                Ok(OperationRequest::GetApplicationData(request_headers))
            }
            State::Response { staged_data } if code == OpCode::GetFinal => {
                let responses = if current_srm_mode == SingleResponseMode::Enable {
                    staged_data.all_responses(HeaderSet::new())?
                } else {
                    vec![staged_data.next_response(HeaderSet::new())?]
                };
                self.check_complete_and_update_state();
                Ok(OperationRequest::SendPackets(responses))
            }
            _ => Err(Error::operation(OpCode::Get, "received invalid request")),
        }
    }

    fn handle_application_response(
        &mut self,
        response: Result<ApplicationResponse, ObexOperationError>,
    ) -> Result<Vec<ResponsePacket>, Error> {
        let response = match response {
            Ok(response) => response,
            Err((code, response_headers)) => {
                trace!("Application rejected GET request: {code:?}");
                self.state = State::Response { staged_data: StagedData::empty() };
                self.check_complete_and_update_state();
                return Ok(vec![ResponsePacket::new_get(code, response_headers)]);
            }
        };

        match response {
            ApplicationResponse::GetInfo(mut response_headers) => {
                if !matches!(self.state, State::Request { .. }) {
                    return Err(Error::operation(OpCode::Get, "GetInfo response in invalid state"));
                }
                let _ = self.maybe_add_srm_header(&mut response_headers)?;
                Ok(vec![ResponsePacket::new_get(ResponseCode::Continue, response_headers)])
            }
            ApplicationResponse::GetData((data, response_headers)) => {
                if !matches!(self.state, State::RequestPhaseComplete) {
                    return Err(Error::operation(
                        OpCode::Get,
                        "Get response before request phase complete",
                    ));
                }

                let mut srm_headers = HeaderSet::new();
                let srm_packet = if self.maybe_add_srm_header(&mut srm_headers)? {
                    Some(ResponsePacket::new_get(ResponseCode::Continue, srm_headers))
                } else {
                    None
                };

                // Potentially split the user data payload into chunks to be sent over multiple
                // response packets.
                let mut staged_data = StagedData::from_data(
                    data,
                    self.max_headers_size,
                    response_headers.encoded_len(),
                )?;

                let responses = match (self.srm_status(), srm_packet) {
                    (SingleResponseMode::Enable, Some(packet)) => {
                        // If SRM was just enabled, then the first packet will only be the SRM
                        // response. The remaining packets will contain the data & informational
                        // headers.
                        let mut packets = vec![packet];
                        packets.append(&mut staged_data.all_responses(response_headers)?);
                        packets
                    }
                    (SingleResponseMode::Disable, Some(packet)) => {
                        // If SRM was just disabled, then the first packet will only be the SRM
                        // response. The peer will make subsequent requests to get the data.
                        vec![packet]
                    }
                    (SingleResponseMode::Enable, None) => {
                        // SRM is enabled so all packets will be returned.
                        staged_data.all_responses(response_headers)?
                    }
                    (SingleResponseMode::Disable, None) => {
                        // SRM is disabled, so only the next packet will be returned.
                        vec![staged_data.next_response(response_headers)?]
                    }
                };
                self.state = State::Response { staged_data };
                self.check_complete_and_update_state();
                Ok(responses)
            }
            ApplicationResponse::Put => {
                Err(Error::operation(OpCode::Get, "invalid application response to GET request"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;

    use crate::header::HeaderIdentifier;
    use crate::header::header_set::{expect_body, expect_end_of_body};
    use crate::server::test_utils::expect_single_packet;

    fn bytes(start_idx: usize, end_idx: usize) -> Vec<u8> {
        // NOTE: In practice this can result in unexpected behavior if `start_idx` and `end_idx`
        // are too large.
        let s = start_idx as u8;
        let e = end_idx as u8;
        (s..e).collect::<Vec<u8>>()
    }

    /// Expects a single outgoing response packet with the `expected_code` and `expected_body`.
    #[track_caller]
    fn expect_packet_with_body(
        operation_request: OperationRequest,
        expected_code: ResponseCode,
        expected_body: Vec<u8>,
    ) {
        let packet = expect_single_packet(operation_request);
        assert_eq!(*packet.code(), expected_code);
        if expected_code == ResponseCode::Ok {
            expect_end_of_body(packet.headers(), expected_body);
        } else {
            expect_body(packet.headers(), expected_body);
        }
    }

    #[fuchsia::test]
    fn single_packet_get_operation() {
        let max_packet_size = 50;
        let mut operation = GetOperation::new(max_packet_size, false);
        assert!(!operation.is_complete());

        // First (and final) request with informational headers.
        let headers = HeaderSet::from_header(Header::name("default"));
        let request = RequestPacket::new_get_final(headers);
        let response1 = operation.handle_peer_request(request).expect("valid request");
        assert_matches!(response1,
            OperationRequest::GetApplicationData(headers)
            if headers.contains_header(&HeaderIdentifier::Name)
        );

        // Application provides the payload. Since the entire payload can fit in a single packet,
        // we expect the operation to be complete after it is returned.
        let payload = bytes(0, 25);
        let mut responses2 = operation
            .handle_application_response(ApplicationResponse::accept_get(payload, HeaderSet::new()))
            .expect("valid response");
        let response2 = responses2.pop().expect("one response");
        assert_eq!(*response2.code(), ResponseCode::Ok);
        expect_end_of_body(response2.headers(), bytes(0, 25));
        assert!(operation.is_complete());
    }

    #[fuchsia::test]
    fn multi_packet_get_operation() {
        let max_packet_size = 50;
        let mut operation = GetOperation::new(max_packet_size, false);
        assert!(!operation.is_complete());

        // First request provides informational headers. Expect to ask the application for a
        // response.
        let headers1 = HeaderSet::from_header(Header::name("foo".into()));
        let request1 = RequestPacket::new_get(headers1);
        let response1 = operation.handle_peer_request(request1).expect("valid request");
        assert_matches!(response1, OperationRequest::GetApplicationInfo(headers) if headers.contains_header(&HeaderIdentifier::Name));

        // Application responds with a header - expect to handle it and return a single outgoing
        // packet to be sent to the peer.
        let info_headers = HeaderSet::from_header(Header::Description("ok".into()));
        let response2 = operation
            .handle_application_response(ApplicationResponse::accept_get_info(info_headers))
            .expect("valid request");
        assert_eq!(response2.len(), 1);
        assert_eq!(*response2[0].code(), ResponseCode::Continue);
        assert!(response2[0].headers().contains_header(&HeaderIdentifier::Description));

        // Peer sends a final request with more informational headers. Expect to ask the profile
        // application for the user data payload. The received informational headers should also be
        // relayed.
        let headers3 = HeaderSet::from_header(Header::Type("text/x-vCard".into()));
        let request3 = RequestPacket::new_get_final(headers3);
        let response3 = operation.handle_peer_request(request3).expect("valid request");
        assert_matches!(response3,
            OperationRequest::GetApplicationData(headers)
            if headers.contains_header(&HeaderIdentifier::Type)
        );

        // After providing the payload, we expect the first response packet to contain the
        // response headers and the first chunk of the payload since it cannot entirely fit in
        // `max_packet_size`.
        // The `response_headers` has an encoded size of 33 bytes. This leaves 17 bytes for the
        // first chunk of user data, of which 6 bytes are allocated to the prefix.
        let payload = bytes(0, 200);
        let response_headers = HeaderSet::from_header(Header::Description("random payload".into()));
        let response_packets4 = operation
            .handle_application_response(ApplicationResponse::accept_get(payload, response_headers))
            .expect("valid response");
        assert_eq!(response_packets4.len(), 1);
        assert_eq!(*response_packets4[0].code(), ResponseCode::Continue);
        expect_body(response_packets4[0].headers(), bytes(0, 11));

        // Peer will keep asking for payload until finished.
        // Each data chunk will be 44 bytes long (max 50 - 6 byte prefix)
        let expected_bytes =
            vec![bytes(11, 55), bytes(55, 99), bytes(99, 143), bytes(143, 187), bytes(187, 200)];
        for (i, expected) in expected_bytes.into_iter().enumerate() {
            let expected_code = if i == 4 { ResponseCode::Ok } else { ResponseCode::Continue };
            let request = RequestPacket::new_get_final(HeaderSet::new());
            let response = operation.handle_peer_request(request).expect("valid request");
            expect_packet_with_body(response, expected_code, expected);
        }
        assert!(operation.is_complete());
    }

    #[fuchsia::test]
    fn multi_packet_get_operation_srm_enabled() {
        let max_packet_size = 50;
        let mut operation = GetOperation::new(max_packet_size, true);
        assert!(!operation.is_complete());
        assert_eq!(operation.srm_status(), SingleResponseMode::Disable);

        // First request provides a Name and SRM enable request. Expect to ask application and
        // eventually reply positively to negotiate SRM.
        let headers1 = HeaderSet::from_headers(vec![
            Header::name("foo".into()),
            SingleResponseMode::Enable.into(),
        ])
        .unwrap();
        let request1 = RequestPacket::new_get(headers1);
        let response1 = operation.handle_peer_request(request1).expect("valid request");
        assert_matches!(response1, OperationRequest::GetApplicationInfo(headers) if headers.contains_header(&HeaderIdentifier::Name));

        // Application responds with a header - expect to return a single outgoing packet to be sent
        // to the peer which contains the SRM header.
        let info_headers = HeaderSet::from_header(Header::Description("ok".into()));
        let response2 = operation
            .handle_application_response(ApplicationResponse::accept_get_info(info_headers))
            .expect("valid request");
        assert_eq!(response2.len(), 1);
        assert_eq!(*response2[0].code(), ResponseCode::Continue);
        assert!(response2[0].headers().contains_header(&HeaderIdentifier::Description));
        assert!(response2[0].headers().contains_header(&HeaderIdentifier::SingleResponseMode));
        // SRM should be enabled now.
        assert_eq!(operation.srm_status(), SingleResponseMode::Enable);

        // Second (non-final) request from the peer. Don't expect an immediate response since SRM is
        // active.
        let headers3 = HeaderSet::from_header(Header::Description("random payload".into()));
        let request3 = RequestPacket::new_get(headers3);
        let response3 = operation.handle_peer_request(request3).expect("valid request");
        assert_matches!(response3, OperationRequest::None);

        // Third and final request provides a Type header - the request phase is considered complete
        // Expect to ask the profile application for the user data payload with the previous two
        // headers.
        let headers4 = HeaderSet::from_header(Header::Type("text/x-vCard".into()));
        let request4 = RequestPacket::new_get_final(headers4);
        let response4 = operation.handle_peer_request(request4).expect("valid request");
        assert_matches!(response4,
            OperationRequest::GetApplicationData(headers)
            if
             headers.contains_header(&HeaderIdentifier::Type)
            && headers.contains_header(&HeaderIdentifier::Description)
        );

        // After getting the payload from the application, we expect to send _all_ of the packets
        // subsequently, since SRM is enabled. We expect 4 packets in total due to `max_packet_size`
        // limitations.
        let payload = bytes(0, 100);
        let response_headers = HeaderSet::from_header(Header::Description("random payload".into()));
        let response_packets = operation
            .handle_application_response(ApplicationResponse::accept_get(payload, response_headers))
            .expect("valid response");
        assert_eq!(response_packets.len(), 4);
        // First packet is special as it contains informational headers & data. `response_headers`
        // takes up 39 bytes when encoded, so only 11 bytes of user data can fit in the packet.
        assert_eq!(*response_packets[0].code(), ResponseCode::Continue);
        expect_body(response_packets[0].headers(), bytes(0, 11));

        // Each subsequent data chunk will be 44 bytes long (max 50 - 3 bytes for response prefix
        // - 3 bytes for header prefix).
        let expected_bytes = [bytes(11, 55), bytes(55, 99), bytes(99, 100)];
        for (i, expected) in expected_bytes.into_iter().enumerate() {
            // Skip the first packet since it's validated outside of the loop.
            let idx = i + 1;
            // Last packet has a code of `Ok`.
            let expected_code = if idx == 3 { ResponseCode::Ok } else { ResponseCode::Continue };
            assert_eq!(*response_packets[idx].code(), expected_code);
            if expected_code == ResponseCode::Ok {
                expect_end_of_body(response_packets[idx].headers(), expected);
            } else {
                expect_body(response_packets[idx].headers(), expected);
            }
        }

        // The operation is considered complete after this.
        assert!(operation.is_complete());
    }

    // While unusual, it's valid for the peer to request SRM on the GetFinal packet. We should
    // handle this and send the remaining data chunks after the first response.
    #[fuchsia::test]
    fn srm_enable_request_during_get_final_success() {
        let max_packet_size = 50;
        let mut operation = GetOperation::new(max_packet_size, true);

        // First (and final) request contains a SRM enable request. Expect to get the data from
        // the application and respond.
        let headers1 = HeaderSet::from_header(SingleResponseMode::Enable.into());
        let request1 = RequestPacket::new_get_final(headers1);
        let response1 = operation.handle_peer_request(request1).expect("valid request");
        assert_matches!(response1, OperationRequest::GetApplicationData(_));
        assert!(!operation.is_complete());

        // Because SRM was just requested, the first response packet should contain the SRM accept
        // response. Subsequent packets will contain the data.
        let payload = bytes(0, 90);
        let response_headers = HeaderSet::from_header(Header::Description("random payload".into()));
        let response_packets = operation
            .handle_application_response(ApplicationResponse::accept_get(payload, response_headers))
            .expect("valid response");
        assert_eq!(response_packets.len(), 4);
        assert_eq!(*response_packets[0].code(), ResponseCode::Continue);
        assert!(
            response_packets[0].headers().contains_header(&HeaderIdentifier::SingleResponseMode)
        );
        // Shouldn't contain the Body or Description, yet.
        assert!(!response_packets[0].headers().contains_header(&HeaderIdentifier::Description));
        assert!(!response_packets[0].headers().contains_header(&HeaderIdentifier::Body));
        assert_eq!(operation.srm_status(), SingleResponseMode::Enable);

        // Each chunk can hold max (50) - 6 bytes (prefix) = 44 bytes of user data.
        // The first chunk of data also contains the Description header (39 bytes), so there is only
        // 12 bytes of data.
        assert!(response_packets[1].headers().contains_header(&HeaderIdentifier::Description));
        expect_body(response_packets[1].headers(), bytes(0, 11));
        expect_body(response_packets[2].headers(), bytes(11, 55));
        expect_end_of_body(response_packets[3].headers(), bytes(55, 90));
        assert!(operation.is_complete());
    }

    #[fuchsia::test]
    fn srm_disable_request_during_get_final_success() {
        let max_packet_size = 50;
        let mut operation = GetOperation::new(max_packet_size, false);

        // First (and final) request contains a SRM enable request. Expect to get the data from
        // the application and respond.
        let headers1 = HeaderSet::from_header(SingleResponseMode::Enable.into());
        let request1 = RequestPacket::new_get_final(headers1);
        let response1 = operation.handle_peer_request(request1).expect("valid request");
        assert_matches!(response1, OperationRequest::GetApplicationData(_));
        assert!(!operation.is_complete());

        // Because SRM was just requested and we don't support it, the first packet should only
        // contain the negative response - SRM should be disabled for this operation.
        let payload = bytes(0, 90);
        let response_packets = operation
            .handle_application_response(ApplicationResponse::accept_get(payload, HeaderSet::new()))
            .expect("valid response");
        assert_eq!(response_packets.len(), 1);
        assert_eq!(*response_packets[0].code(), ResponseCode::Continue);
        let received_srm = response_packets[0]
            .headers()
            .get(&HeaderIdentifier::SingleResponseMode)
            .expect("contains SRM");
        assert_eq!(*received_srm, Header::SingleResponseMode(SingleResponseMode::Disable));
        // Shouldn't contain the Body in the first packet.
        assert!(!response_packets[0].headers().contains_header(&HeaderIdentifier::Body));
        assert_eq!(operation.srm_status(), SingleResponseMode::Disable);

        // Because SRM is disabled, the peeer should issue GETFINAL requests for each data chunk.
        let expected_bytes = vec![bytes(0, 44), bytes(44, 88), bytes(88, 90)];
        for (i, expected) in expected_bytes.into_iter().enumerate() {
            let expected_code = if i == 2 { ResponseCode::Ok } else { ResponseCode::Continue };
            let request2 = RequestPacket::new_get_final(HeaderSet::new());
            let response2 = operation.handle_peer_request(request2).expect("valid request");
            expect_packet_with_body(response2, expected_code, expected);
        }
        assert!(operation.is_complete());
    }

    #[fuchsia::test]
    fn application_rejects_request_success() {
        let mut operation = GetOperation::new_at_state(10, State::RequestPhaseComplete);
        let headers = HeaderSet::from_header(Header::Description("not allowed today".into()));
        let response_packets = operation
            .handle_application_response(Err((ResponseCode::Forbidden, headers)))
            .expect("rejection is ok");
        assert_eq!(*response_packets[0].code(), ResponseCode::Forbidden);
        assert!(response_packets[0].headers().contains_header(&HeaderIdentifier::Description));
        assert!(operation.is_complete());
    }

    #[fuchsia::test]
    fn handle_application_response_error() {
        let max_packet_size = 15;
        // Receiving the application data response before the request phase is complete is an Error.
        let mut operation = GetOperation::new(max_packet_size, false);
        let data = vec![1, 2, 3];
        assert_matches!(
            operation.handle_application_response(ApplicationResponse::accept_get(
                data,
                HeaderSet::new()
            )),
            Err(Error::OperationError { .. })
        );

        // Receiving the application info response after the request phase is complete is an Error.
        let mut operation = GetOperation::new_at_state(10, State::RequestPhaseComplete);
        assert_matches!(
            operation.handle_application_response(ApplicationResponse::accept_get_info(
                HeaderSet::new()
            )),
            Err(Error::OperationError { .. })
        );
    }

    #[fuchsia::test]
    fn non_get_request_is_error() {
        let mut operation = GetOperation::new(50, false);
        let random_request1 = RequestPacket::new_put(HeaderSet::new());
        assert_matches!(
            operation.handle_peer_request(random_request1),
            Err(Error::OperationError { .. })
        );

        let random_request2 = RequestPacket::new_disconnect(HeaderSet::new());
        assert_matches!(
            operation.handle_peer_request(random_request2),
            Err(Error::OperationError { .. })
        );
    }

    #[fuchsia::test]
    fn get_request_invalid_state_is_error() {
        let random_headers = HeaderSet::from_header(Header::name("foo".into()));

        // Receiving another GET request while we are waiting for the application to accept is an
        // Error.
        let mut operation1 = GetOperation::new_at_state(10, State::RequestPhaseComplete);
        let request1 = RequestPacket::new_get(random_headers.clone());
        let response1 = operation1.handle_peer_request(request1);
        assert_matches!(response1, Err(Error::OperationError { .. }));

        // Receiving a GET request when the operation is complete is an Error.
        let mut operation2 = GetOperation::new_at_state(10, State::Complete);
        let request2 = RequestPacket::new_get(random_headers.clone());
        let response2 = operation2.handle_peer_request(request2);
        assert_matches!(response2, Err(Error::OperationError { .. }));

        // Receiving a non-GETFINAL request in the response phase is an Error.
        let staged_data = StagedData { first: None, rest: VecDeque::from(vec![vec![1, 2, 3]]) };
        let mut operation3 = GetOperation::new_at_state(10, State::Response { staged_data });
        let request3 = RequestPacket::new_get(random_headers);
        let response3 = operation3.handle_peer_request(request3);
        assert_matches!(response3, Err(Error::OperationError { .. }));
    }

    #[fuchsia::test]
    fn build_staged_data_success() {
        // An empty data payload is fine. Should be chunked as a single packet.
        let empty_data = Vec::new();
        let empty_headers = HeaderSet::new();
        let result = StagedData::from_data(empty_data, 50, empty_headers.encoded_len())
            .expect("can divide data");
        let expected = StagedData { first: Some(Some(vec![])), rest: VecDeque::new() };
        assert_eq!(result, expected);

        // A data payload that can fit in a single packet.
        let headers = HeaderSet::from_header(Header::name("foo".into()));
        let data = vec![1, 2, 3];
        let result =
            StagedData::from_data(data, 50, headers.encoded_len()).expect("can divide data");
        let expected = StagedData { first: Some(Some(vec![1, 2, 3])), rest: VecDeque::new() };
        assert_eq!(result, expected);

        // A data payload with headers that is split into multiple packets. The first chunk is
        // smaller since there must be room for the provided headers.
        let headers = HeaderSet::from_header(Header::Http(vec![5, 5, 5]));
        let max = 10;
        let large_data = (0..50).collect::<Vec<u8>>();
        let result =
            StagedData::from_data(large_data, max, headers.encoded_len()).expect("can divide data");
        let first = Some(vec![0]);
        let rest = VecDeque::from(vec![
            bytes(1, 8),
            bytes(8, 15),
            bytes(15, 22),
            bytes(22, 29),
            bytes(29, 36),
            bytes(36, 43),
            bytes(43, 50),
        ]);
        let expected = StagedData { first: Some(first), rest };
        assert_eq!(result, expected);
    }

    #[fuchsia::test]
    fn build_staged_data_error() {
        let random_data = bytes(0, 50);

        // Cannot build and stage data if the overall max packet size is too small.
        let too_small_max = 2;
        assert_matches!(
            StagedData::from_data(random_data.clone(), too_small_max, 0),
            Err(Error::OperationError { .. })
        );
        assert_matches!(
            StagedData::from_data(random_data.clone(), 0, 0),
            Err(Error::OperationError { .. })
        );

        // Cannot build and stage data if the header size is larger than the max.
        // TODO(https://fxbug.dev/42082648): Delete this case when headers can be split across packets.
        let small_max = 10;
        let large_header_size = 20;
        assert_matches!(
            StagedData::from_data(random_data, small_max, large_header_size),
            Err(Error::OperationError { .. })
        );
    }

    #[fuchsia::test]
    fn empty_staged_data_success() {
        let empty = Vec::new();
        let empty_headers = HeaderSet::new();
        let mut staged = StagedData::from_data(empty.clone(), 50, empty_headers.encoded_len())
            .expect("can construct");
        assert!(staged.is_first_response());
        assert!(!staged.is_complete());
        let response = staged.next_response(empty_headers).expect("has first response");
        assert_eq!(*response.code(), ResponseCode::Ok);
        expect_end_of_body(response.headers(), vec![]);
        assert!(!staged.is_first_response());
        assert!(staged.is_complete());
    }

    #[fuchsia::test]
    fn single_packet_staged_data_success() {
        let single = vec![1, 2, 3];
        let empty_headers = HeaderSet::new();
        let mut staged = StagedData::from_data(single.clone(), 50, empty_headers.encoded_len())
            .expect("can construct");
        let response = staged.next_response(empty_headers).expect("has first response");
        assert_eq!(*response.code(), ResponseCode::Ok);
        expect_end_of_body(response.headers(), single);
        assert!(staged.is_complete());
    }

    #[fuchsia::test]
    fn multi_packet_staged_data_success() {
        let max_packet_size = 10;
        let large_data = (0..30).collect::<Vec<u8>>();
        let headers = HeaderSet::from_header(Header::Who(vec![1, 2, 3, 4, 5]));
        let mut staged = StagedData::from_data(large_data, max_packet_size, headers.encoded_len())
            .expect("can construct");
        let response1 = staged.next_response(headers).expect("has first response");
        assert_eq!(*response1.code(), ResponseCode::Continue);
        // First buffer has no user data since it can't fit with headers.
        assert!(response1.headers().contains_header(&HeaderIdentifier::Who));
        assert!(!response1.headers().contains_header(&HeaderIdentifier::Body));
        assert!(!staged.is_complete());

        // Each next chunk should contain 7 bytes each since the max is 10 and 3 bytes are
        // reserved for the header prefix.
        let expected_bytes = vec![bytes(0, 7), bytes(7, 14), bytes(14, 21), bytes(21, 28)];
        for expected in expected_bytes {
            let r = staged.next_response(HeaderSet::new()).expect("has next response");
            assert_eq!(*r.code(), ResponseCode::Continue);
            expect_body(r.headers(), expected);
        }

        // Final chunk has the remaining bits and an Ok response code to signal completion.
        let final_response = staged.next_response(HeaderSet::new()).expect("has next response");
        assert_eq!(*final_response.code(), ResponseCode::Ok);
        let expected = bytes(28, 30);
        expect_end_of_body(final_response.headers(), expected);
        assert!(staged.is_complete());
    }

    #[fuchsia::test]
    fn staged_data_response_error() {
        // Calling `next_response` when complete is Error.
        let mut staged = StagedData::new(None, VecDeque::new());
        let _ = staged.next_response(HeaderSet::new()).expect("has first response");
        assert!(staged.is_complete());
        assert_matches!(staged.next_response(HeaderSet::new()), Err(Error::OperationError { .. }));
    }
}
