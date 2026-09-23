// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use log::trace;

use crate::error::Error;
use crate::header::{Header, HeaderIdentifier, HeaderSet, SingleResponseMode};
use crate::operation::{MAX_OBJECT_SIZE, OpCode, RequestPacket, ResponseCode, ResponsePacket};
use crate::server::handler::ObexOperationError;
use crate::server::{ApplicationResponse, OperationRequest, ServerOperation};

/// The current state of the PUT operation.
#[derive(Debug)]
enum State {
    /// Receiving informational headers and data packets.
    Request { headers: HeaderSet, staged_data: Option<Vec<u8>> },
    /// The final request packet has been received.
    RequestPhaseComplete,
    /// The operation is complete.
    Complete,
}

/// An in-progress PUT operation.
pub struct PutOperation {
    /// Whether SRM is locally supported.
    srm_supported: bool,
    /// The current SRM status for this operation. This is None if it has not been negotiated
    /// and Some<T> when negotiated.
    /// Defaults to disabled if never negotiated.
    srm: Option<SingleResponseMode>,
    /// The maximum total payload size allowed for an object in this operation.
    max_object_size: usize,
    state: State,
}

impl PutOperation {
    pub fn new(srm_supported: bool) -> Self {
        Self::new_with_max_size(srm_supported, MAX_OBJECT_SIZE)
    }

    fn new_with_max_size(srm_supported: bool, max_object_size: usize) -> Self {
        Self {
            srm_supported,
            srm: None,
            max_object_size,
            state: State::Request { headers: HeaderSet::new(), staged_data: None },
        }
    }

    #[cfg(test)]
    fn new_at_state(state: State) -> Self {
        Self { srm_supported: false, srm: None, max_object_size: MAX_OBJECT_SIZE, state }
    }

    /// Validates that incoming payload data does not exceed maximum size limits and stages it.
    fn validate_and_stage_data(
        max_object_size: usize,
        request_headers: &mut HeaderSet,
        current_headers: &mut HeaderSet,
        staged_data: &mut Option<Vec<u8>>,
        is_final: bool,
    ) -> Result<Result<(), OperationRequest>, Error> {
        let body_data = request_headers.remove_body(is_final).ok();
        let new_data_len = body_data.as_ref().map_or(0, |d| d.len());

        let has_oversized_length_header = match request_headers
            .get(&HeaderIdentifier::Length)
            .or_else(|| current_headers.get(&HeaderIdentifier::Length))
        {
            Some(Header::Length(len)) => *len as usize > max_object_size,
            _ => false,
        };

        let current_len = staged_data.as_ref().map_or(0, |v| v.len());
        if has_oversized_length_header || current_len.saturating_add(new_data_len) > max_object_size
        {
            return Ok(Err(OperationRequest::single_response(ResponsePacket::new_empty(
                ResponseCode::RequestedEntityTooLarge,
            ))));
        }

        if let Some(mut data) = body_data {
            let staged = staged_data.get_or_insert(Vec::new());
            staged.append(&mut data);
        }

        current_headers.try_append(std::mem::take(request_headers))?;
        Ok(Ok(()))
    }
}

impl ServerOperation for PutOperation {
    fn srm_status(&self) -> SingleResponseMode {
        self.srm.unwrap_or(SingleResponseMode::Disable)
    }

    fn is_complete(&self) -> bool {
        matches!(self.state, State::Complete)
    }

