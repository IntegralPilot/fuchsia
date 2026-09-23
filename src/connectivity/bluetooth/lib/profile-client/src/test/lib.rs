// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ServerEnd;
use fidl_fuchsia_bluetooth as fidl_bt;
use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_bluetooth::types::{self as bt, PeerId};
use futures::{Stream, StreamExt};
use profile_client::ProfileClient;
use std::pin::Pin;
use std::task::{Context, Poll};

pub struct TestProfileServerEndpoints {
    pub proxy: bredr::ProfileProxy,
    pub client: ProfileClient,
    pub test_server: TestProfileServer,
}

/// Used to specify the channel to expect on an incoming Connect message
#[derive(Debug)]
pub enum ConnectChannel {
    L2CapPsm(u16),
    RfcommChannel(u8), // Valid channels are 1-30
}

/// Holds all the server side resources associated with a `Profile`'s connection to
/// fuchsia.bluetooth.bredr.Profile. Provides helper methods for common test related tasks.
/// Some fields are optional because they are not populated until the Profile has completed
/// registration.
// TODO(b/333456020): Clean up `advertise_responder`
pub struct TestProfileServer {
    profile_request_stream: bredr::ProfileRequestStream,
    search_results_proxy: Option<bredr::SearchResultsProxy>,
    connection_receiver_proxy: Option<bredr::ConnectionReceiverProxy>,
    advertise_responder: Option<bredr::ProfileAdvertiseResponder>,
}

impl From<bredr::ProfileRequestStream> for TestProfileServer {
    fn from(profile_request_stream: bredr::ProfileRequestStream) -> Self {
        Self {
            profile_request_stream,
            search_results_proxy: None,
            connection_receiver_proxy: None,
            advertise_responder: None,
        }
    }
}

impl TestProfileServer {
    /// Create a new Profile proxy and stream, and create a profile client that wraps the proxy and a
    /// test server that wraps the stream.
    ///
    /// If service_class_profile_id is Some, add a search for that service class.
    ///
    /// If service_definition is Some, advertise with that service definition.
    ///
    /// Returns a struct containing the proxy, profile client and test server.
    pub fn new(
        service_definition: Option<bredr::ServiceDefinition>,
        service_class_profile_id: Option<bredr::ServiceClassProfileIdentifier>,
    ) -> TestProfileServerEndpoints {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<bredr::ProfileMarker>();

        let mut client = match service_definition {
            None => ProfileClient::new(proxy.clone()),
            Some(service_definition) => {
                let channel_params = fidl_bt::ChannelParameters::default();
                ProfileClient::advertise(proxy.clone(), vec![service_definition], channel_params)
                    .expect("Failed to advertise.")
            }
        };

        if let Some(service_class_profile_id) = service_class_profile_id {
            client.add_search(service_class_profile_id, None).expect("Failed to search for peers.");
        }

        let test_server = TestProfileServer::from(stream);

        TestProfileServerEndpoints { proxy, client, test_server }
    }

    pub async fn expect_search(&mut self) {
        let request = self.profile_request_stream.next().await;
        match request {
            Some(Ok(bredr::ProfileRequest::Search { payload, .. })) => {
                self.search_results_proxy = Some(payload.results.unwrap().into_proxy());
            }
            _ => panic!(
                "unexpected result on profile request stream while waiting for search: {request:?}"
            ),
        }
    }

    pub async fn expect_advertise(&mut self) {
        let request = self.profile_request_stream.next().await;
        match request {
            Some(Ok(bredr::ProfileRequest::Advertise { payload, responder, .. })) => {
                self.connection_receiver_proxy = Some(payload.receiver.unwrap().into_proxy());
                if let Some(_old_responder) = self.advertise_responder.replace(responder) {
                    panic!("Got new advertise request before old request is complete.");
                }
            }
            _ => panic!(
                "unexpected result on profile request stream while waiting for advertisement: {request:?}"
            ),
        }
    }

