// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mode_management::iface_manager_api::IfaceManagerApi;
use crate::util::listener;
use anyhow::{Error, format_err};
use fidl::epitaph::ChannelEpitaphExt;
use fidl_fuchsia_wlan_policy as fidl_policy;
use futures::channel::mpsc;
use futures::future::LocalBoxFuture;
use futures::lock::{Mutex, MutexGuard};
use futures::sink::SinkExt;
use futures::stream::{FuturesUnordered, StreamExt, TryStreamExt};
use futures::{FutureExt, TryFutureExt, select};
use log::{error, info, warn};
use std::sync::Arc;
use wlan_common::RadioConfig;
use wlan_common::channel::Bandwidth;

pub mod state_machine;
pub mod types;

// Wrapper around an AP interface allowing a watcher to set the underlying SME and the policy API
// servicing routines to utilize the SME.
#[derive(Clone)]
pub struct AccessPoint {
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
    update_sender: listener::ApListenerMessageSender,
    ap_provider_lock: Arc<Mutex<()>>,
}

// This number was chosen arbitrarily.
const MAX_CONCURRENT_LISTENERS: usize = 1000;

type ApRequests = fidl::endpoints::ServerEnd<fidl_policy::AccessPointControllerMarker>;

impl AccessPoint {
    /// Creates a new, empty AccessPoint. The returned AccessPoint effectively represents the state
    /// in which no AP interface is available.
    pub fn new(
        iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
        update_sender: listener::ApListenerMessageSender,
        ap_provider_lock: Arc<Mutex<()>>,
    ) -> Self {
        Self { iface_manager, update_sender, ap_provider_lock }
    }

    fn send_listener_message(&self, message: listener::ApMessage) -> Result<(), Error> {
        self.update_sender
            .clone()
            .unbounded_send(message)
            .map_err(|e| format_err!("failed to send state update: {}", e))
    }

    /// Serves the AccessPointProvider protocol.  Only one caller is allowed to interact with an
    /// AccessPointController.  This routine ensures that one active user has access at a time.
    /// Additional requests are terminated immediately.
    pub async fn serve_provider_requests(
        self,
        mut requests: fidl_policy::AccessPointProviderRequestStream,
    ) {
        let mut pending_response_queue =
            FuturesUnordered::<LocalBoxFuture<'static, Result<Response, Error>>>::new();
        let (internal_messages_sink, mut internal_messages_stream) = mpsc::channel(0);
        let mut provider_reqs = FuturesUnordered::new();

        loop {
            select! {
                // Progress controller requests.
                _ = provider_reqs.select_next_some() => (),
                // Process provider requests.
                req = requests.select_next_some() => match req {
                    Ok(req) => {
                        // Reject the new provider request if another client is already using the
                        // AccessPointProvider service.
                        if let Some(ap_provider_guard) = self.ap_provider_lock.try_lock() {
                            let mut ap = self.clone();
                            let sink_copy = internal_messages_sink.clone();
                            let fut = async move {
                                ap.handle_provider_request(
                                    sink_copy,
                                    ap_provider_guard,
                                    req
                                ).await
                            };
                            provider_reqs.push(fut);
                        } else if let Err(e) = reject_provider_request(req) {
                            error!("error sending rejection epitaph: {:?}", e);
                        }
                    }
                    Err(e) => error!("encountered and error while serving provider requests: {}", e)
                },
                complete => break,
                msg = internal_messages_stream.select_next_some() => pending_response_queue.push(msg),
                resp = pending_response_queue.select_next_some() => match resp {
                    Ok(Response::StartResponse(result)) => {
                        match result.result {
                            Ok(()) => {}
                            Err(_) => {
                                let ssid_as_str = std::str::from_utf8(&result.config.id.ssid).unwrap_or("");
                                error!("AP {} did not start", ssid_as_str);
                            }
                        };
                    },
                    Err(e) => error!("error while processing AP requests: {}", e)
                }
            }
        }
    }

    /// Handles any incoming requests for the AccessPointProvider protocol.
    async fn handle_provider_request(
        &mut self,
        internal_msg_sink: mpsc::Sender<LocalBoxFuture<'static, Result<Response, Error>>>,
        ap_provider_guard: MutexGuard<'_, ()>,
        req: fidl_policy::AccessPointProviderRequest,
    ) -> Result<(), fidl::Error> {
        match req {
            fidl_policy::AccessPointProviderRequest::GetController {
                requests, updates, ..
            } => {
                self.register_listener(updates.into_proxy());
                self.handle_ap_requests(internal_msg_sink, ap_provider_guard, requests).await?;
                Ok(())
            }
        }
    }

