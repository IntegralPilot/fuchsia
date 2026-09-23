// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::pin::Pin;
use core::task::{Context, Poll};
use fidl_fuchsia_bluetooth_deviceid as di;
use futures::future::FusedFuture;
use futures::stream::StreamFuture;
use futures::{Future, FutureExt, StreamExt};
use log::{debug, info, warn};

use crate::device_id::server::BrEdrProfileAdvertisement;
use crate::device_id::service_record::DeviceIdentificationService;

/// A token representing a FIDL client's Device Identification advertisement request.
pub struct DeviceIdRequestToken {
    /// The DI service that was requested by the client to be advertised.
    service: DeviceIdentificationService,
    /// The upstream BR/EDR advertisement request.
    advertisement: Option<BrEdrProfileAdvertisement>,
    /// The channel representing the FIDL client's request.
    client_request: Option<StreamFuture<di::DeviceIdentificationHandleRequestStream>>,
    /// The responder used to notify the FIDL client that the request has terminated.
    ///
    /// This will be Some<T> as long as the `DeviceIdRequestToken::Future` is active, and will be
    /// consumed when the token is dropped or upstream advertisement finished.
    responder: Option<di::DeviceIdentificationSetDeviceIdentificationResponder>,
}

impl DeviceIdRequestToken {
    pub fn new(
        service: DeviceIdentificationService,
        advertisement: BrEdrProfileAdvertisement,
        client_request: di::DeviceIdentificationHandleRequestStream,
        responder: di::DeviceIdentificationSetDeviceIdentificationResponder,
    ) -> Self {
        Self {
            service,
            advertisement: Some(advertisement),
            client_request: Some(client_request.into_future()),
            responder: Some(responder),
        }
    }

    pub fn size(&self) -> usize {
        self.service.size()
    }

    pub fn contains_primary(&self) -> bool {
        self.service.contains_primary()
    }

    // Notifies the `responder` when the request has been closed.
    fn notify_responder(responder: di::DeviceIdentificationSetDeviceIdentificationResponder) {
        let _ = responder.send(Ok(()));
        info!("DeviceIdRequestToken successfully closed");
    }
}

impl Future for DeviceIdRequestToken {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut advertisement = self.advertisement.take().expect("can't poll future twice");
        let mut client_request = self.client_request.take().expect("can't poll future twice");
        if let Poll::Ready(res) = advertisement.poll_unpin(cx) {
            debug!("Upstream BR/EDR server terminated DI advertisement: {:?}", res);
            Self::notify_responder(self.responder.take().expect("responder exists"));
            return Poll::Ready(());
        }

        match client_request.poll_unpin(cx) {
            Poll::Ready((Some(_req), _s)) => {
                panic!("Unexpected request in DI Token stream: {:?}", _req)
            }
            Poll::Ready((None, _s)) => {
                debug!("DI FIDL client closed token channel");
                Self::notify_responder(self.responder.take().expect("responder exists"));
                Poll::Ready(())
            }
            Poll::Pending => {
                self.advertisement = Some(advertisement);
                self.client_request = Some(client_request);
                Poll::Pending
            }
        }
    }
}

impl FusedFuture for DeviceIdRequestToken {
    fn is_terminated(&self) -> bool {
        // The token is considered terminated if either the upstream BR/EDR Profile advertisement
        // or FIDL client request is terminated.
        self.advertisement.is_none() || self.client_request.is_none()
    }
}