    fn handle_peer_request(&mut self, request: RequestPacket) -> Result<OperationRequest, Error> {
        let code = *request.code();
        let mut request_headers = HeaderSet::from(request);
        match &mut self.state {
            State::Request { headers, staged_data } if code == OpCode::Put => {
                // A non-final PUT request may contain a Body header specifying user data (among
                // other informational headers).
                if let Err(op_req) = Self::validate_and_stage_data(
                    self.max_object_size,
                    &mut request_headers,
                    headers,
                    staged_data,
                    /*is_final=*/ false,
                )? {
                    self.state = State::Complete;
                    return Ok(op_req);
                }

                // The response to the `request` depends on the current SRM status.
                // If SRM is enabled, then no response is needed.
                // If SRM is disabled, then we must acknowledge the `request`.
                // If SRM hasn't been negotiated yet, we check to see if the peer requests it
                // and reply with the negotiated SRM header.
                let response_headers = match self.srm {
                    Some(SingleResponseMode::Enable) => return Ok(OperationRequest::None),
                    Some(SingleResponseMode::Disable) => HeaderSet::new(),
                    None => {
                        self.srm = Self::check_headers_for_srm(self.srm_supported, &headers);
                        // If SRM was just negotiated then we need to include it in the response.
                        self.srm.as_ref().map_or_else(HeaderSet::new, |srm| {
                            HeaderSet::from_header(Header::SingleResponseMode(*srm))
                        })
                    }
                };
                let response =
                    ResponsePacket::new_no_data(ResponseCode::Continue, response_headers);
                Ok(OperationRequest::single_response(response))
            }
            State::Request { headers, staged_data } if code == OpCode::PutFinal => {
                // A final PUT request may contain an EndOfBody header specifying user data (among
                // other informational headers).
                if let Err(op_req) = Self::validate_and_stage_data(
                    self.max_object_size,
                    &mut request_headers,
                    headers,
                    staged_data,
                    /*is_final=*/ true,
                )? {
                    self.state = State::Complete;
                    return Ok(op_req);
                }

                let request_headers = std::mem::replace(headers, HeaderSet::new());
                let request_data = std::mem::take(staged_data);
                self.state = State::RequestPhaseComplete;
                // If data is provided, then we are placing data in the application. Otherwise, we
                // are deleting data identified by the provided `request_headers`.
                // See OBEX 1.5 Section 3.4.3.6.
                if let Some(data) = request_data {
                    Ok(OperationRequest::PutApplicationData(data, request_headers))
                } else {
                    Ok(OperationRequest::DeleteApplicationData(request_headers))
                }
            }
            _ => Err(Error::operation(OpCode::Put, "received invalid request")),
        }
    }