    /// Serves the AccessPointListener protocol.
    pub async fn serve_listener_requests(
        self,
        requests: fidl_policy::AccessPointListenerRequestStream,
    ) {
        let serve_fut = requests
            .try_for_each_concurrent(MAX_CONCURRENT_LISTENERS, |req| {
                self.handle_listener_request(req)
            })
            .unwrap_or_else(|e| error!("error serving Client Listener API: {}", e));
        serve_fut.await;
    }

    /// Handles all requests of the AccessPointController.
    async fn handle_ap_requests(
        &self,
        mut internal_msg_sink: mpsc::Sender<LocalBoxFuture<'static, Result<Response, Error>>>,
        ap_provider_guard: MutexGuard<'_, ()>,
        requests: ApRequests,
    ) -> Result<(), fidl::Error> {
        let mut request_stream = requests.into_stream();
        while let Some(request) = request_stream.try_next().await? {
            log_ap_request(&request);
            match request {
                fidl_policy::AccessPointControllerRequest::StartAccessPoint {
                    config,
                    mode,
                    band,
                    responder,
                } => {
                    let ap_config = match derive_ap_config(&config, mode, band) {
                        Ok(config) => config,
                        Err(e) => {
                            info!("StartAccessPoint could not derive AP config: {}", e);
                            responder.send(fidl_policy::RequestStatus::RejectedNotSupported)?;
                            continue;
                        }
                    };

                    let mut iface_manager = self.iface_manager.lock().await;
                    let receiver = match iface_manager.start_ap(ap_config.clone()).await {
                        Ok(receiver) => receiver,
                        Err(e) => {
                            info!("failed to start AP: {}", e);
                            responder.send(fidl_policy::RequestStatus::RejectedIncompatibleMode)?;
                            continue;
                        }
                    };

                    let fut = async move {
                        receiver.await?;
                        Ok(Response::StartResponse(StartParameters {
                            config: ap_config,
                            result: Ok(()),
                        }))
                    };
                    if let Err(e) = internal_msg_sink.send(fut.boxed()).await {
                        error!("Failed to send internal message: {:?}", e)
                    }
                    responder.send(fidl_policy::RequestStatus::Acknowledged)?;
                }
                fidl_policy::AccessPointControllerRequest::StopAccessPoint {
                    config,
                    responder,
                } => {
                    let ssid = match config.id {
                        Some(id) => types::Ssid::from_bytes_unchecked(id.ssid),
                        None => {
                            warn!("received disconnect request with no SSID specified");
                            responder.send(fidl_policy::RequestStatus::RejectedNotSupported)?;
                            continue;
                        }
                    };
                    let credential = match config.credential {
                        Some(fidl_policy::Credential::Password(password)) => password,
                        Some(fidl_policy::Credential::Psk(psk)) => psk,
                        Some(fidl_policy::Credential::None(fidl_policy::Empty)) => vec![],
                        // The compiler insists that the unknown credential variant be handled.
                        Some(_) => vec![],
                        None => {
                            warn!("received disconnect request with no credential specified");
                            responder.send(fidl_policy::RequestStatus::RejectedNotSupported)?;
                            continue;
                        }
                    };

                    let mut iface_manager = self.iface_manager.lock().await;
                    match iface_manager.stop_ap(ssid, credential).await {
                        Ok(()) => {
                            responder.send(fidl_policy::RequestStatus::Acknowledged)?;
                        }
                        Err(e) => {
                            error!("failed to stop AP: {}", e);
                            responder.send(fidl_policy::RequestStatus::RejectedIncompatibleMode)?;
                        }
                    }
                }
                fidl_policy::AccessPointControllerRequest::StopAllAccessPoints { .. } => {
                    let mut iface_manager = self.iface_manager.lock().await;
                    match iface_manager.stop_all_aps().await {
                        Ok(()) => {}
                        Err(e) => {
                            info!("could not cleanly stop all APs: {}", e);
                        }
                    }
                }
            }
        }
        drop(ap_provider_guard);
        Ok(())
    }

    /// Registers a new update listener.
    /// The client's current state will be send to the newly added listener immediately.
    fn register_listener(&self, listener: fidl_policy::AccessPointStateUpdatesProxy) {
        if let Err(e) = self.send_listener_message(listener::Message::NewListener(listener)) {
            error!("failed to register new listener: {}", e);
        }
    }

