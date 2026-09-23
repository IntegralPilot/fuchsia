// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/42077657) Use this in task.rs.
#![allow(unused)]

use anyhow::{Result, format_err};
use at_commands as at;
use at_commands::{DeserializeBytes, SerDe};
use fuchsia_bluetooth::types::{Channel, PeerId};
use futures::stream::FusedStream;
use futures::{SinkExt, Stream};
use log::{debug, warn};
use std::collections::VecDeque;
use std::io::Cursor;
use std::mem;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::peer::ag_indicators::AgIndicatorIndex;
use crate::peer::parse_cind_test;

#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    Recognized(at::Response),
    #[allow(unused)]
    CindTest {
        ordered_indicators: Vec<AgIndicatorIndex>,
    },
}

pub struct Connection {
    peer_id: PeerId,
    rfcomm: Channel,
    unreturned_responses: VecDeque<Response>,
    remaining_bytes: DeserializeBytes,
}

/// Stream for Connection.  The stream produces at::Responses coming in from the peer.  These are
/// yielded one at a time.  While we expect spec-compliant peers to give us one AT response per
/// RFCOMM data transfer, this stream will assemble fragmented AT responses and split multiple AT
/// responses which arrive together into a series of well formed responses.
impl Stream for Connection {
    type Item = Result<Response>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if self.is_terminated() {
                panic!(
                    "Tried to poll AT Connection to peer {} after it was terminated.",
                    self.peer_id
                )
            }

            if let Some(item) = self.unreturned_responses.pop_front() {
                return Poll::Ready(Some(Ok(item)));
            }

            let rfcomm = Pin::new(&mut self.rfcomm);
            let bytes_poll = rfcomm.poll_next(context);
            let bytes = match bytes_poll {
                Poll::Pending => return Poll::Pending, // Nothing to read
                Poll::Ready(None) => return Poll::Ready(None), // Channel is closed
                Poll::Ready(Some(Err(err))) => return Poll::Ready(Some(Err(format_err!(err)))), // zx::Status indicating error
                Poll::Ready(Some(Ok(bytes))) if bytes.len() == 0 => continue, // Got no bytes; try again
                Poll::Ready(Some(Ok(bytes))) => bytes,                        // Received bytes
            };

            let mut cursor = Cursor::new(&bytes);
            let remaining_bytes = mem::take(&mut self.remaining_bytes);
            let at::DeserializeResult {
                values: deserialized_values,
                error: deserialize_error,
                remaining_bytes,
            } = at::Response::deserialize(&mut cursor, remaining_bytes);
            self.remaining_bytes = remaining_bytes;

            let mut parsed_responses =
                deserialized_values.into_iter().map(Response::Recognized).collect::<VecDeque<_>>();
            self.unreturned_responses.append(&mut parsed_responses);
            // This will be returned on a future loop.

            if let Some(error) = deserialize_error {
                // In this case, we may have other commands that deserialized correctly, so continue.
                // This is probably an AT command that isn't specified in a .at file, so try manual parsing.
                debug!(
                    "Could not deserialize AT response received from peer {}: {:?}",
                    self.peer_id, error
                );
                let hand_parsed_response = Self::parse_unparsed_bytes(error.bytes.clone());

                match hand_parsed_response {
                    Ok(resp) => self.unreturned_responses.push_back(resp),
                    err @ Err(_) => return Poll::Ready(Some(Err(error.into()))), // Return original error
                }
            }

            // Loop.
        }
    }
}

impl FusedStream for Connection {
    fn is_terminated(&self) -> bool {
        self.unreturned_responses.is_empty() && self.rfcomm.is_terminated()
    }
}

impl Connection {
    pub fn new(peer_id: PeerId, rfcomm: Channel) -> Self {
        Self {
            peer_id,
            rfcomm,
            unreturned_responses: VecDeque::new(),
            remaining_bytes: at::DeserializeBytes::new(),
        }
    }

    /// Serializes the AT commands and sends them through the RFCOMM channel
    pub async fn write_commands(&mut self, commands: &[at::Command]) -> Result<()> {
        if commands.len() > 0 {
            let mut bytes = Vec::new();
            at::Command::serialize(&mut bytes, commands).map_err(|err| {
                format_err!(
                    "Failed to serialize AT commands to channel for peer {:}: {:?}",
                    self.peer_id,
                    err
                )
            })?;
            self.rfcomm.send(bytes).await.map_err(|err| {
                format_err!(
                    "Could not write serialized AT commands to channel for peer {:}: {:?}",
                    self.peer_id,
                    err
                )
            })?;
        }

        Ok(())
    }

