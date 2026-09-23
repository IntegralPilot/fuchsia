// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod ap;
pub mod client;

use crate::{MlmeEventStream, MlmeStream, Station};
use anyhow::format_err;
use fidl::endpoints::ServerEnd;
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::future::FutureObj;
use futures::prelude::*;
use futures::select;
use futures::stream::FuturesUnordered;
use log::{error, info, warn};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use wlan_common::timer::{self, ScheduledEvent};
use {
    fidl_fuchsia_wlan_common as fidl_common, fidl_fuchsia_wlan_mlme as fidl_mlme,
    fidl_fuchsia_wlan_sme as fidl_sme,
};

pub type ClientSmeServer = mpsc::UnboundedSender<client::Endpoint>;
pub type ApSmeServer = mpsc::UnboundedSender<ap::Endpoint>;

#[derive(Clone)]
pub enum SmeServer {
    Client(ClientSmeServer),
    Ap(ApSmeServer),
}

async fn serve_generic_sme(
    mut generic_sme_request_stream: <fidl_sme::GenericSmeMarker as fidl::endpoints::ProtocolMarker>::RequestStream,
    mlme_sink: crate::MlmeSink,
    mut sme_server_sender: SmeServer,
    mut telemetry_server_sender: Option<
        mpsc::UnboundedSender<fidl::endpoints::ServerEnd<fidl_sme::TelemetryMarker>>,
    >,
) -> Result<(), anyhow::Error> {
    loop {
        match generic_sme_request_stream.next().await {
            Some(Ok(req)) => {
                let result = match req {
                    fidl_sme::GenericSmeRequest::Query { responder } => {
                        let (info_responder, info_receiver) = crate::responder::Responder::new();
                        mlme_sink.send(crate::MlmeRequest::QueryDeviceInfo(info_responder));
                        match info_receiver.await {
                            Ok(info) => responder.send(&fidl_sme::GenericSmeQuery {
                                role: info.role,
                                sta_addr: info.sta_addr,
                                factory_addr: info.factory_addr,
                            }),
                            Err(e) => {
                                error!("Failed to query device info: {}", e);
                                Ok(())
                            }
                        }
                    }
                    fidl_sme::GenericSmeRequest::QueryIfaceCapabilities { responder } => {
                        let (apf_responder, apf_receiver) = crate::responder::Responder::new();
                        mlme_sink
                            .send(crate::MlmeRequest::QueryApfPacketFilterSupport(apf_responder));
                        match apf_receiver.await {
                            Ok(apf_support) => responder.send(apf_support.as_ref().map_err(|e| *e)),
                            Err(e) => {
                                error!("Failed to query device capabilities: {}", e);
                                responder.send(Err(zx::Status::INTERNAL.into_raw()))
                            }
                        }
                    }
                    fidl_sme::GenericSmeRequest::GetClientSme { sme_server, responder } => {
                        let response =
                            if let SmeServer::Client(server_sender) = &mut sme_server_sender {
                                server_sender
                                    .send(sme_server)
                                    .await
                                    .map_err(|_| zx::Status::PEER_CLOSED.into_raw())
                            } else {
                                Err(zx::Status::NOT_SUPPORTED.into_raw())
                            };
                        responder.send(response)
                    }
                    fidl_sme::GenericSmeRequest::GetApSme { sme_server, responder } => {
                        let response = if let SmeServer::Ap(server_sender) = &mut sme_server_sender
                        {
                            server_sender
                                .send(sme_server)
                                .await
                                .map_err(|_| zx::Status::PEER_CLOSED.into_raw())
                        } else {
                            Err(zx::Status::NOT_SUPPORTED.into_raw())
                        };
                        responder.send(response)
                    }
                    fidl_sme::GenericSmeRequest::GetSmeTelemetry {
                        telemetry_server,
                        responder,
                    } => {
                        let response = if let Some(server) = telemetry_server_sender.as_mut() {
                            server
                                .send(telemetry_server)
                                .await
                                .map_err(|_| zx::Status::PEER_CLOSED.into_raw())
                        } else {
                            warn!("Requested unsupported SME telemetry API");
                            Err(zx::Status::NOT_SUPPORTED.into_raw())
                        };
                        responder.send(response)
                    }
                };
                if let Err(e) = result {
                    error!("Failed to respond to SME handle request: {}", e);
                }
            }
            Some(Err(e)) => {
                return Err(format_err!("Generic SME request stream failed: {}", e));
            }
            None => {
                info!("Generic SME request stream terminated. Shutting down.");
                return Ok(());
            }
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::type_complexity,
    reason = "mass allow for https://fxbug.dev/381896734"
)]
pub fn create_sme(
    cfg: crate::Config,
    mlme_event_stream: MlmeEventStream,
    device_info: &fidl_mlme::DeviceInfo,
    security_support: fidl_common::SecuritySupport,
    spectrum_management_support: fidl_common::SpectrumManagementSupport,
    inspector: fuchsia_inspect::Inspector,
    generic_sme_request_stream: <fidl_sme::GenericSmeMarker as fidl::endpoints::ProtocolMarker>::RequestStream,
) -> Result<
    (MlmeStream, Pin<Box<impl Future<Output = Result<(), anyhow::Error>> + use<>>>),
    anyhow::Error,
> {
    let device_info = device_info.clone();
    let inspect_node = inspector.root().create_child("usme");
    let (server, mlme_req_sink, mlme_req_stream, telemetry_sender, sme_fut) = match device_info.role
    {
        fidl_common::WlanMacRole::Client => {
            let (telemetry_endpoint_sender, telemetry_endpoint_receiver) = mpsc::unbounded();
            let (sender, receiver) = mpsc::unbounded();
            let (mlme_req_sink, mlme_req_stream, fut) = client::serve(
                cfg,
                device_info,
                security_support,
                spectrum_management_support,
                mlme_event_stream,
                receiver,
                telemetry_endpoint_receiver,
                inspector,
                inspect_node,
            );
            (
                SmeServer::Client(sender),
                mlme_req_sink,
                mlme_req_stream,
                Some(telemetry_endpoint_sender),
                FutureObj::new(Box::new(fut)),
            )
        }
        fidl_common::WlanMacRole::Ap => {
            let (sender, receiver) = mpsc::unbounded();
            let (mlme_req_sink, mlme_req_stream, fut) =
                ap::serve(device_info, spectrum_management_support, mlme_event_stream, receiver);
            (
                SmeServer::Ap(sender),
                mlme_req_sink,
                mlme_req_stream,
                None,
                FutureObj::new(Box::new(fut)),
            )
        }
        fidl_common::WlanMacRole::Mesh => {
            return Err(format_err!("Mesh mode is unsupported"));
        }
        fidl_common::WlanMacRoleUnknown!() => {
            return Err(format_err!("Unknown WlanMacRole type: {:?}", device_info.role));
        }
    };
    let generic_sme_fut =
        serve_generic_sme(generic_sme_request_stream, mlme_req_sink, server, telemetry_sender);
    let unified_fut = async move {
        select! {
            sme_fut = sme_fut.fuse() => sme_fut,
            generic_sme_fut = generic_sme_fut.fuse() => generic_sme_fut,
        }
    };
    Ok((mlme_req_stream, Box::pin(unified_fut)))
}

// The returned future successfully terminates when MLME closes the channel
async fn serve_mlme_sme<STA, TS>(
    mut event_stream: MlmeEventStream,
    station: Arc<Mutex<STA>>,
    time_stream: TS,
) -> Result<(), anyhow::Error>
where
    STA: Station,
    TS: Stream<Item = ScheduledEvent<<STA as crate::Station>::Event>> + Unpin,
{
    let mut timeout_stream = timer::make_async_timed_event_stream(time_stream).fuse();

    loop {
        select! {
            // Fuse rationale: any `none`s in the MLME stream should result in
            // bailing immediately, so we don't need to track if we've seen a
            // `None` or not and can `fuse` directly in the `select` call.
            mlme_event = event_stream.next() => match mlme_event {
                Some(mlme_event) => station.lock().on_mlme_event(mlme_event),
                None => return Ok(()),
            },
            timeout = timeout_stream.next() => match timeout {
                Some(timed_event) => station.lock().on_timeout(timed_event),
                None => return Err(format_err!("SME timer stream has ended unexpectedly")),
            },
        }
    }
}

#[allow(clippy::extra_unused_lifetimes, reason = "mass allow for https://fxbug.dev/381896734")]
async fn serve_fidl<
    'a,
    C: Clone,
    T: fidl::endpoints::ProtocolMarker,
    Fut: futures::Future<Output = Result<(), fidl::Error>>,
>(
    context: C,
    new_fidl_clients: mpsc::UnboundedReceiver<ServerEnd<T>>,
    event_handler: impl Fn(C, fidl::endpoints::Request<T>) -> Fut + Copy,
) -> Result<Infallible, anyhow::Error> {
    let mut new_fidl_clients = new_fidl_clients.fuse();
    let mut fidl_clients = FuturesUnordered::new();
    loop {
        select! {
            new_fidl_client = new_fidl_clients.next() => match new_fidl_client {
                Some(end) => fidl_clients.push(serve_fidl_endpoint(context.clone(), end, event_handler)),
                None => return Err(format_err!("New FIDL client stream unexpectedly ended")),
            },
            () = fidl_clients.select_next_some() => {},
        }
    }
}

#[allow(
    clippy::extra_unused_lifetimes,
    clippy::needless_return,
    reason = "mass allow for https://fxbug.dev/381896734"
)]
async fn serve_fidl_endpoint<
    'a,
    C: Clone,
    T: fidl::endpoints::ProtocolMarker,
    Fut: futures::Future<Output = Result<(), fidl::Error>>,