    /// Handle inbound requests to register an additional AccessPointStateUpdates listener.
    async fn handle_listener_request(
        &self,
        req: fidl_policy::AccessPointListenerRequest,
    ) -> Result<(), fidl::Error> {
        match req {
            fidl_policy::AccessPointListenerRequest::GetListener { updates, .. } => {
                self.register_listener(updates.into_proxy());
                Ok(())
            }
        }
    }
}

fn reject_provider_request(
    req: fidl_policy::AccessPointProviderRequest,
) -> Result<(), fidl::Error> {
    match req {
        fidl_policy::AccessPointProviderRequest::GetController { requests, updates, .. } => {
            info!("Rejecting new access point controller request because a controller is in use");
            requests.into_channel().close_with_epitaph(zx::Status::ALREADY_BOUND)?;
            updates.into_channel().close_with_epitaph(zx::Status::ALREADY_BOUND)?;
            Ok(())
        }
    }
}

// The NetworkConfigs will be used in the future to aggregate state in crate::util::listener.
struct StartParameters {
    config: state_machine::ApConfig,
    result: Result<(), Error>,
}

enum Response {
    StartResponse(StartParameters),
}

fn derive_ap_config(
    config: &fidl_policy::NetworkConfig,
    mode: fidl_policy::ConnectivityMode,
    band: fidl_policy::OperatingBand,
) -> Result<state_machine::ApConfig, Error> {
    let network_id = match config.id.as_ref() {
        Some(id) => id.clone(),
        None => return Err(format_err!("invalid NetworkIdentifier")),
    };
    let credential = match config.credential.as_ref() {
        Some(credential) => match credential {
            fidl_policy::Credential::None(fidl_policy::Empty) => b"".to_vec(),
            fidl_policy::Credential::Password(bytes) => bytes.to_vec(),
            fidl_policy::Credential::Psk(bytes) => bytes.to_vec(),
            credential => {
                return Err(format_err!("Unrecognized credential: {:?}", credential));
            }
        },
        None => b"".to_vec(),
    };

    // TODO(https://fxbug.dev/42131511): Improve the channel selection algorithm.
    let (channel, wlan_band) = match band {
        fidl_policy::OperatingBand::Any => (11, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz),
        fidl_policy::OperatingBand::Only24Ghz => {
            (11, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz)
        }
        fidl_policy::OperatingBand::Only5Ghz => {
            (36, fidl_fuchsia_wlan_ieee80211::WlanBand::FiveGhz)
        }
    };

    let radio_config = RadioConfig::new(
        fidl_fuchsia_wlan_ieee80211::WlanPhyType::Ht,
        Bandwidth::Cbw20,
        channel,
        wlan_band,
    );

    Ok(state_machine::ApConfig {
        id: network_id.into(),
        credential,
        radio_config,
        mode: types::ConnectivityMode::from(mode),
        band: types::OperatingBand::from(band),
    })
}