    fn parse_unparsed_bytes(bytes: Vec<u8>) -> Result<Response> {
        let response = parse_cind_test::parse(bytes)?;
        // Add more parses here if necessary.
        Ok(response)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use async_utils::PollExt;
    use bt_channel_test_support::{Transport, create_test_channels};
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use test_case::test_case;

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn at_response_received(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut near, far) = create_test_channels(transport);

        let mut conn = Connection::new(PeerId(1), far);

        let response_bytes = "+BRSF:0\r".as_bytes();
        exec.run_singlethreaded(near.send(response_bytes.to_vec())).expect("Sent AT");

        let response = exec
            .run_singlethreaded(conn.next())
            .expect("Received channel read closed error")
            .expect("Received channel read Zircon error");

        let expected_response =
            Response::Recognized(at::Response::Success(at::Success::Brsf { features: 0 }));
        assert_eq!(response, expected_response);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn unparsed_response_received(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut near, far) = create_test_channels(transport);

        let mut conn = Connection::new(PeerId(1), far);

        let response_bytes = "+CIND: (\"service\",(0,1))\r".as_bytes();
        exec.run_singlethreaded(near.send(response_bytes.to_vec())).expect("Sent AT");

        let response = exec
            .run_singlethreaded(conn.next())
            .expect("Received channel read closed error")
            .expect("Received channel read Zircon error");

        let expected_response =
            Response::CindTest { ordered_indicators: vec![AgIndicatorIndex::ServiceAvailable] };
        assert_eq!(response, expected_response);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn at_responses_received(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut near, far) = create_test_channels(transport);

        let mut conn = Connection::new(PeerId(1), far);

        let response_bytes = "+BRSF:1\r+BRSF:2\r".as_bytes();
        exec.run_singlethreaded(near.send(response_bytes.to_vec())).expect("Sent AT");

        let response_1 = exec
            .run_singlethreaded(conn.next())
            .expect("Received channel read closed error")
            .expect("Received channel read Zircon error");

        let expected_response_1 =
            Response::Recognized(at::Response::Success(at::Success::Brsf { features: 1 }));
        assert_eq!(response_1, expected_response_1);

        let response_2 = exec
            .run_singlethreaded(conn.next())
            .expect("Received AT connection closed error")
            .expect("Received AT connection Zircon error");

        let expected_response_2 =
            Response::Recognized(at::Response::Success(at::Success::Brsf { features: 2 }));
        assert_eq!(response_2, expected_response_2);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn at_response_received_and_defragmented(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut near, far) = create_test_channels(transport);

        let mut conn = Connection::new(PeerId(1), far);

        let response_bytes_1 = "+BRS".as_bytes();
        let response_bytes_2 = "F:0\r".as_bytes();

        exec.run_singlethreaded(near.send(response_bytes_1.to_vec())).expect("Sent AT 1");

        exec.run_until_stalled(&mut conn.next()).expect_pending("Reading AT fragmanet");

        exec.run_singlethreaded(near.send(response_bytes_2.to_vec())).expect("Sent AT 2");

        let response = exec
            .run_singlethreaded(&mut conn.next())
            .expect("Received channel read closed error")
            .expect("Received channel read Zircon error");

        let expected_response =
            Response::Recognized(at::Response::Success(at::Success::Brsf { features: 0 }));
        assert_eq!(response, expected_response);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn at_commands_written(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut near, far) = create_test_channels(transport);

        let mut conn = Connection::new(PeerId(1), far);

        let command_1 = at::Command::Brsf { features: 1 };
        let command_2 = at::Command::Brsf { features: 2 };

        exec.run_singlethreaded(conn.write_commands(&[command_1, command_2]))
            .expect("Sent commands");

        let commands_bytes = exec
            .run_singlethreaded(near.next())
            .expect("Received channel read closed error")
            .expect("Received channel read Zircon error");

        let expected_commands_bytes: Vec<u8> = "AT+BRSF=1\rAT+BRSF=2\r".into();

        assert_eq!(commands_bytes, expected_commands_bytes);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn stream_terminates_when_responses_are_consumed(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (mut near, far) = create_test_channels(transport);

        let mut conn = Connection::new(PeerId(1), far);

        let response_bytes = "+BRSF:0\r".as_bytes();
        exec.run_singlethreaded(near.send(response_bytes.to_vec())).expect("Sent AT");

        // RFCOMM is open and response isn't yet consumed.
        assert!(!conn.is_terminated());

        drop(near);

        // RFCOMM is closed and response isn't yet consumed.
        assert!(!conn.is_terminated());

        let response = exec
            .run_singlethreaded(conn.next())
            .expect("Received channel read closed error")
            .expect("Received channel read Zircon error");

        let expected_response =
            Response::Recognized(at::Response::Success(at::Success::Brsf { features: 0 }));
        assert_eq!(response, expected_response);

        // Read again to get a closed channel
        let response = exec.run_singlethreaded(conn.next());
        assert!(response.is_none());

        // RFCOMM is closed and response is consumed.
        assert!(conn.is_terminated());
    }
}
