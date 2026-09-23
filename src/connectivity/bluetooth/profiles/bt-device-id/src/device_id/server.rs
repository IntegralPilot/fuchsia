// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::pin::Pin;
use core::task::{Context, Poll};
use futures::channel::mpsc;
use futures::stream::{FlattenUnordered, FuturesUnordered};
use futures::{ready, select, Future, StreamExt};
use log::{info, trace, warn};
use {fidl_fuchsia_bluetooth_bredr as bredr, fidl_fuchsia_bluetooth_deviceid as di};

use crate::device_id::service_record::{DIRecord, DeviceIdentificationService};
use crate::device_id::token::DeviceIdRequestToken;
use crate::error::Error;

/// Represents a service advertisement with the upstream BR/EDR Profile server.
pub struct BrEdrProfileAdvertisement {
    // Although we don't expect any stream items (connection requests), this channel must be kept
    // alive for the duration of the advertisement.
    // The upstream `Profile` service may close this channel to signal the termination of the
    // advertisement.
    pub connect_stream: bredr::ConnectionReceiverRequestStream,
}

impl Future for BrEdrProfileAdvertisement {
    type Output = Result<(), fidl::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            let result = ready!(self.connect_stream.poll_next_unpin(cx));
            match result {
                None => return Poll::Ready(Ok(())),
                Some(Err(e)) => return Poll::Ready(Err(e)),
                // Otherwise, it's a spurious `bredr.ConnectionReceiver.Connect` request which
                // can be ignored.
                _ => {}
            }
        }
    }
}

/// The server that manages the current set of Device Identification advertisements.
pub struct DeviceIdServer {
    /// The maximum number of concurrent DI advertisements this server supports.
    max_advertisements: usize,
    /// The default `DIRecord` that will be advertised on startup, if set.
    default: Option<DIRecord>,
    /// The connection to the upstream BR/EDR Profile server.
    profile: bredr::ProfileProxy,
    /// FIDL client connection requests to the `DeviceIdentification` protocol.
    device_id_requests: FlattenUnordered<mpsc::Receiver<di::DeviceIdentificationRequestStream>>,
    /// The default DI advertisement - this is Some<T> on component startup. None if a FIDL client
    /// has requested to set the DI advertisement.
    default_di_advertisement: Option<BrEdrProfileAdvertisement>,
    /// The current set of managed DI advertisements.
    device_id_advertisements: FuturesUnordered<DeviceIdRequestToken>,
}

impl DeviceIdServer {
    pub fn new(
        max_advertisements: usize,
        default: Option<DIRecord>,
        profile: bredr::ProfileProxy,
        device_id_clients: mpsc::Receiver<di::DeviceIdentificationRequestStream>,
    ) -> Self {
        Self {
            max_advertisements,
            default,
            profile,
            device_id_requests: device_id_clients.flatten_unordered(None),
            default_di_advertisement: None,
            device_id_advertisements: FuturesUnordered::new(),
        }
    }

    /// Returns the current number of DI records advertised.
    fn advertisement_size(&self) -> usize {
        self.device_id_advertisements.iter().map(|adv| adv.size()).sum()
    }

    fn contains_primary(&self) -> bool {
        self.device_id_advertisements.iter().filter(|adv| adv.contains_primary()).count() != 0
    }

    /// Returns true if the server has enough space to accommodate the `requested_space`.
    fn has_space(&self, requested_space: usize) -> bool {
        self.advertisement_size() + requested_space <= self.max_advertisements
    }

    /// Advertises the default Device Identification provided to the server on startup.
    fn make_default_advertisement(&mut self) -> Result<(), Error> {
        let Some(record) = self.default.as_ref() else { return Ok(()) };
        trace!(record:?; "Publishing default DI record");
        let bredr_record: bredr::ServiceDefinition = record.into();
        let advertisement = Self::advertise(&self.profile, vec![bredr_record.clone()])?;
        self.default_di_advertisement = Some(advertisement);
        info!(bredr_record:?; "Successfully published DI record");
        Ok(())
    }