/// Logs incoming AccessPointController requests.
fn log_ap_request(request: &fidl_policy::AccessPointControllerRequest) {
    info!(
        "Received policy AP request {}",
        match request {
            fidl_policy::AccessPointControllerRequest::StartAccessPoint { .. } => {
                "StartAccessPoint"
            }
            fidl_policy::AccessPointControllerRequest::StopAccessPoint { .. } => {
                "StopAccessPoint"
            }
            fidl_policy::AccessPointControllerRequest::StopAllAccessPoints { .. } => {
                "StopAllAccessPoints"
            }
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::types as client_types;
    use crate::mode_management::iface_manager_api::{ConnectAttemptRequest, SmeForScan};
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use fidl::endpoints::{Proxy, create_proxy, create_request_stream};
    use fuchsia_async as fasync;
    use futures::channel::oneshot;
    use futures::task::Poll;
    use std::pin::pin;
    use std::unimplemented;

    #[derive(Debug)]
    struct FakeIfaceManager {
        pub start_response_succeeds: bool,
        pub start_succeeds: bool,
        pub stop_succeeds: bool,
    }

    impl FakeIfaceManager {
        pub fn new() -> Self {
            FakeIfaceManager {
                start_response_succeeds: true,
                start_succeeds: true,
                stop_succeeds: true,
            }
        }
    }

    #[async_trait(?Send)]
    impl IfaceManagerApi for FakeIfaceManager {
        async fn disconnect(
            &mut self,
            _network_id: types::NetworkIdentifier,
            _reason: client_types::DisconnectReason,
        ) -> Result<(), Error> {
            unimplemented!()
        }

        async fn connect(&mut self, _connect_req: ConnectAttemptRequest) -> Result<(), Error> {
            unimplemented!()
        }

        async fn record_idle_client(&mut self, _iface_id: u16) -> Result<(), Error> {
            unimplemented!()
        }

        async fn has_idle_client(&mut self) -> Result<bool, Error> {
            unimplemented!()
        }

        async fn handle_added_iface(&mut self, _iface_id: u16) -> Result<(), Error> {
            unimplemented!()
        }

        async fn handle_removed_iface(&mut self, _iface_id: u16) -> Result<(), Error> {
            unimplemented!()
        }

        async fn get_sme_proxy_for_scan(&mut self) -> Result<SmeForScan, Error> {
            unimplemented!()
        }

        async fn stop_client_connections(
            &mut self,
            _reason: client_types::DisconnectReason,
        ) -> Result<(), Error> {
            unimplemented!()
        }

        async fn start_client_connections(&mut self) -> Result<(), Error> {
            unimplemented!()
        }

        async fn start_ap(
            &mut self,
            _config: state_machine::ApConfig,
        ) -> Result<oneshot::Receiver<()>, Error> {
            if self.start_succeeds {
                let (sender, receiver) = oneshot::channel();

                if self.start_response_succeeds {
                    let _ = sender.send(());
                }

                Ok(receiver)
            } else {
                Err(format_err!("start_ap was configured to fail"))
            }
        }

        async fn stop_ap(&mut self, _ssid: types::Ssid, _password: Vec<u8>) -> Result<(), Error> {
            if self.stop_succeeds {
                Ok(())
            } else {
                Err(format_err!("stop was instructed to fail"))
            }
        }

        async fn stop_all_aps(&mut self) -> Result<(), Error> {
            if self.stop_succeeds {
                Ok(())
            } else {
                Err(format_err!("stop was instructed to fail"))
            }
        }

        async fn set_country(
            &mut self,
            _country_code: Option<client_types::CountryCode>,
        ) -> Result<(), Error> {
            unimplemented!()
        }
    }

    /// Requests a new AccessPointController from the given AccessPointProvider.
    fn request_controller(
        provider: &fidl_policy::AccessPointProviderProxy,
    ) -> (fidl_policy::AccessPointControllerProxy, fidl_policy::AccessPointStateUpdatesRequestStream)
    {
        let (controller, requests) = create_proxy::<fidl_policy::AccessPointControllerMarker>();
        let (update_sink, update_stream) =
            create_request_stream::<fidl_policy::AccessPointStateUpdatesMarker>();
        provider.get_controller(requests, update_sink).expect("error getting controller");
        (controller, update_stream)
    }

    struct TestValues {
        provider: fidl_policy::AccessPointProviderProxy,
        requests: fidl_policy::AccessPointProviderRequestStream,
        ap: AccessPoint,
        iface_manager: Arc<Mutex<FakeIfaceManager>>,
    }

    /// Setup channels and proxies needed for the tests to use use the AP Provider and
    /// AP Controller APIs in tests.
    fn test_setup() -> TestValues {
        let (provider, requests) = create_proxy::<fidl_policy::AccessPointProviderMarker>();
        let requests = requests.into_stream();

        let iface_manager = FakeIfaceManager::new();
        let iface_manager = Arc::new(Mutex::new(iface_manager));
        let (sender, _) = mpsc::unbounded();
        let ap = AccessPoint::new(iface_manager.clone(), sender, Arc::new(Mutex::new(())));
        TestValues { provider, requests, ap, iface_manager }
    }

    /// Tests the case where StartAccessPoint is called and there is a valid interface to service
    /// the request and the request succeeds.
    #[fuchsia::test]
    fn test_start_access_point_with_iface_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let serve_fut = test_values.ap.serve_provider_requests(test_values.requests);
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue StartAP request.
        let network_id = fidl_policy::NetworkIdentifier {
            ssid: b"test".to_vec(),
            type_: fidl_policy::SecurityType::None,
        };
        let network_config = fidl_policy::NetworkConfig {
            id: Some(network_id),
            credential: None,
            ..Default::default()
        };
        let connectivity_mode = fidl_policy::ConnectivityMode::LocalOnly;
        let operating_band = fidl_policy::OperatingBand::Any;
        let start_fut =
            controller.start_access_point(&network_config, connectivity_mode, operating_band);
        let mut start_fut = pin!(start_fut);

        // Process start request and verify start response.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut start_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::Acknowledged))
        );
    }

    /// Tests the case where StartAccessPoint is called and there is a valid interface to service
    /// the request, but the request fails.
    #[fuchsia::test]
    fn test_start_access_point_with_iface_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let serve_fut = test_values.ap.serve_provider_requests(test_values.requests);
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Set the StartAp response.
        {
            let iface_manager_fut = test_values.iface_manager.lock();
            let mut iface_manager_fut = pin!(iface_manager_fut);
            let mut iface_manager = assert_matches!(
                exec.run_until_stalled(&mut iface_manager_fut),
                Poll::Ready(iface_manager) => { iface_manager }
            );
            iface_manager.start_response_succeeds = false;
        }

        // Issue StartAP request.
        let network_id = fidl_policy::NetworkIdentifier {
            ssid: b"test".to_vec(),
            type_: fidl_policy::SecurityType::None,
        };
        let network_config = fidl_policy::NetworkConfig {
            id: Some(network_id),
            credential: None,
            ..Default::default()
        };
        let connectivity_mode = fidl_policy::ConnectivityMode::LocalOnly;
        let operating_band = fidl_policy::OperatingBand::Any;
        let start_fut =
            controller.start_access_point(&network_config, connectivity_mode, operating_band);
        let mut start_fut = pin!(start_fut);

        // Verify the start response is successful despite the AP's failure to start.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut start_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::Acknowledged))
        );
    }

    /// Tests the case where there are no interfaces available to handle a StartAccessPoint
    /// request.
    #[fuchsia::test]
    fn test_start_access_point_no_iface() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();

        // Set the IfaceManager to fail when asked to start an AP.
        {
            let iface_manager_fut = test_values.iface_manager.lock();
            let mut iface_manager_fut = pin!(iface_manager_fut);
            let mut iface_manager = assert_matches!(
                exec.run_until_stalled(&mut iface_manager_fut),
                Poll::Ready(iface_manager) => { iface_manager }
            );
            iface_manager.start_succeeds = false;
        }

        let serve_fut = test_values.ap.serve_provider_requests(test_values.requests);
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue StartAP request.
        let connectivity_mode = fidl_policy::ConnectivityMode::LocalOnly;
        let operating_band = fidl_policy::OperatingBand::Any;
        let network_config =
            fidl_policy::NetworkConfig { id: None, credential: None, ..Default::default() };
        let start_fut =
            controller.start_access_point(&network_config, connectivity_mode, operating_band);
        let mut start_fut = pin!(start_fut);

        // Process start request and verify start response.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut start_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::RejectedNotSupported))
        );
    }

    /// Tests the case where StopAccessPoint is called and there is a valid interface to handle the
    /// request and the request succeeds.
    #[fuchsia::test]
    fn test_stop_access_point_with_iface_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let serve_fut = test_values.ap.serve_provider_requests(test_values.requests);
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue StopAP request.
        let network_id = fidl_policy::NetworkIdentifier {
            ssid: b"test".to_vec(),
            type_: fidl_policy::SecurityType::None,
        };
        let credential = fidl_policy::Credential::None(fidl_policy::Empty);
        let network_config = fidl_policy::NetworkConfig {
            id: Some(network_id),
            credential: Some(credential),
            ..Default::default()
        };
        let stop_fut = controller.stop_access_point(&network_config);
        let mut stop_fut = pin!(stop_fut);

        // Process stop request and verify stop response.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut stop_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::Acknowledged))
        );
    }

    /// Tests the case where StopAccessPoint is called and there is a valid interface to service
    /// the request, but the request fails.
    #[fuchsia::test]
    fn test_stop_access_point_with_iface_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let serve_fut = test_values.ap.serve_provider_requests(test_values.requests);
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Set the StopAp response.
        {
            let iface_manager_fut = test_values.iface_manager.lock();
            let mut iface_manager_fut = pin!(iface_manager_fut);
            let mut iface_manager = assert_matches!(
                exec.run_until_stalled(&mut iface_manager_fut),
                Poll::Ready(iface_manager) => { iface_manager }
            );
            iface_manager.stop_succeeds = false;
        }

        // Issue StopAP request.
        let network_id = fidl_policy::NetworkIdentifier {
            ssid: b"test".to_vec(),
            type_: fidl_policy::SecurityType::None,
        };
        let credential = fidl_policy::Credential::None(fidl_policy::Empty);
        let network_config = fidl_policy::NetworkConfig {
            id: Some(network_id),
            credential: Some(credential),
            ..Default::default()
        };
        let stop_fut = controller.stop_access_point(&network_config);
        let mut stop_fut = pin!(stop_fut);

        // Process stop request and verify stop response.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut stop_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::RejectedIncompatibleMode))
        );
    }

    /// Tests the case where StopAccessPoints is called, there is a valid interface to handle the
    /// request, and the request succeeds.
    #[fuchsia::test]
    fn test_stop_all_access_points_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let serve_fut = test_values.ap.serve_provider_requests(test_values.requests);
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue StopAllAps request.
        let stop_result = controller.stop_all_access_points();
        assert!(stop_result.is_ok());

        // Verify that the service is still running.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
    }

    /// Tests the case where StopAccessPoints is called and there is a valid interface to handle
    /// the request, but the request fails.
    #[fuchsia::test]
    fn test_stop_all_access_points_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let serve_fut = test_values.ap.serve_provider_requests(test_values.requests);
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Set the StopAp response.
        {
            let iface_manager_fut = test_values.iface_manager.lock();
            let mut iface_manager_fut = pin!(iface_manager_fut);
            let mut iface_manager = assert_matches!(
                exec.run_until_stalled(&mut iface_manager_fut),
                Poll::Ready(iface_manager) => { iface_manager }
            );
            iface_manager.stop_succeeds = false;
        }

        // Request a new controller.
        let (controller, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue StopAllAps request.
        let stop_result = controller.stop_all_access_points();
        assert!(stop_result.is_ok());

        // Verify that the service is still running despite the call to stop failing.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
    }

    #[fuchsia::test]
    fn test_multiple_controllers() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let serve_fut = test_values.ap.serve_provider_requests(test_values.requests);
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a controller.
        let (_controller1, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request another controller.
        let (controller2, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Verify Epitaph was received.
        let mut controller2_event_stream = controller2.take_event_stream();
        let controller2_event_fut = controller2_event_stream.next();
        let mut controller2_event_fut = pin!(controller2_event_fut);
        assert_matches!(
            exec.run_until_stalled(&mut controller2_event_fut),
            Poll::Ready(Some(Err(fidl::Error::ClientChannelClosed { epitaph, .. })))
                if epitaph == zx::Status::ALREADY_BOUND
        );

        assert!(controller2.is_closed());
    }

    #[fuchsia::test]
    fn test_multiple_api_clients() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let serve_fut = test_values.ap.clone().serve_provider_requests(test_values.requests);
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a controller.
        let (controller1, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Create another request stream and begin serving it.  This is equivalent to the behavior
        // that occurs when a second client connects to the AccessPointProvider service.
        let (provider, requests) = create_proxy::<fidl_policy::AccessPointProviderMarker>();
        let requests = requests.into_stream();
        let second_serve_fut = test_values.ap.serve_provider_requests(requests);
        let mut second_serve_fut = pin!(second_serve_fut);

        let (controller2, _) = request_controller(&provider);
        assert_matches!(exec.run_until_stalled(&mut second_serve_fut), Poll::Pending);

        // Verify Epitaph was received.
        let mut controller2_event_stream = controller2.take_event_stream();
        let controller2_event_fut = controller2_event_stream.next();
        let mut controller2_event_fut = pin!(controller2_event_fut);
        assert_matches!(
            exec.run_until_stalled(&mut controller2_event_fut),
            Poll::Ready(Some(Err(fidl::Error::ClientChannelClosed { epitaph, .. })))
                if epitaph == zx::Status::ALREADY_BOUND
        );

        assert!(controller2.is_closed());

        // Drop the initial client controller and make sure the second service instance can get a
        // client controller and make a request.
        drop(controller1);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        let (controller2, _) = request_controller(&provider);
        assert_matches!(exec.run_until_stalled(&mut second_serve_fut), Poll::Pending);

        // Issue StartAP request to make sure the new controller works.
        let network_id = fidl_policy::NetworkIdentifier {
            ssid: b"test".to_vec(),
            type_: fidl_policy::SecurityType::None,
        };
        let network_config = fidl_policy::NetworkConfig {
            id: Some(network_id),
            credential: None,
            ..Default::default()
        };
        let connectivity_mode = fidl_policy::ConnectivityMode::LocalOnly;
        let operating_band = fidl_policy::OperatingBand::Any;
        let start_fut =
            controller2.start_access_point(&network_config, connectivity_mode, operating_band);
        let mut start_fut = pin!(start_fut);

        // Process start request and verify start response.
        assert_matches!(exec.run_until_stalled(&mut second_serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut start_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::Acknowledged))
        );
    }
}