    pub async fn expect_connect(
        &mut self,
        expected_channel: Option<ConnectChannel>,
    ) -> bt::Channel {
        let request = self.profile_request_stream.next().await;
        match request {
            Some(Ok(bredr::ProfileRequest::Connect { connection, responder, .. })) => {
                match (expected_channel, connection) {
                    (None, _) => {}
                    (
                        Some(ConnectChannel::L2CapPsm(expected_psm)),
                        bredr::ConnectParameters::L2cap(bredr::L2capParameters {
                            psm: psm_option,
                            ..
                        }),
                    ) => assert_eq!(Some(expected_psm), psm_option),
                    (
                        Some(ConnectChannel::RfcommChannel(expected_channel)),
                        bredr::ConnectParameters::Rfcomm(bredr::RfcommParameters {
                            channel: channel_option,
                            ..
                        }),
                    ) => assert_eq!(Some(expected_channel), channel_option),
                    (expected_channel, connection) => {
                        panic!("On connect, expected {expected_channel:?}, got {connection:?}")
                    }
                }

                let (near_bt_channel, far_bt_channel) = bt::Channel::create_socket_pair();
                let far_bredr_channel: bredr::Channel =
                    far_bt_channel.try_into().expect("BT Channel into FIDL BREDR Channel");
                responder.send(Ok(far_bredr_channel)).expect("Send channel");
                near_bt_channel
            }
            _ => panic!(
                "Unexpected result on profile request stream expecting connection: {request:?}",
            ),
        }
    }

    pub async fn expect_sco_connect(
        &mut self,
        expected_initiator: bool,
    ) -> ServerEnd<bredr::ScoConnectionMarker> {
        let request = self.profile_request_stream.next().await;
        let connection = match request {
            Some(Ok(bredr::ProfileRequest::ConnectSco {
                payload: bredr::ProfileConnectScoRequest { initiator, connection, .. },
                ..
            })) if initiator == Some(expected_initiator) => connection,
            Some(Ok(bredr::ProfileRequest::ConnectSco {
                payload: bredr::ProfileConnectScoRequest { initiator, .. },
                ..
            })) => {
                panic!(
                    "Got SCO connection request expected initatior: {expected_initiator:}, actual initiator: {initiator:?}"
                );
            }
            _ => panic!(
                "Unexpected result on profile request stream expecting SCO connection: {request:?}",
            ),
        };

        connection.expect("Got no connection when expecting SCO connection.")
    }

    pub fn send_service_found(
        &mut self,
        peer_id: PeerId,
        protocol_list: Option<Vec<bredr::ProtocolDescriptor>>,
        attributes: Vec<bredr::Attribute>,
    ) -> fidl::client::QueryResponseFut<()> {
        let search_results_proxy = self.search_results_proxy.as_ref().expect("Search result proxy");
        search_results_proxy.service_found(&peer_id.into(), protocol_list.as_deref(), &attributes)
    }

    pub fn send_connected(
        &mut self,
        peer_id: PeerId,
        protocol_list: Vec<bredr::ProtocolDescriptor>,
    ) -> bt::Channel {
        let (near_bt_channel, far_bt_channel) = bt::Channel::create_socket_pair();
        let far_bredr_channel: bredr::Channel =
            far_bt_channel.try_into().expect("BT Channel into FIDL BREDR Channel");

        let connection_receiver_proxy =
            self.connection_receiver_proxy.as_ref().expect("Connection receiver proxy");
        connection_receiver_proxy
            .connected(&peer_id.into(), far_bredr_channel, &protocol_list)
            .expect("Connected");

        near_bt_channel
    }
}

/// Expose the underlying ProfileRequestStream
impl Stream for TestProfileServer {
    type Item = Result<bredr::ProfileRequest, fidl::Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let pinned_stream = Pin::new(&mut self.profile_request_stream);
        pinned_stream.poll_next(context)
    }
}

impl Drop for TestProfileServer {
    fn drop(&mut self) {
        // TODO(b/333456020): Clean-up to not store responder.
        if let Some(responder) = self.advertise_responder.take() {
            responder
                .send(Ok(&bredr::ProfileAdvertiseResponse::default()))
                .expect("Drop responder");
        }
    }
}