>(
    context: C,
    endpoint: ServerEnd<T>,
    event_handler: impl Fn(C, fidl::endpoints::Request<T>) -> Fut + Copy,
) {
    let stream = endpoint.into_stream();
    const MAX_CONCURRENT_REQUESTS: usize = 1000;
    let handler = &event_handler;
    let r = stream
        .try_for_each_concurrent(MAX_CONCURRENT_REQUESTS, move |request| {
            (*handler)(context.clone(), request)
        })
        .await;
    if let Err(e) = r {
        error!("Error serving FIDL: {}", e);
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils;
    use assert_matches::assert_matches;
    use fidl::endpoints::{create_proxy, create_proxy_and_stream};
    use fuchsia_async as fasync;
    use fuchsia_inspect::Inspector;
    use futures::task::Poll;
    use std::pin::pin;
    use test_case::test_case;
    use wlan_common::test_utils::fake_features::{
        fake_security_support, fake_spectrum_management_support_empty,
    };

    #[test]
    fn create_sme_fails_startup_role_unknown() {
        let mut _exec = fasync::TestExecutor::new();
        let inspector = Inspector::default();
        let (_mlme_event_sender, mlme_event_stream) = mpsc::unbounded();
        let (_generic_sme_proxy, generic_sme_stream) =
            create_proxy_and_stream::<fidl_sme::GenericSmeMarker>();
        let device_info = fidl_mlme::DeviceInfo {
            role: fidl_common::WlanMacRole::unknown(),
            ..test_utils::fake_device_info([0; 6].into())
        };
        let result = create_sme(
            crate::Config::default(),
            mlme_event_stream,
            &device_info,
            fake_security_support(),
            fake_spectrum_management_support_empty(),
            inspector,
            generic_sme_stream,
        );

        assert!(result.is_err());
    }

    #[test]
    fn sme_shutdown_on_generic_sme_closed() {
        let mut exec = fasync::TestExecutor::new();
        let (_mlme_event_sender, mlme_event_stream) = mpsc::unbounded();
        let inspector = Inspector::default();
        let (generic_sme_proxy, generic_sme_stream) =
            create_proxy_and_stream::<fidl_sme::GenericSmeMarker>();
        let (_mlme_req_stream, serve_fut) = create_sme(
            crate::Config::default(),
            mlme_event_stream,
            &test_utils::fake_device_info([0; 6].into()),
            fake_security_support(),
            fake_spectrum_management_support_empty(),
            inspector,
            generic_sme_stream,
        )
        .unwrap();
        let mut serve_fut = pin!(serve_fut);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Also close secondary SME endpoint in the Generic SME.
        drop(generic_sme_proxy);

        // Verify SME future finished cleanly.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Ready(Ok(())));
    }

    struct GenericSmeTestHelper {
        proxy: fidl_sme::GenericSmeProxy,
        mlme_req_stream: MlmeStream,

        // These values must stay in scope or the SME will terminate, but they
        // are not relevant to Generic SME tests.
        _inspector: Inspector,
        _mlme_event_sender: mpsc::UnboundedSender<crate::MlmeEvent>,
        // Executor goes last to avoid test shutdown failures.
        exec: fasync::TestExecutor,
    }

    #[allow(clippy::type_complexity, reason = "mass allow for https://fxbug.dev/381896734")]
    fn start_generic_sme_test(
        role: fidl_common::WlanMacRole,
    ) -> Result<
        (GenericSmeTestHelper, Pin<Box<impl Future<Output = Result<(), anyhow::Error>>>>),
        anyhow::Error,
    > {
        let mut exec = fasync::TestExecutor::new();
        let inspector = Inspector::default();
        let (mlme_event_sender, mlme_event_stream) = mpsc::unbounded();
        let (generic_sme_proxy, generic_sme_stream) =
            create_proxy_and_stream::<fidl_sme::GenericSmeMarker>();
        let device_info =
            fidl_mlme::DeviceInfo { role, ..test_utils::fake_device_info([0; 6].into()) };
        let (mlme_req_stream, serve_fut) = create_sme(
            crate::Config::default(),
            mlme_event_stream,
            &device_info,
            fake_security_support(),
            fake_spectrum_management_support_empty(),
            inspector.clone(),
            generic_sme_stream,
        )?;
        let mut serve_fut = Box::pin(serve_fut);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        Ok((
            GenericSmeTestHelper {
                proxy: generic_sme_proxy,
                mlme_req_stream,
                _inspector: inspector,
                _mlme_event_sender: mlme_event_sender,
                exec,
            },
            serve_fut,
        ))
    }

    #[test]
    fn generic_sme_get_client() {
        let (mut helper, mut serve_fut) =
            start_generic_sme_test(fidl_common::WlanMacRole::Client).unwrap();

        let (client_proxy, client_server) = create_proxy();
        let mut client_sme_fut = helper.proxy.get_client_sme(client_server);
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            helper.exec.run_until_stalled(&mut client_sme_fut),
            Poll::Ready(Ok(Ok(())))
        );

        let mut status_fut = client_proxy.status();
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            helper.exec.run_until_stalled(&mut status_fut),
            Poll::Ready(Ok(fidl_sme::ClientStatusResponse::Idle(_)))
        );
    }

    #[test]
    fn generic_sme_get_ap_from_client_fails() {
        let (mut helper, mut serve_fut) =
            start_generic_sme_test(fidl_common::WlanMacRole::Client).unwrap();

        let (_ap_proxy, ap_server) = create_proxy();
        let mut client_sme_fut = helper.proxy.get_ap_sme(ap_server);
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            helper.exec.run_until_stalled(&mut client_sme_fut),
            Poll::Ready(Ok(Err(_)))
        );
    }

    #[test]
    fn generic_sme_get_ap() {
        let (mut helper, mut serve_fut) =
            start_generic_sme_test(fidl_common::WlanMacRole::Ap).unwrap();

        let (ap_proxy, ap_server) = create_proxy();
        let mut ap_sme_fut = helper.proxy.get_ap_sme(ap_server);
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(helper.exec.run_until_stalled(&mut ap_sme_fut), Poll::Ready(Ok(Ok(()))));

        let mut status_fut = ap_proxy.status();
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            helper.exec.run_until_stalled(&mut status_fut),
            Poll::Ready(Ok(fidl_sme::ApStatusResponse { .. }))
        );
    }

    #[test]
    fn generic_sme_get_client_from_ap_fails() {
        let (mut helper, mut serve_fut) =
            start_generic_sme_test(fidl_common::WlanMacRole::Ap).unwrap();

        let (_client_proxy, client_server) = create_proxy();
        let mut client_sme_fut = helper.proxy.get_client_sme(client_server);
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            helper.exec.run_until_stalled(&mut client_sme_fut),
            Poll::Ready(Ok(Err(_)))
        );
    }

    fn get_telemetry_proxy(
        helper: &mut GenericSmeTestHelper,
        serve_fut: &mut Pin<Box<impl Future<Output = Result<(), anyhow::Error>>>>,
    ) -> fidl_sme::TelemetryProxy {
        let (proxy, server) = create_proxy();
        let mut telemetry_fut = helper.proxy.get_sme_telemetry(server);
        assert_matches!(helper.exec.run_until_stalled(serve_fut), Poll::Pending);
        assert_matches!(helper.exec.run_until_stalled(&mut telemetry_fut), Poll::Ready(Ok(Ok(()))));
        proxy
    }

    #[test]
    fn generic_sme_query_telemetry_support_for_client() {
        let (mut helper, mut serve_fut) =
            start_generic_sme_test(fidl_common::WlanMacRole::Client).unwrap();
        let telemetry_proxy = get_telemetry_proxy(&mut helper, &mut serve_fut);

        // Forward request to MLME.
        let mut support_fut = telemetry_proxy.query_telemetry_support();
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Mock response from MLME. Use a fake error code to make the response easily verifiable.
        let support_req = assert_matches!(helper.exec.run_until_stalled(&mut helper.mlme_req_stream.next()), Poll::Ready(Some(req)) => req);
        let support_responder = assert_matches!(support_req, crate::MlmeRequest::QueryTelemetrySupport(responder) => responder);
        support_responder.respond(Err(1337));

        // Verify that the response made it to us without alteration.
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        let support_result = assert_matches!(helper.exec.run_until_stalled(&mut support_fut), Poll::Ready(Ok(support_result)) => support_result);
        assert_eq!(support_result, Err(1337));
    }

    #[test]
    fn generic_sme_get_histogram_stats_for_client() {
        let (mut helper, mut serve_fut) =
            start_generic_sme_test(fidl_common::WlanMacRole::Client).unwrap();
        let telemetry_proxy = get_telemetry_proxy(&mut helper, &mut serve_fut);

        // Forward request to MLME.
        let mut histogram_fut = telemetry_proxy.get_histogram_stats();
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Mock response from MLME. Use a fake error code to make the response easily verifiable.
        let histogram_req = assert_matches!(helper.exec.run_until_stalled(&mut helper.mlme_req_stream.next()), Poll::Ready(Some(req)) => req);
        let histogram_responder = assert_matches!(histogram_req, crate::MlmeRequest::GetIfaceHistogramStats(responder) => responder);
        histogram_responder.respond(fidl_mlme::GetIfaceHistogramStatsResponse::ErrorStatus(1337));

        // Verify that the response made it to us without alteration.
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        let histogram_result = assert_matches!(helper.exec.run_until_stalled(&mut histogram_fut), Poll::Ready(Ok(histogram_result)) => histogram_result);
        assert_eq!(histogram_result, Err(1337));
    }

    #[test]
    fn generic_sme_get_iface_stats_for_client() {
        let (mut helper, mut serve_fut) =
            start_generic_sme_test(fidl_common::WlanMacRole::Client).unwrap();
        let telemetry_proxy = get_telemetry_proxy(&mut helper, &mut serve_fut);

        // Forward request to MLME.
        let mut counter_fut = telemetry_proxy.get_iface_stats();
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Mock response from MLME. Use a fake error code to make the response easily verifiable.
        let counter_req = assert_matches!(helper.exec.run_until_stalled(&mut helper.mlme_req_stream.next()), Poll::Ready(Some(req)) => req);
        let counter_responder =
            assert_matches!(counter_req, crate::MlmeRequest::GetIfaceStats(responder) => responder);
        counter_responder.respond(fidl_mlme::GetIfaceStatsResponse::ErrorStatus(1337));

        // Verify that the response made it to us without alteration.
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        let counter_result = assert_matches!(helper.exec.run_until_stalled(&mut counter_fut), Poll::Ready(Ok(counter_result)) => counter_result);
        assert_eq!(counter_result, Err(1337));
    }

    #[test]
    fn generic_sme_get_signal_report_for_client() {
        let (mut helper, mut serve_fut) =
            start_generic_sme_test(fidl_common::WlanMacRole::Client).unwrap();
        let telemetry_proxy = get_telemetry_proxy(&mut helper, &mut serve_fut);

        // Forward request to MLME.
        let mut report_fut = telemetry_proxy.get_signal_report();
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Mock response from MLME. Use a fake error code to make the response easily verifiable.
        let report_req = assert_matches!(helper.exec.run_until_stalled(&mut helper.mlme_req_stream.next()), Poll::Ready(Some(req)) => req);
        let report_responder = assert_matches!(report_req, crate::MlmeRequest::GetSignalReport(responder) => responder);
        report_responder.respond(Err(1337));

        // Verify that the response made it to us without alteration.
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        let report_result = assert_matches!(helper.exec.run_until_stalled(&mut report_fut), Poll::Ready(Ok(report_result)) => report_result);
        assert_eq!(report_result, Err(1337));
    }

    #[test]
    fn generic_sme_get_telemetry_for_ap_fails() {
        let (mut helper, mut serve_fut) =
            start_generic_sme_test(fidl_common::WlanMacRole::Ap).unwrap();

        let (_telemetry_proxy, telemetry_server) = create_proxy();
        let mut telemetry_fut = helper.proxy.get_sme_telemetry(telemetry_server);
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(helper.exec.run_until_stalled(&mut telemetry_fut), Poll::Ready(Ok(Err(_))));
    }

    #[test_case(fidl_common::WlanMacRole::Client)]
    #[test_case(fidl_common::WlanMacRole::Ap)]
    fn generic_sme_query(mac_role: fidl_common::WlanMacRole) {
        let (mut helper, mut serve_fut) = start_generic_sme_test(mac_role).unwrap();

        let mut query_fut = helper.proxy.query();
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        let query_req = assert_matches!(helper.exec.run_until_stalled(&mut helper.mlme_req_stream.next()), Poll::Ready(Some(req)) => req);
        let query_responder =
            assert_matches!(query_req, crate::MlmeRequest::QueryDeviceInfo(responder) => responder);
        query_responder.respond(fidl_mlme::DeviceInfo {
            role: mac_role,
            sta_addr: [2; 6],
            factory_addr: [2; 6],
            bands: vec![],
            softmac_hardware_capability: 0,
            qos_capable: false,
        });

        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        let query_result = assert_matches!(helper.exec.run_until_stalled(&mut query_fut), Poll::Ready(Ok(result)) => result);
        assert_eq!(query_result.role, mac_role);
        assert_eq!(query_result.sta_addr, [2; 6]);
    }

    #[test]
    fn generic_sme_query_iface_capabilities() {
        let (mut helper, mut serve_fut) =
            start_generic_sme_test(fidl_common::WlanMacRole::Client).unwrap();

        let mut query_fut = helper.proxy.query_iface_capabilities();
        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        let query_req = assert_matches!(helper.exec.run_until_stalled(&mut helper.mlme_req_stream.next()), Poll::Ready(Some(req)) => req);
        let query_responder = assert_matches!(query_req, crate::MlmeRequest::QueryApfPacketFilterSupport(responder) => responder);
        let apf_support = fidl_common::ApfPacketFilterSupport {
            supported: Some(true),
            version: Some(1),
            max_filter_length: Some(1024),
            ..Default::default()
        };
        query_responder.respond(Ok(apf_support.clone()));

        assert_matches!(helper.exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        let query_result = assert_matches!(helper.exec.run_until_stalled(&mut query_fut), Poll::Ready(Ok(result)) => result);
        assert_eq!(query_result, Ok(apf_support));
    }
}