impl Drop for DeviceIdRequestToken {
    fn drop(&mut self) {
        if let Some(responder) = self.responder.take() {
            warn!(service:? = self.service; "DeviceIdRequestToken unexpectedly dropped");
            let _ = responder.send(Err(zx::Status::CANCELED.into_raw()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use async_utils::PollExt;
    use fidl::client::QueryResponseFut;
    use fidl::endpoints::Proxy;
    use fidl_fuchsia_bluetooth_bredr::{ConnectionReceiverMarker, ConnectionReceiverProxy};
    use fuchsia_async as fasync;
    use std::pin::pin;

    use crate::device_id::service_record::tests::minimal_record;

    fn make_set_device_id_request(
        exec: &mut fasync::TestExecutor,
    ) -> (
        di::DeviceIdentificationRequest,
        di::DeviceIdentificationHandleProxy,
        QueryResponseFut<Result<(), i32>>,
    ) {
        let (c, mut s) =
            fidl::endpoints::create_proxy_and_stream::<di::DeviceIdentificationMarker>();

        let records = &[minimal_record(false)];
        let (token_client, token_server) =
            fidl::endpoints::create_proxy::<di::DeviceIdentificationHandleMarker>();
        let request_fut =
            c.set_device_identification(records, token_server).check().expect("valid fidl request");

        let mut next = Box::pin(s.next());
        match exec.run_singlethreaded(&mut next).expect("fidl request") {
            Ok(request) => (request, token_client, request_fut),
            x => panic!("Expected SetDeviceIdentification request but got: {:?}", x),
        }
    }

    /// Makes a DeviceIdRequestToken from the provided `responder`.
    /// Returns the token and the `ConnectionReceiver` client that represents the upstream
    /// `bredr.Profile` end of the Device ID advertisement.
    fn make_token(
        responder: di::DeviceIdentificationSetDeviceIdentificationResponder,
        request_server: di::DeviceIdentificationHandleRequestStream,
    ) -> (DeviceIdRequestToken, ConnectionReceiverProxy) {
        // Use a random service definition.
        let svc = DeviceIdentificationService::from_di_records(&vec![minimal_record(false)])
            .expect("should parse record");
        // A BR/EDR advertise request.
        let (connect_client, connect_stream) =
            fidl::endpoints::create_proxy_and_stream::<ConnectionReceiverMarker>();
        let advertisement = BrEdrProfileAdvertisement { connect_stream };
        let token = DeviceIdRequestToken::new(svc, advertisement, request_server, responder);

        (token, connect_client)
    }

    #[fuchsia::test]
    fn responder_notified_with_error_on_token_drop() {
        let mut exec = fasync::TestExecutor::new();

        let (request, _request_client, client_fut) = make_set_device_id_request(&mut exec);
        let (_record, request_server, responder) =
            request.into_set_device_identification().expect("set device id request");
        let mut client_fut = pin!(client_fut);

        let (token, _connect_client) = make_token(responder, request_server.into_stream());

        // The client request should still be alive since the `token` is alive.
        exec.run_until_stalled(&mut client_fut).expect_pending("Token is still alive");
        // Token unexpectedly dropped - FIDL client should be notified with Error.
        drop(token);
        let res = exec
            .run_until_stalled(&mut client_fut)
            .expect("Token dropped, client fut should resolve")
            .expect("fidl response");
        assert_eq!(res, Err(zx::Status::CANCELED.into_raw()));
    }

    #[fuchsia::test]
    fn token_terminates_when_upstream_advertisement_terminates() {
        let mut exec = fasync::TestExecutor::new();

        let (request, _request_client, client_fut) = make_set_device_id_request(&mut exec);
        let (_record, request_server, responder) =
            request.into_set_device_identification().expect("set device id request");
        let mut client_fut = pin!(client_fut);

        let (token, connect_client) = make_token(responder, request_server.into_stream());
        let mut token = pin!(token);

        // The client request should still be alive since the `token` is alive.
        exec.run_until_stalled(&mut client_fut).expect_pending("Token is still alive");

        // Simulate upstream `Profile` server terminating the advertisement.
        drop(connect_client);
        exec.run_until_stalled(&mut token)
            .expect("Upstream advertisement done, token should resolve");
        assert!(token.is_terminated());
        // The FIDL client should then receive the response that the closure has been processed.
        let res = exec
            .run_until_stalled(&mut client_fut)
            .expect("Token dropped, client fut should resolve")
            .expect("fidl response");
        assert_eq!(res, Ok(()));
    }

    #[fuchsia::test]
    fn token_terminates_when_fidl_client_closes_channel() {
        let mut exec = fasync::TestExecutor::new();

        let (request, _request_client, client_fut) = make_set_device_id_request(&mut exec);
        let (_record, request_server, responder) =
            request.into_set_device_identification().expect("set device id request");
        let mut client_fut = pin!(client_fut);

        let (token, connect_client) = make_token(responder, request_server.into_stream());
        let mut token = pin!(token);

        assert!(!token.is_terminated());

        // The client request should still be alive since the `token` is alive.
        exec.run_until_stalled(&mut client_fut)
            .expect_pending("Client request still waiting for response");
        exec.run_until_stalled(&mut token).expect_pending("Token still waiting for closure");

        // Because the FIDL client closed its end of the channel, we expect the `token` to resolve.
        drop(_request_client);
        exec.run_until_stalled(&mut token)
            .expect("FIDL client request terminated, token should resolve");
        assert!(token.is_terminated());
        // The FIDL client should receive a response indicating that the DI advertisement has been
        // stopped.
        let res = exec
            .run_until_stalled(&mut client_fut)
            .expect("Token terminated, client fut should resolve")
            .expect("fidl response");
        assert_matches!(res, Ok(_));
        // The DI advertisement should be unregistered from the upstream `bredr.Profile` server.
        let mut closed_fut = pin!(connect_client.on_closed());
        let result = exec.run_until_stalled(&mut closed_fut).expect("ConnectionReceiver closed");
        assert_matches!(result, Ok(_));
    }
}