    fn handle_application_response(
        &mut self,
        response: Result<ApplicationResponse, ObexOperationError>,
    ) -> Result<Vec<ResponsePacket>, Error> {
        // Only expect a response when all request packets have been received.
        if !matches!(self.state, State::RequestPhaseComplete) {
            return Err(Error::operation(OpCode::Put, "invalid state"));
        }

        let response = match response {
            Ok(ApplicationResponse::Put) => ResponsePacket::new_empty(ResponseCode::Ok),
            Err((code, response_headers)) => {
                trace!("Application rejected PUT request: {code:?}");
                ResponsePacket::new_no_data(code, response_headers)
            }
            _ => {
                return Err(Error::operation(
                    OpCode::Put,
                    "invalid application response to PUT request",
                ));
            }
        };
        self.state = State::Complete;
        Ok(vec![response])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;

    use crate::header::HeaderIdentifier;
    use crate::server::test_utils::expect_single_packet;

    #[fuchsia::test]
    fn single_stage_put_success() {
        let mut operation = PutOperation::new(/*srm_supported=*/ false);
        assert!(!operation.is_complete());

        let body = (1..10).collect::<Vec<u8>>();
        let eob = Header::EndOfBody(body.clone());
        let name = Header::name("foo".into());
        let type_ = Header::Type("text".into());
        let headers = HeaderSet::from_headers(vec![eob, name, type_]).unwrap();
        let request = RequestPacket::new_put_final(headers);
        let response = operation.handle_peer_request(request).expect("valid request");
        assert_matches!(response,
            OperationRequest::PutApplicationData(data, headers)
            if headers.contains_header(&HeaderIdentifier::Name)
            && headers.contains_header(&HeaderIdentifier::Type)
            && data == body
        );
        assert!(!operation.is_complete());

        // Application accepts.
        let responses = operation
            .handle_application_response(ApplicationResponse::accept_put())
            .expect("valid response");
        assert_eq!(*responses[0].code(), ResponseCode::Ok);
        assert!(operation.is_complete());
    }

    #[fuchsia::test]
    fn multi_packet_put() {
        let mut operation = PutOperation::new(/*srm_supported=*/ false);
        assert!(!operation.is_complete());

        // First request to provide informational headers describing the PUT payload. We will ack
        // with an empty Continue packet.
        let headers1 = HeaderSet::from_header(Header::name("random file".into()));
        let request1 = RequestPacket::new_put(headers1);
        let response1 = operation.handle_peer_request(request1).expect("valid request");
        let response_packet1 = expect_single_packet(response1);
        assert_eq!(*response_packet1.code(), ResponseCode::Continue);
        assert!(!operation.is_complete());

        // Second request just contains a part of the payload. We will ack with an empty Continue
        // packet.
        let body2 = (0..50).collect::<Vec<u8>>();
        let headers2 = HeaderSet::from_header(Header::Body(body2));
        let request2 = RequestPacket::new_put(headers2);
        let response2 = operation.handle_peer_request(request2).expect("valid request");
        let response_packet2 = expect_single_packet(response2);
        assert_eq!(*response_packet2.code(), ResponseCode::Continue);
        assert!(!operation.is_complete());

        // Third and final request contains the remaining payload. We will ask the application to
        // accept or reject with the complete reassembled data payload.
        let body3 = (50..100).collect::<Vec<u8>>();
        let headers3 = HeaderSet::from_header(Header::EndOfBody(body3));
        let request3 = RequestPacket::new_put_final(headers3);
        let response3 = operation.handle_peer_request(request3).expect("valid request");
        let expected_payload = (0..100).collect::<Vec<u8>>();
        assert_matches!(response3,
            OperationRequest::PutApplicationData(data, headers)
            if headers.contains_header(&HeaderIdentifier::Name)
            && data == expected_payload
        );
        assert!(!operation.is_complete());

        // Simulate application accepting - expect a single outgoing packet acknowledging the PUT.
        let responses4 = operation
            .handle_application_response(ApplicationResponse::accept_put())
            .expect("valid response");
        assert_eq!(responses4.len(), 1);
        assert_eq!(*responses4[0].code(), ResponseCode::Ok);
        assert!(operation.is_complete());
    }

    #[fuchsia::test]
    fn multi_packet_put_with_duplicate_headers_is_ok() {
        let mut operation = PutOperation::new(/*srm_supported=*/ false);

        // Peer's first request provides informational headers about the payload.
        let type_header = Header::Type("foo".into());
        let headers1 =
            HeaderSet::from_headers(vec![Header::name("random file".into()), type_header.clone()])
                .unwrap();
        let request1 = RequestPacket::new_put(headers1);
        let response1 = operation.handle_peer_request(request1).expect("valid request");
        let response_packet1 = expect_single_packet(response1);
        assert_eq!(*response_packet1.code(), ResponseCode::Continue);
        assert!(!operation.is_complete());

        // Peer's second and final request contains a duplicate informational header & payload.
        let body = (0..10).collect::<Vec<u8>>();
        let headers2 =
            HeaderSet::from_headers(vec![type_header, Header::EndOfBody(body.clone())]).unwrap();
        let request2 = RequestPacket::new_put_final(headers2);
        let response2 = operation.handle_peer_request(request2).expect("valid request");
        assert_matches!(response2,
            OperationRequest::PutApplicationData(data, headers)
            if headers.contains_header(&HeaderIdentifier::Name) && headers.contains_header(&HeaderIdentifier::Type)
            && data == body
        );
        assert!(!operation.is_complete());

        // Simulate application accepting - expect a single outgoing packet acknowledging the PUT.
        let responses3 = operation
            .handle_application_response(ApplicationResponse::accept_put())
            .expect("valid response");
        assert_eq!(responses3.len(), 1);
        assert_eq!(*responses3[0].code(), ResponseCode::Ok);
        assert!(operation.is_complete());
    }

    #[fuchsia::test]
    fn multi_packet_put_srm_enabled() {
        let mut operation = PutOperation::new(/*srm_supported=*/ true);
        assert!(!operation.is_complete());
        assert_eq!(operation.srm_status(), SingleResponseMode::Disable);

        // First request provides informational headers & SRM enable request. Expect to reply
        // positively and enable SRM.
        let headers1 = HeaderSet::from_headers(vec![
            Header::name("random file".into()),
            SingleResponseMode::Enable.into(),
        ])
        .unwrap();
        let request1 = RequestPacket::new_put(headers1);
        let response1 = operation.handle_peer_request(request1).expect("valid request");
        let response_packet1 = expect_single_packet(response1);
        assert_eq!(*response_packet1.code(), ResponseCode::Continue);
        let received_srm = response_packet1
            .headers()
            .get(&HeaderIdentifier::SingleResponseMode)
            .expect("contains SRM header");
        assert_eq!(*received_srm, Header::SingleResponseMode(SingleResponseMode::Enable));
        assert_eq!(operation.srm_status(), SingleResponseMode::Enable);
        assert!(!operation.is_complete());

        // Second request just contains a part of the payload - since SRM is enabled, no ack.
        let body2 = (0..50).collect::<Vec<u8>>();
        let request2 = RequestPacket::new_put(HeaderSet::from_header(Header::Body(body2)));
        let response2 = operation.handle_peer_request(request2).expect("valid request");
        assert_matches!(response2, OperationRequest::None);
        assert!(!operation.is_complete());

        // Third and final request contains the remaining payload. We will ask the application to
        // accept or reject with the complete reassembled data payload.
        let body3 = (50..100).collect::<Vec<u8>>();
        let request3 =
            RequestPacket::new_put_final(HeaderSet::from_header(Header::EndOfBody(body3)));
        let response3 = operation.handle_peer_request(request3).expect("valid request");
        let expected_payload = (0..100).collect::<Vec<u8>>();
        assert_matches!(response3,
            OperationRequest::PutApplicationData(data, headers)
            if headers.contains_header(&HeaderIdentifier::Name)
            && data == expected_payload
        );
        assert!(!operation.is_complete());

        // Simulate application accepting - expect a single outbound packet acknowledging the PUT.
        let responses4 = operation
            .handle_application_response(ApplicationResponse::accept_put())
            .expect("valid response");
        assert_eq!(responses4.len(), 1);
        assert_eq!(*responses4[0].code(), ResponseCode::Ok);
        assert!(operation.is_complete());
    }

    #[fuchsia::test]
    fn application_reject_is_ok() {
        let mut operation = PutOperation::new_at_state(State::RequestPhaseComplete);

        let headers = HeaderSet::from_header(Header::Description("not allowed".into()));
        let reject = Err((ResponseCode::Forbidden, headers));
        let response_packets =
            operation.handle_application_response(reject).expect("valid response");
        assert_eq!(*response_packets[0].code(), ResponseCode::Forbidden);
        assert!(response_packets[0].headers().contains_header(&HeaderIdentifier::Description));
        assert!(operation.is_complete());
    }

    #[fuchsia::test]
    fn non_put_request_is_error() {
        let mut operation = PutOperation::new(/*srm_supported=*/ false);
        let random_request1 = RequestPacket::new_get(HeaderSet::new());
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
    fn invalid_application_response_is_error() {
        // Receiving a response before the request phase is complete is an Error.
        let mut operation = PutOperation::new(/*srm_supported=*/ false);
        assert_matches!(
            operation.handle_application_response(ApplicationResponse::accept_put()),
            Err(Error::OperationError { .. })
        );

        // We don't expect a GET response from the application.
        let mut operation = PutOperation::new_at_state(State::RequestPhaseComplete);
        let invalid = ApplicationResponse::accept_get(vec![], HeaderSet::new());
        assert_matches!(
            operation.handle_application_response(invalid),
            Err(Error::OperationError { .. })
        );
    }

    #[fuchsia::test]
    fn delete_request_success() {
        let mut operation = PutOperation::new(/*srm_supported=*/ false);

        let headers = HeaderSet::from_header(Header::name("randomfile.txt"));
        let request = RequestPacket::new_put_final(headers);
        let response = operation.handle_peer_request(request).expect("valid request");
        assert_matches!(response, OperationRequest::DeleteApplicationData(headers) if headers.contains_header(&HeaderIdentifier::Name));
        assert!(!operation.is_complete());

        // Application accepts delete request.
        let final_responses = operation
            .handle_application_response(ApplicationResponse::accept_put())
            .expect("valid application response");
        let final_response = final_responses.first().expect("one response");
        assert_eq!(*final_response.code(), ResponseCode::Ok);
        assert!(final_response.headers().is_empty());
        assert!(operation.is_complete());
    }

    #[fuchsia::test]
    fn put_request_with_oversized_length_header_rejected() {
        let mut operation = PutOperation::new_with_max_size(
            /*srm_supported=*/ false, /*max_object_size=*/ 100,
        );

        let headers =
            HeaderSet::from_headers(vec![Header::name("foo.txt"), Header::Length(101)]).unwrap();
        let request = RequestPacket::new_put(headers);
        let response = operation.handle_peer_request(request).expect("valid request");
        let response_packet = expect_single_packet(response);
        assert_eq!(*response_packet.code(), ResponseCode::RequestedEntityTooLarge);
        assert!(operation.is_complete());
    }

    #[fuchsia::test]
    fn multi_packet_put_exceeds_max_object_size_rejected() {
        let mut operation = PutOperation::new_with_max_size(
            /*srm_supported=*/ false, /*max_object_size=*/ 50,
        );

        // First request sends 30 bytes of data (within 50-byte limit).
        let body1 = vec![0u8; 30];
        let headers1 = HeaderSet::from_header(Header::Body(body1));
        let request1 = RequestPacket::new_put(headers1);
        let response1 = operation.handle_peer_request(request1).expect("valid request");
        let response_packet1 = expect_single_packet(response1);
        assert_eq!(*response_packet1.code(), ResponseCode::Continue);
        assert!(!operation.is_complete());

        // Second request sends another 30 bytes (total 60 bytes > 50-byte limit).
        let body2 = vec![0u8; 30];
        let headers2 = HeaderSet::from_header(Header::Body(body2));
        let request2 = RequestPacket::new_put(headers2);
        let response2 = operation.handle_peer_request(request2).expect("valid request");
        let response_packet2 = expect_single_packet(response2);
        assert_eq!(*response_packet2.code(), ResponseCode::RequestedEntityTooLarge);
        assert!(operation.is_complete());
    }

    #[fuchsia::test]
    fn put_request_headers_only_exceeds_max_object_size_rejected() {
        let mut operation = PutOperation::new_with_max_size(
            /*srm_supported=*/ false, /*max_object_size=*/ 100,
        );

        // Put request containing only informational headers (no Body/EndOfBody data)
        // with a Length header specifying a payload larger than 100 bytes.
        let headers = HeaderSet::from_headers(vec![
            Header::name("header_only.txt"),
            Header::Type("text/plain".into()),
            Header::Length(105),
        ])
        .unwrap();
        let request = RequestPacket::new_put(headers);
        let response = operation.handle_peer_request(request).expect("valid request");
        let response_packet = expect_single_packet(response);
        assert_eq!(*response_packet.code(), ResponseCode::RequestedEntityTooLarge);
        assert!(operation.is_complete());
    }
}