    pub async fn run(mut self) -> Result<(), Error> {
        self.make_default_advertisement()?;
        loop {
            select! {
                device_id_request = self.device_id_requests.select_next_some() => {
                    match device_id_request {
                        Ok(req) => {
                            match self.handle_device_id_request(req) {
                                Ok(token) => {
                                    // Remove the default advertisement if we've successfully built
                                    // the FIDL client DI request.
                                    self.default_di_advertisement = None;
                                    self.device_id_advertisements.push(token);
                                }
                                Err(e) => info!("Couldn't handle DI request: {:?}", e),
                            }
                        }
                        Err(e) => warn!("Error receiving DI request: {:?}", e),
                    }
                }
                _finished = self.device_id_advertisements.select_next_some() => {}
                complete => {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Advertises the provided service `defs` with the BR/EDR `profile` service. Returns a Future
    /// representing the advertisement on success, Error otherwise.
    fn advertise(
        profile: &bredr::ProfileProxy,
        defs: Vec<bredr::ServiceDefinition>,
    ) -> Result<BrEdrProfileAdvertisement, Error> {
        let (connect_client, connect_stream) =
            fidl::endpoints::create_request_stream::<bredr::ConnectionReceiverMarker>();
        // The result of advertise (registered services) is not needed.
        let _advertise_fut = profile.advertise(bredr::ProfileAdvertiseRequest {
            services: Some(defs),
            receiver: Some(connect_client),
            ..Default::default()
        });
        Ok(BrEdrProfileAdvertisement { connect_stream })
    }

    /// Ensures that the `service` can be registered. Returns true if the service needs to be
    /// marked as primary, false otherwise.
    fn validate_service(&self, service: &DeviceIdentificationService) -> Result<bool, zx::Status> {
        // Ensure that the server has space for the requested records.
        if !self.has_space(service.size()) {
            return Err(zx::Status::NO_RESOURCES);
        }

        // Ensure that only one service is requesting primary.
        if self.contains_primary() && service.contains_primary() {
            return Err(zx::Status::ALREADY_EXISTS);
        }

        // If this is the first advertisement, it may need to be marked as primary
        let needs_primary = self.advertisement_size() == 0;
        Ok(needs_primary)
    }

    fn handle_device_id_request(
        &self,
        request: di::DeviceIdentificationRequest,
    ) -> Result<DeviceIdRequestToken, Error> {
        let di::DeviceIdentificationRequest::SetDeviceIdentification { records, token, responder } =
            request;
        info!("Received SetDeviceIdentification request: {:?}", records);

        let parsed_result = DeviceIdentificationService::from_di_records(&records);
        let Ok(mut service) = parsed_result else {
            let _ = responder.send(Err(zx::Status::INVALID_ARGS.into_raw()));
            return Err(parsed_result.unwrap_err());
        };

        match self.validate_service(&service) {
            Ok(true) => {
                let _ = service.maybe_update_primary();
            }
            Ok(false) => {}
            Err(e) => {
                let _ = responder.send(Err(e.into_raw()));
                return Err(e.into());
            }
        }

        let client_request = token.into_stream();

        let bredr_advertisement = match Self::advertise(&self.profile, (&service).into()) {
            Err(e) => {
                let _ = responder.send(Err(zx::Status::CANCELED.into_raw()));
                return Err(e);
            }
            Ok(fut) => fut,
        };

        // The lifetime of the request is tied to the FIDL client and the upstream BR/EDR
        // service advertisement.
        Ok(DeviceIdRequestToken::new(service, bredr_advertisement, client_request, responder))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use async_test_helpers::run_while;
    use async_utils::PollExt;
    use fidl::client::QueryResponseFut;
    use fidl::endpoints::Proxy;
    use fuchsia_async as fasync;
    use futures::SinkExt;
    use std::pin::pin;

    use crate::device_id::service_record::tests::minimal_record;
    use crate::DEFAULT_MAX_DEVICE_ID_ADVERTISEMENTS;

    fn setup_server(
        max: Option<usize>,
    ) -> (
        fasync::TestExecutor,
        DeviceIdServer,
        bredr::ProfileRequestStream,
        mpsc::Sender<di::DeviceIdentificationRequestStream>,
    ) {
        let exec = fasync::TestExecutor::new();

        let (sender, receiver) = mpsc::channel(0);
        let (profile, profile_server) =
            fidl::endpoints::create_proxy_and_stream::<bredr::ProfileMarker>();
        let max = max.unwrap_or(DEFAULT_MAX_DEVICE_ID_ADVERTISEMENTS);
        let server = DeviceIdServer::new(max, None, profile, receiver);

        (exec, server, profile_server, sender)
    }

    fn connect_fidl_client<Fut>(
        exec: &mut fasync::TestExecutor,
        server_fut: Fut,
        mut sender: mpsc::Sender<di::DeviceIdentificationRequestStream>,
    ) -> (Fut, di::DeviceIdentificationProxy)
    where
        Fut: Future + Unpin,
    {
        let (c, s) = fidl::endpoints::create_proxy_and_stream::<di::DeviceIdentificationMarker>();

        let send_fut = sender.send(s);
        let (send_result, server_fut) = run_while(exec, server_fut, send_fut);
        assert_matches!(send_result, Ok(_));

        (server_fut, c)
    }

    /// Makes a SetDeviceIdentification request to the server.
    /// `primary` specifies if the requested DI record is the primary record.
    fn make_request(
        client: &di::DeviceIdentificationProxy,
        primary: bool,
    ) -> (di::DeviceIdentificationHandleProxy, QueryResponseFut<Result<(), i32>>) {
        let records = &[minimal_record(primary)];
        let (token_client, token_server) =
            fidl::endpoints::create_proxy::<di::DeviceIdentificationHandleMarker>();
        let request_fut = client
            .set_device_identification(records, token_server)
            .check()
            .expect("valid fidl request");
        (token_client, request_fut)
    }

    #[track_caller]
    fn expect_advertise_request(
        exec: &mut fasync::TestExecutor,
        profile_server: &mut bredr::ProfileRequestStream,
    ) -> bredr::ConnectionReceiverProxy {
        let mut profile_requests = Box::pin(profile_server.next());
        match exec.run_until_stalled(&mut profile_requests).expect("Should have request") {
            Some(Ok(bredr::ProfileRequest::Advertise { payload, responder, .. })) => {
                let _ = responder.send(Ok(&bredr::ProfileAdvertiseResponse::default()));
                payload.receiver.unwrap().into_proxy()
            }
            x => panic!("Expected Advertise request, got: {x:?}"),
        }
    }

    #[fuchsia::test]
    fn fidl_client_terminates_advertisement() {
        let (mut exec, server, mut profile_server, sender) = setup_server(None);
        let server_fut = pin!(server.run());

        // Connect a new FIDL client and make a SetDeviceId request.
        let (mut server_fut, client) = connect_fidl_client(&mut exec, server_fut, sender.clone());
        let (token, fidl_client_fut) = make_request(&client, false);
        // Run the server task to process the request.
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server still active");
        let mut fidl_client_fut = pin!(fidl_client_fut);

        // Should expect the server to attempt to advertise over BR/EDR.
        let upstream_receiver = expect_advertise_request(&mut exec, &mut profile_server);

        // FIDL client request should still be alive because the server is still advertising.
        let _ = exec.run_until_stalled(&mut fidl_client_fut).expect_pending("still active");
        // Client no longer wants to advertise - request should terminate.
        drop(token);
        let (fidl_client_result, _server_fut) = run_while(&mut exec, server_fut, fidl_client_fut);
        assert_matches!(fidl_client_result, Ok(Ok(_)));
        // Upstream receiver should be closed.
        let mut closed_fut = pin!(upstream_receiver.on_closed());
        let result = exec.run_until_stalled(&mut closed_fut).expect("receiver should be closed");
        assert_matches!(result, Ok(_));
    }

    #[fuchsia::test]
    fn fidl_client_notified_when_upstream_advertisement_terminated() {
        let (mut exec, server, mut profile_server, sender) = setup_server(None);
        let server_fut = pin!(server.run());

        // Connect a new FIDL client and make a SetDeviceId request.
        let (mut server_fut, client) = connect_fidl_client(&mut exec, server_fut, sender.clone());
        let (_token, fidl_client_fut) = make_request(&client, false);
        // Run the server task to process the request.
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server still active");
        let mut fidl_client_fut = pin!(fidl_client_fut);

        // Should expect the server to attempt to advertise over BR/EDR.
        let receiver = expect_advertise_request(&mut exec, &mut profile_server);
        // FIDL client request should still be alive because the server is still advertising.
        let _ = exec.run_until_stalled(&mut fidl_client_fut).expect_pending("still active");

        // Upstream BR/EDR Profile server terminates advertisement for some reason.
        drop(receiver);
        let (fidl_client_result, _server_fut) = run_while(&mut exec, server_fut, fidl_client_fut);
        assert_matches!(fidl_client_result, Ok(Ok(_)));
    }

    #[fuchsia::test]
    fn multiple_clients_advertise_success() {
        let (mut exec, server, mut profile_server, sender) = setup_server(None);
        let server_fut = pin!(server.run());

        // Connect two FIDL clients and make two SetDeviceId requests.
        let (server_fut, client1) = connect_fidl_client(&mut exec, server_fut, sender.clone());
        let (mut server_fut, client2) = connect_fidl_client(&mut exec, server_fut, sender.clone());
        let (_token1, fidl_client_fut1) = make_request(&client1, false);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server still active");
        let (_token2, fidl_client_fut2) = make_request(&client2, false);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server still active");
        let mut fidl_client_fut1 = pin!(fidl_client_fut1);
        let mut fidl_client_fut2 = pin!(fidl_client_fut2);

        // Both FIDL client advertise requests shouldn't resolve yet as the advertisement is OK.
        let _ = exec.run_until_stalled(&mut fidl_client_fut1).expect_pending("still active");
        let _ = exec.run_until_stalled(&mut fidl_client_fut2).expect_pending("still active");

        // Upstream BR/EDR Profile server should receive both requests.
        let _adv_receiver1 = expect_advertise_request(&mut exec, &mut profile_server);
        let _adv_receiver2 = expect_advertise_request(&mut exec, &mut profile_server);

        // Both DI advertise requests should be active.
        let _ = exec.run_until_stalled(&mut fidl_client_fut1).expect_pending("still active");
        let _ = exec.run_until_stalled(&mut fidl_client_fut2).expect_pending("still active");
    }

    #[fuchsia::test]
    fn client_advertisement_rejected_when_server_full() {
        // Make a server with no capacity.
        let (mut exec, server, _profile_server, sender) = setup_server(Some(0));
        let server_fut = pin!(server.run());

        let (mut server_fut, client) = connect_fidl_client(&mut exec, server_fut, sender.clone());
        let (_token, fidl_client_fut) = make_request(&client, false);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server still active");

        // The FIDL client request should immediately resolve as the server cannot register it.
        let (fidl_client_result, _server_fut) = run_while(&mut exec, server_fut, fidl_client_fut);
        let expected_err = zx::Status::NO_RESOURCES.into_raw();
        assert_eq!(fidl_client_result.expect("fidl result is ok"), Err(expected_err));
    }

    #[fuchsia::test]
    fn second_client_rejected_when_requesting_primary() {
        let (mut exec, server, _profile_server, sender) = setup_server(None);
        let server_fut = pin!(server.run());

        // First FIDL client makes a request with a primary record.
        let (mut server_fut, client1) = connect_fidl_client(&mut exec, server_fut, sender.clone());
        let (_token1, fidl_client_fut1) = make_request(&client1, true);
        let mut fidl_client_fut1 = pin!(fidl_client_fut1);
        let _ = exec.run_until_stalled(&mut server_fut).expect_pending("server still active");
        let _ = exec
            .run_until_stalled(&mut fidl_client_fut1)
            .expect_pending("advertisement still active");

        // Second FIDL client tries to make a request with another primary record - should be rejected.
        let (server_fut, client2) = connect_fidl_client(&mut exec, server_fut, sender.clone());
        let (_token2, fidl_client_fut2) = make_request(&client2, true);
        let fidl_client_fut2 = pin!(fidl_client_fut2);
        // The FIDL client request should immediately resolve as the server cannot register it.
        let (fidl_client_result, _server_fut) = run_while(&mut exec, server_fut, fidl_client_fut2);
        let expected_err = zx::Status::ALREADY_EXISTS.into_raw();
        assert_eq!(fidl_client_result.expect("fidl result is ok"), Err(expected_err));
    }

    /// Verifies that a default advertisement is published on server startup and is overwritten by
    /// a `DeviceIdentification` FIDL client's advertise request.
    #[fuchsia::test]
    fn default_advertisement_is_replaced_by_fidl_advertisement() {
        let mut exec = fasync::TestExecutor::new();

        let (sender, receiver) = mpsc::channel(0);
        let (profile, mut profile_server) =
            fidl::endpoints::create_proxy_and_stream::<bredr::ProfileMarker>();
        let default = (&minimal_record(true)).try_into().ok();
        let server =
            DeviceIdServer::new(DEFAULT_MAX_DEVICE_ID_ADVERTISEMENTS, default, profile, receiver);
        let mut server_fut = pin!(server.run());

        // Since a default configuration is provided, the server should attempt to publish it via
        // the BR/EDR Profile service.
        exec.run_until_stalled(&mut server_fut).expect_pending("server still active");
        let default_upstream_receiver = expect_advertise_request(&mut exec, &mut profile_server);

        // FIDL client connects and requests to advertise.
        let (mut server_fut, client1) = connect_fidl_client(&mut exec, server_fut, sender.clone());
        let (_token1, fidl_client_fut1) = make_request(&client1, true);
        let mut fidl_client_fut1 = pin!(fidl_client_fut1);
        exec.run_until_stalled(&mut server_fut).expect_pending("server still active");
        exec.run_until_stalled(&mut fidl_client_fut1).expect_pending("advertisement still active");
        // Expect the BR/EDR Profile service to receive the advertisement. The previous default
        // advertisement should be terminated by the DeviceID Server.
        let _upstream_receiver = expect_advertise_request(&mut exec, &mut profile_server);
        let mut closed_fut = pin!(default_upstream_receiver.on_closed());
        let result = exec.run_until_stalled(&mut closed_fut).expect("receiver should be closed");
        assert_matches!(result, Ok(_));
    }
}
