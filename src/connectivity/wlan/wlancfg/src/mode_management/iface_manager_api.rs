// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::access_point::{state_machine as ap_fsm, types as ap_types};
use crate::client::types as client_types;
use crate::config_management::network_config::Credential;
use crate::mode_management::iface_manager_types::*;
use crate::mode_management::{Defect, IfaceFailure};
use anyhow::{Error, bail, format_err};
use async_trait::async_trait;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fuchsia_async::TimeoutExt;
use futures::channel::{mpsc, oneshot};
use futures::{TryFutureExt, TryStreamExt};
use log::{info, warn};
use wlan_telemetry::TimeoutSource;

// A long amount of time that a scan should be able to finish within. If a scan takes longer than
// this is indicates something is wrong.
const SCAN_TIMEOUT: fuchsia_async::MonotonicDuration =
    fuchsia_async::MonotonicDuration::from_seconds(60);
const CONNECT_TIMEOUT: fuchsia_async::MonotonicDuration =
    fuchsia_async::MonotonicDuration::from_seconds(30);
const DISCONNECT_TIMEOUT: fuchsia_async::MonotonicDuration =
    fuchsia_async::MonotonicDuration::from_seconds(10);
const START_AP_TIMEOUT: fuchsia_async::MonotonicDuration =
    fuchsia_async::MonotonicDuration::from_seconds(30);
const STOP_AP_TIMEOUT: fuchsia_async::MonotonicDuration =
    fuchsia_async::MonotonicDuration::from_seconds(10);
const AP_STATUS_TIMEOUT: fuchsia_async::MonotonicDuration =
    fuchsia_async::MonotonicDuration::from_seconds(10);

#[async_trait(?Send)]
pub trait IfaceManagerApi {
    /// Finds the client iface with the given network configuration, disconnects from the network,
    /// and removes the client's network configuration information.
    async fn disconnect(
        &mut self,
        network_id: ap_types::NetworkIdentifier,
        reason: client_types::DisconnectReason,
    ) -> Result<(), Error>;

    /// Selects a client iface, ensures that a ClientSmeProxy and client connectivity state machine
    /// exists for the iface, and then issues a connect request to the client connectivity state
    /// machine.
    async fn connect(&mut self, connect_req: ConnectAttemptRequest) -> Result<(), Error>;

    /// Marks an existing client interface as unconfigured.
    async fn record_idle_client(&mut self, iface_id: u16) -> Result<(), Error>;

    /// Returns an indication of whether or not any client interfaces are unconfigured.
    async fn has_idle_client(&mut self) -> Result<bool, Error>;

    /// Queries the properties of the provided interface ID and internally accounts for the newly
    /// added client or AP.
    async fn handle_added_iface(&mut self, iface_id: u16) -> Result<(), Error>;

    /// Removes all internal references of the provided interface ID.
    async fn handle_removed_iface(&mut self, iface_id: u16) -> Result<(), Error>;

    /// Selects a client iface and return it for use with a scan
    async fn get_sme_proxy_for_scan(&mut self) -> Result<SmeForScan, Error>;

    /// Disconnects all configured clients and disposes of all client ifaces before instructing
    /// the PhyManager to stop client connections.
    async fn stop_client_connections(
        &mut self,
        reason: client_types::DisconnectReason,
    ) -> Result<(), Error>;

    /// Passes the call to start client connections through to the PhyManager.
    async fn start_client_connections(&mut self) -> Result<(), Error>;

    /// Starts an AP interface with the provided configuration.
    async fn start_ap(&mut self, config: ap_fsm::ApConfig) -> Result<oneshot::Receiver<()>, Error>;

    /// Stops the AP interface corresponding to the provided configuration and destroys it.
    async fn stop_ap(&mut self, ssid: Ssid, password: Vec<u8>) -> Result<(), Error>;

    /// Stops all AP interfaces and destroys them.
    async fn stop_all_aps(&mut self) -> Result<(), Error>;

    /// Sets the country code for WLAN PHYs.
    async fn set_country(
        &mut self,
        country_code: Option<client_types::CountryCode>,
    ) -> Result<(), Error>;
}

#[derive(Clone)]
pub struct IfaceManager {
    pub sender: mpsc::Sender<IfaceManagerRequest>,
}

#[async_trait(?Send)]
impl IfaceManagerApi for IfaceManager {
    async fn disconnect(
        &mut self,
        network_id: ap_types::NetworkIdentifier,
        reason: client_types::DisconnectReason,
    ) -> Result<(), Error> {
        let (responder, receiver) = oneshot::channel();
        let req = DisconnectRequest { network_id, responder, reason };
        self.sender.try_send(IfaceManagerRequest::Disconnect(req))?;

        receiver.await?
    }

    async fn connect(&mut self, connect_req: ConnectAttemptRequest) -> Result<(), Error> {
        let (responder, receiver) = oneshot::channel();
        let req = ConnectRequest { request: connect_req, responder };
        self.sender.try_send(IfaceManagerRequest::Connect(req))?;

        receiver.await?
    }

    async fn record_idle_client(&mut self, iface_id: u16) -> Result<(), Error> {
        let (responder, receiver) = oneshot::channel();
        let req = RecordIdleIfaceRequest { iface_id, responder };
        self.sender.try_send(IfaceManagerRequest::RecordIdleIface(req))?;
        receiver.await?;
        Ok(())
    }

    async fn has_idle_client(&mut self) -> Result<bool, Error> {
        let (responder, receiver) = oneshot::channel();
        let req = HasIdleIfaceRequest { responder };
        self.sender.try_send(IfaceManagerRequest::HasIdleIface(req))?;
        receiver.await.map_err(|e| e.into())
    }

    async fn handle_added_iface(&mut self, iface_id: u16) -> Result<(), Error> {
        let (responder, receiver) = oneshot::channel();
        let req = AddIfaceRequest { iface_id, responder };
        self.sender.try_send(IfaceManagerRequest::AddIface(req))?;
        receiver.await?;
        Ok(())
    }

    async fn handle_removed_iface(&mut self, iface_id: u16) -> Result<(), Error> {
        let (responder, receiver) = oneshot::channel();
        let req = RemoveIfaceRequest { iface_id, responder };
        self.sender.try_send(IfaceManagerRequest::RemoveIface(req))?;
        receiver.await?;
        Ok(())
    }

    async fn get_sme_proxy_for_scan(&mut self) -> Result<SmeForScan, Error> {
        let (responder, receiver) = oneshot::channel();
        let req = ScanProxyRequest { responder };
        self.sender.try_send(IfaceManagerRequest::GetScanProxy(req))?;
        receiver.await?
    }

    async fn start_client_connections(&mut self) -> Result<(), Error> {
        let (responder, receiver) = oneshot::channel();
        let req = StartClientConnectionsRequest { responder };
        self.sender.try_send(IfaceManagerRequest::StartClientConnections(req))?;
        receiver.await?
    }

    async fn start_ap(&mut self, config: ap_fsm::ApConfig) -> Result<oneshot::Receiver<()>, Error> {
        let (responder, receiver) = oneshot::channel();
        let req = StartApRequest { config, responder };
        self.sender.try_send(IfaceManagerRequest::StartAp(req))?;
        receiver.await?
    }

    async fn stop_client_connections(
        &mut self,
        reason: client_types::DisconnectReason,
    ) -> Result<(), Error> {
        let (responder, receiver) = oneshot::channel();
        let req = StopClientConnectionsRequest { responder, reason };
        self.sender.try_send(IfaceManagerRequest::StopClientConnections(req))?;
        receiver.await?
    }

    async fn stop_ap(&mut self, ssid: Ssid, password: Vec<u8>) -> Result<(), Error> {
        let (responder, receiver) = oneshot::channel();
        let req = StopApRequest { ssid, password, responder };
        self.sender.try_send(IfaceManagerRequest::StopAp(req))?;
        receiver.await?
    }

    async fn stop_all_aps(&mut self) -> Result<(), Error> {
        let (responder, receiver) = oneshot::channel();
        let req = StopAllApsRequest { responder };
        self.sender.try_send(IfaceManagerRequest::StopAllAps(req))?;
        receiver.await?
    }

    async fn set_country(
        &mut self,
        country_code: Option<client_types::CountryCode>,
    ) -> Result<(), Error> {
        let (responder, receiver) = oneshot::channel();
        let req = SetCountryRequest { country_code, responder };
        self.sender.try_send(IfaceManagerRequest::SetCountry(req))?;
        receiver.await?
    }
}

trait DefectReporter {
    fn defect_sender(&self) -> mpsc::Sender<Defect>;

    fn report_defect(&self, defect: Defect) {
        let mut defect_sender = self.defect_sender();
        if let Err(e) = defect_sender.try_send(defect) {
            warn!("Failed to report defect {:?}: {:?}", defect, e)
        }
    }
}

#[derive(Debug)]
pub struct SmeForScan {
    proxy: fidl_sme::ClientSmeProxy,
    iface_id: u16,
    defect_sender: mpsc::Sender<Defect>,
}

impl SmeForScan {
    pub fn new(
        proxy: fidl_sme::ClientSmeProxy,
        iface_id: u16,
        defect_sender: mpsc::Sender<Defect>,
    ) -> Self {
        SmeForScan { proxy, iface_id, defect_sender }
    }

    pub async fn scan(
        &self,
        req: &fidl_sme::ScanRequest,
    ) -> Result<fidl_sme::ClientSmeScanResult, Error> {
        self.proxy
            .scan(req)
            .map_err(|e| format_err!("{:?}", e))
            .on_timeout(SCAN_TIMEOUT, || {
                self.report_defect(Defect::Iface(IfaceFailure::Timeout {
                    iface_id: self.iface_id,
                    source: TimeoutSource::Scan,
                }));
                Err(format_err!("Timed out waiting on scan response from SME"))
            })
            .await
    }

    pub fn log_aborted_scan_defect(&self) {
        self.report_defect(Defect::Iface(IfaceFailure::CanceledScan { iface_id: self.iface_id }))
    }

    pub fn log_failed_scan_defect(&self) {
        self.report_defect(Defect::Iface(IfaceFailure::FailedScan { iface_id: self.iface_id }))
    }

    pub fn log_empty_scan_defect(&self) {
        self.report_defect(Defect::Iface(IfaceFailure::EmptyScanResults {
            iface_id: self.iface_id,
        }))
    }
}

impl DefectReporter for SmeForScan {
    fn defect_sender(&self) -> mpsc::Sender<Defect> {
        self.defect_sender.clone()
    }
}

#[derive(Clone, Debug)]
pub struct SmeForClientStateMachine {
    proxy: fidl_sme::ClientSmeProxy,
    iface_id: u16,
    defect_sender: mpsc::Sender<Defect>,
}

impl SmeForClientStateMachine {
    pub fn new(
        proxy: fidl_sme::ClientSmeProxy,
        iface_id: u16,
        defect_sender: mpsc::Sender<Defect>,
    ) -> Self {
        Self { proxy, iface_id, defect_sender }
    }

    pub async fn connect(
        &self,
        req: &fidl_sme::ConnectRequest,
    ) -> Result<(fidl_sme::ConnectResult, fidl_sme::ConnectTransactionEventStream), anyhow::Error>
    {
        let (connect_txn, remote) = create_proxy();

        self.proxy
            .connect(req, Some(remote))
            .map_err(|e| format_err!("Failed to send command to wlanstack: {:?}", e))?;

        let mut stream = connect_txn.take_event_stream();
        let result = wait_for_connect_result(&mut stream)
            .on_timeout(CONNECT_TIMEOUT, || {
                self.report_defect(Defect::Iface(IfaceFailure::Timeout {
                    iface_id: self.iface_id,
                    source: TimeoutSource::Connect,
                }));
                Err(format_err!("Timed out waiting for connect result from SME."))
            })
            .await?;

        Ok((result, stream))
    }

    pub async fn disconnect(&self, reason: fidl_sme::UserDisconnectReason) -> Result<(), Error> {
        self.proxy
            .disconnect(reason)
            .map_err(|e| format_err!("Failed to send command to wlanstack: {:?}", e))
            .on_timeout(DISCONNECT_TIMEOUT, || {
                self.report_defect(Defect::Iface(IfaceFailure::Timeout {
                    iface_id: self.iface_id,
                    source: TimeoutSource::Disconnect,
                }));
                Err(format_err!("Timed out waiting for disconnect"))
            })
            .await
    }

    pub fn roam(&self, req: &fidl_sme::RoamRequest) -> Result<(), Error> {
        self.proxy.roam(req).map_err(|e| format_err!("Failed to send roam command: {:}", e))
    }

    pub fn take_event_stream(&self) -> fidl_sme::ClientSmeEventStream {
        self.proxy.take_event_stream()
    }

    pub fn sme_for_scan(&self) -> SmeForScan {
        SmeForScan {
            proxy: self.proxy.clone(),
            iface_id: self.iface_id,
            defect_sender: self.defect_sender.clone(),
        }
    }
}

impl DefectReporter for SmeForClientStateMachine {
    fn defect_sender(&self) -> mpsc::Sender<Defect> {
        self.defect_sender.clone()
    }
}

/// Wait until stream returns an OnConnectResult event or None. Ignore other event types.
async fn wait_for_connect_result(
    stream: &mut fidl_sme::ConnectTransactionEventStream,
) -> Result<fidl_sme::ConnectResult, Error> {
    loop {
        let stream_fut = stream.try_next();
        match stream_fut
            .await
            .map_err(|e| format_err!("Failed to receive connect result from sme: {:?}", e))?
        {
            Some(fidl_sme::ConnectTransactionEvent::OnConnectResult { result }) => {
                return Ok(result);
            }
            Some(other) => {
                info!(
                    "Expected ConnectTransactionEvent::OnConnectResult, got {}. Ignoring.",
                    connect_txn_event_name(&other)
                );
            }
            None => {
                bail!("Server closed the ConnectTransaction channel before sending a response");
            }
        };
    }
}

fn connect_txn_event_name(event: &fidl_sme::ConnectTransactionEvent) -> &'static str {
    match event {
        fidl_sme::ConnectTransactionEvent::OnConnectResult { .. } => "OnConnectResult",
        fidl_sme::ConnectTransactionEvent::OnRoamResult { .. } => "OnRoamResult",
        fidl_sme::ConnectTransactionEvent::OnDisconnect { .. } => "OnDisconnect",
        fidl_sme::ConnectTransactionEvent::OnSignalReport { .. } => "OnSignalReport",
        fidl_sme::ConnectTransactionEvent::OnChannelSwitched { .. } => "OnChannelSwitched",
    }
}

#[derive(Clone, Debug)]
pub struct SmeForApStateMachine {
    proxy: fidl_sme::ApSmeProxy,
    iface_id: u16,
    defect_sender: mpsc::Sender<Defect>,
}

impl SmeForApStateMachine {
    pub fn new(
        proxy: fidl_sme::ApSmeProxy,
        iface_id: u16,
        defect_sender: mpsc::Sender<Defect>,
    ) -> Self {
        Self { proxy, iface_id, defect_sender }
    }

    pub async fn start(
        &self,
        config: &fidl_sme::ApConfig,
    ) -> Result<fidl_sme::StartApResultCode, Error> {
        self.proxy
            .start(config)
            .map_err(|e| format_err!("Failed to send command to wlanstack: {:?}", e))
            .on_timeout(START_AP_TIMEOUT, || {
                self.report_defect(Defect::Iface(IfaceFailure::Timeout {
                    iface_id: self.iface_id,
                    source: TimeoutSource::ApStart,
                }));
                Err(format_err!("Timed out waiting for AP to start"))
            })
            .await
    }

    pub async fn stop(&self) -> Result<fidl_sme::StopApResultCode, Error> {
        self.proxy
            .stop()
            .map_err(|e| format_err!("Failed to send command to wlanstack: {:?}", e))
            .on_timeout(STOP_AP_TIMEOUT, || {
                self.report_defect(Defect::Iface(IfaceFailure::Timeout {
                    iface_id: self.iface_id,
                    source: TimeoutSource::ApStop,
                }));
                Err(format_err!("Timed out waiting for AP to stop"))
            })
            .await
    }

    pub async fn status(&self) -> Result<fidl_sme::ApStatusResponse, Error> {
        self.proxy
            .status()
            .map_err(|e| format_err!("Failed to send command to wlanstack: {:?}", e))
            .on_timeout(AP_STATUS_TIMEOUT, || {
                self.report_defect(Defect::Iface(IfaceFailure::Timeout {
                    iface_id: self.iface_id,
                    source: TimeoutSource::ApStatus,
                }));
                Err(format_err!("Timed out waiting for AP status"))
            })
            .await
    }

    pub fn take_event_stream(&self) -> fidl_sme::ApSmeEventStream {
        self.proxy.take_event_stream()
    }
}

impl DefectReporter for SmeForApStateMachine {
    fn defect_sender(&self) -> mpsc::Sender<Defect> {
        self.defect_sender.clone()
    }
}

// A request to connect to a specific candidate, and count of attempts to find a BSS.
#[cfg_attr(test, derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct ConnectAttemptRequest {
    pub network: client_types::NetworkIdentifier,
    pub credential: Credential,
    pub reason: client_types::ConnectReason,
    pub attempts: u8,
}

impl ConnectAttemptRequest {
    pub fn new(
        network: client_types::NetworkIdentifier,
        credential: Credential,
        reason: client_types::ConnectReason,
    ) -> Self {
        ConnectAttemptRequest { network, credential, reason, attempts: 0 }
    }
}

impl From<client_types::ConnectSelection> for ConnectAttemptRequest {
    fn from(selection: client_types::ConnectSelection) -> ConnectAttemptRequest {
        ConnectAttemptRequest::new(
            selection.target.network,
            selection.target.credential,
            selection.reason,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access_point::types;
    use crate::util::testing::{generate_connect_selection, poll_sme_req};
    use anyhow::format_err;
    use assert_matches::assert_matches;
    use fidl::endpoints::{RequestStream, create_proxy};
    use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
    use fidl_fuchsia_wlan_internal as fidl_internal;
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use futures::future::LocalBoxFuture;
    use futures::stream::StreamFuture;
    use futures::task::Poll;
    use std::pin::pin;
    use test_case::test_case;
    use wlan_common::channel::Bandwidth;
    use wlan_common::sequestered::Sequestered;
    use wlan_common::{RadioConfig, random_fidl_bss_description};

    struct TestValues {
        exec: fasync::TestExecutor,
        iface_manager: IfaceManager,
        receiver: mpsc::Receiver<IfaceManagerRequest>,
    }

    fn test_setup() -> TestValues {
        let exec = fasync::TestExecutor::new();
        let (sender, receiver) = mpsc::channel(1);
        TestValues { exec, iface_manager: IfaceManager { sender }, receiver }
    }

    #[allow(clippy::enum_variant_names, reason = "mass allow for https://fxbug.dev/381896734")]
    #[derive(Clone)]
    enum NegativeTestFailureMode {
        RequestFailure,
        OperationFailure,
        ServiceFailure,
    }

    fn handle_negative_test_result_responder<T: std::fmt::Debug>(
        responder: oneshot::Sender<Result<T, Error>>,
        failure_mode: NegativeTestFailureMode,
    ) {
        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {
                panic!("Test bug: this request should have been handled previously")
            }
            NegativeTestFailureMode::OperationFailure => {
                responder
                    .send(Err(format_err!("operation failed")))
                    .expect("failed to send response");
            }
            NegativeTestFailureMode::ServiceFailure => {
                // Just drop the responder so that the client side sees a failure.
                drop(responder);
            }
        }
    }

    fn handle_negative_test_responder<T: std::fmt::Debug>(
        responder: oneshot::Sender<T>,
        failure_mode: NegativeTestFailureMode,
    ) {
        match failure_mode {
            NegativeTestFailureMode::RequestFailure | NegativeTestFailureMode::OperationFailure => {
                panic!("Test bug: invalid operation")
            }
            NegativeTestFailureMode::ServiceFailure => {
                // Just drop the responder so that the client side sees a failure.
                drop(responder);
            }
        }
    }

    fn iface_manager_api_negative_test(
        mut receiver: mpsc::Receiver<IfaceManagerRequest>,
        failure_mode: NegativeTestFailureMode,
    ) -> LocalBoxFuture<'static, ()> {
        if let NegativeTestFailureMode::RequestFailure = failure_mode {
            // Drop the receiver so that no requests can be made.
            drop(receiver);
            let fut = async move {};
            return Box::pin(fut);
        }

        let fut = async move {
            let req = match receiver.next().await {
                Some(req) => req,
                None => panic!("no request available."),
            };

            match req {
                // Result<(), Err> responder values
                IfaceManagerRequest::StopClientConnections(StopClientConnectionsRequest {
                    responder,
                    ..
                })
                | IfaceManagerRequest::Disconnect(DisconnectRequest { responder, .. })
                | IfaceManagerRequest::StopAp(StopApRequest { responder, .. })
                | IfaceManagerRequest::StopAllAps(StopAllApsRequest { responder, .. })
                | IfaceManagerRequest::SetCountry(SetCountryRequest { responder, .. })
                | IfaceManagerRequest::StartClientConnections(StartClientConnectionsRequest {
                    responder,
                })
                | IfaceManagerRequest::Connect(ConnectRequest { responder, .. }) => {
                    handle_negative_test_result_responder(responder, failure_mode);
                }
                // Result<ClientSmeProxy, Err>
                IfaceManagerRequest::GetScanProxy(ScanProxyRequest { responder }) => {
                    handle_negative_test_result_responder(responder, failure_mode);
                }
                // Result<oneshot::Receiver<()>, Err>
                IfaceManagerRequest::StartAp(StartApRequest { responder, .. }) => {
                    handle_negative_test_result_responder(responder, failure_mode);
                }
                // Unit responder values
                IfaceManagerRequest::RecordIdleIface(RecordIdleIfaceRequest {
                    responder, ..
                })
                | IfaceManagerRequest::AddIface(AddIfaceRequest { responder, .. })
                | IfaceManagerRequest::RemoveIface(RemoveIfaceRequest { responder, .. }) => {
                    handle_negative_test_responder(responder, failure_mode);
                }
                // Boolean responder values
                IfaceManagerRequest::HasIdleIface(HasIdleIfaceRequest { responder }) => {
                    handle_negative_test_responder(responder, failure_mode);
                }
            }
        };
        Box::pin(fut)
    }

    #[fuchsia::test]
    fn test_disconnect_succeeds() {
        let mut test_values = test_setup();

        // Issue a disconnect command and wait for the command to be sent.
        let req = ap_types::NetworkIdentifier {
            ssid: Ssid::try_from("foo").unwrap(),
            security_type: ap_types::SecurityType::None,
        };
        let req_reason = client_types::DisconnectReason::NetworkUnsaved;
        let disconnect_fut = test_values.iface_manager.disconnect(req.clone(), req_reason);
        let mut disconnect_fut = pin!(disconnect_fut);

        assert_matches!(test_values.exec.run_until_stalled(&mut disconnect_fut), Poll::Pending);

        // Verify that the receiver sees the command and send back a response.
        let next_message = test_values.receiver.next();
        let mut next_message = pin!(next_message);

        assert_matches!(
            test_values.exec.run_until_stalled(&mut next_message),
            Poll::Ready(Some(IfaceManagerRequest::Disconnect(DisconnectRequest {
                network_id, responder, reason
            }))) => {
                assert_eq!(network_id, req);
                assert_eq!(reason, req_reason);
                responder.send(Ok(())).expect("failed to send disconnect response");
            }
        );

        // Verify that the disconnect requestr receives the response.
        assert_matches!(
            test_values.exec.run_until_stalled(&mut disconnect_fut),
            Poll::Ready(Ok(()))
        );
    }

    #[test_case(NegativeTestFailureMode::RequestFailure; "request failure")]
    #[test_case(NegativeTestFailureMode::OperationFailure; "operation failure")]
    #[test_case(NegativeTestFailureMode::ServiceFailure; "service failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn disconnect_negative_test(failure_mode: NegativeTestFailureMode) {
        let mut test_values = test_setup();

        // Issue a disconnect command and wait for the command to be sent.
        let req = ap_types::NetworkIdentifier {
            ssid: Ssid::try_from("foo").unwrap(),
            security_type: ap_types::SecurityType::None,
        };
        let disconnect_fut = test_values
            .iface_manager
            .disconnect(req.clone(), client_types::DisconnectReason::NetworkUnsaved);
        let mut disconnect_fut = pin!(disconnect_fut);

        let service_fut =
            iface_manager_api_negative_test(test_values.receiver, failure_mode.clone());
        let mut service_fut = pin!(service_fut);

        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {}
            _ => {
                // Run the request and the servicing of the request
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut disconnect_fut),
                    Poll::Pending
                );
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut service_fut),
                    Poll::Ready(())
                );
            }
        }

        // Verify that the disconnect requestr receives the response.
        assert_matches!(
            test_values.exec.run_until_stalled(&mut disconnect_fut),
            Poll::Ready(Err(_))
        );
    }

    #[fuchsia::test]
    fn test_connect_succeeds() {
        let mut test_values = test_setup();

        // Issue a connect command and wait for the command to be sent.
        let req = ConnectAttemptRequest::new(
            client_types::NetworkIdentifier {
                ssid: Ssid::try_from("foo").unwrap(),
                security_type: client_types::SecurityType::None,
            },
            Credential::None,
            client_types::ConnectReason::FidlConnectRequest,
        );
        let connect_fut = test_values.iface_manager.connect(req.clone());
        let mut connect_fut = pin!(connect_fut);

        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);

        // Verify that the receiver sees the command and send back a response.
        let next_message = test_values.receiver.next();
        let mut next_message = pin!(next_message);

        assert_matches!(
            test_values.exec.run_until_stalled(&mut next_message),
            Poll::Ready(Some(IfaceManagerRequest::Connect(ConnectRequest {
                request, responder
            }))) => {
                assert_eq!(request, req);
                responder.send(Ok(())).expect("failed to send connect response");
            }
        );

        // Verify that the connect requestr receives the response.
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Ready(Ok(_)));
    }

    #[test_case(NegativeTestFailureMode::RequestFailure; "request failure")]
    #[test_case(NegativeTestFailureMode::OperationFailure; "operation failure")]
    #[test_case(NegativeTestFailureMode::ServiceFailure; "service failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn connect_negative_test(failure_mode: NegativeTestFailureMode) {
        let mut test_values = test_setup();

        // Issue a connect command and wait for the command to be sent.
        let req = ConnectAttemptRequest::new(
            client_types::NetworkIdentifier {
                ssid: Ssid::try_from("foo").unwrap(),
                security_type: client_types::SecurityType::None,
            },
            Credential::None,
            client_types::ConnectReason::FidlConnectRequest,
        );
        let connect_fut = test_values.iface_manager.connect(req.clone());
        let mut connect_fut = pin!(connect_fut);

        let service_fut =
            iface_manager_api_negative_test(test_values.receiver, failure_mode.clone());
        let mut service_fut = pin!(service_fut);

        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {}
            _ => {
                // Run the request and the servicing of the request
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut connect_fut),
                    Poll::Pending
                );
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut service_fut),
                    Poll::Ready(())
                );
            }
        }

        // Verify that the request completes in error.
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_record_idle_client_succeeds() {
        let mut test_values = test_setup();

        // Request that an idle client be recorded.
        let iface_id = 123;
        let idle_client_fut = test_values.iface_manager.record_idle_client(iface_id);
        let mut idle_client_fut = pin!(idle_client_fut);

        assert_matches!(test_values.exec.run_until_stalled(&mut idle_client_fut), Poll::Pending);

        // Verify that the receiver sees the request.
        let next_message = test_values.receiver.next();
        let mut next_message = pin!(next_message);

        assert_matches!(
            test_values.exec.run_until_stalled(&mut next_message),
            Poll::Ready(
                Some(IfaceManagerRequest::RecordIdleIface(RecordIdleIfaceRequest{ iface_id: 123, responder}))
            ) => {
                responder.send(()).expect("failed to send idle iface response");
            }
        );

        // Verify that the client sees the response.
        assert_matches!(
            test_values.exec.run_until_stalled(&mut idle_client_fut),
            Poll::Ready(Ok(()))
        );
    }

    #[test_case(NegativeTestFailureMode::RequestFailure; "request failure")]
    #[test_case(NegativeTestFailureMode::ServiceFailure; "service failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_record_idle_client_service_failure(failure_mode: NegativeTestFailureMode) {
        let mut test_values = test_setup();

        // Request that an idle client be recorded.
        let iface_id = 123;
        let idle_client_fut = test_values.iface_manager.record_idle_client(iface_id);
        let mut idle_client_fut = pin!(idle_client_fut);

        let service_fut =
            iface_manager_api_negative_test(test_values.receiver, failure_mode.clone());
        let mut service_fut = pin!(service_fut);

        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {}
            _ => {
                // Run the request and the servicing of the request
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut idle_client_fut),
                    Poll::Pending
                );
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut service_fut),
                    Poll::Ready(())
                );
            }
        }

        // Verify that the client side finishes
        assert_matches!(
            test_values.exec.run_until_stalled(&mut idle_client_fut),
            Poll::Ready(Err(_))
        );
    }

    #[fuchsia::test]
    fn test_has_idle_client_success() {
        let mut test_values = test_setup();

        // Query whether there is an idle client
        let idle_client_fut = test_values.iface_manager.has_idle_client();
        let mut idle_client_fut = pin!(idle_client_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut idle_client_fut), Poll::Pending);

        // Verify that the service sees the query
        let next_message = test_values.receiver.next();
        let mut next_message = pin!(next_message);

        assert_matches!(
            test_values.exec.run_until_stalled(&mut next_message),
            Poll::Ready(
                Some(IfaceManagerRequest::HasIdleIface(HasIdleIfaceRequest{ responder}))
            ) => responder.send(true).expect("failed to reply to idle client query")
        );

        // Verify that the client side finishes
        assert_matches!(
            test_values.exec.run_until_stalled(&mut idle_client_fut),
            Poll::Ready(Ok(true))
        );
    }

    #[test_case(NegativeTestFailureMode::RequestFailure; "request failure")]
    #[test_case(NegativeTestFailureMode::ServiceFailure; "service failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn idle_client_negative_test(failure_mode: NegativeTestFailureMode) {
        let mut test_values = test_setup();

        // Query whether there is an idle client
        let idle_client_fut = test_values.iface_manager.has_idle_client();
        let mut idle_client_fut = pin!(idle_client_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut idle_client_fut), Poll::Pending);

        let service_fut =
            iface_manager_api_negative_test(test_values.receiver, failure_mode.clone());
        let mut service_fut = pin!(service_fut);

        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {}
            _ => {
                // Run the request and the servicing of the request
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut idle_client_fut),
                    Poll::Pending
                );
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut service_fut),
                    Poll::Ready(())
                );
            }
        }

        // Verify that the request completes in error.
        assert_matches!(
            test_values.exec.run_until_stalled(&mut idle_client_fut),
            Poll::Ready(Err(_))
        );
    }

    #[fuchsia::test]
    fn test_add_iface_success() {
        let mut test_values = test_setup();

        // Add an interface
        let added_iface_fut = test_values.iface_manager.handle_added_iface(123);
        let mut added_iface_fut = pin!(added_iface_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut added_iface_fut), Poll::Pending);

        // Verify that the service sees the query
        let next_message = test_values.receiver.next();
        let mut next_message = pin!(next_message);

        assert_matches!(
            test_values.exec.run_until_stalled(&mut next_message),
            Poll::Ready(
                Some(IfaceManagerRequest::AddIface(AddIfaceRequest{ iface_id: 123, responder }))
            ) => {
                responder.send(()).expect("failed to respond while adding iface");
            }
        );

        // Verify that the client side finishes
        assert_matches!(
            test_values.exec.run_until_stalled(&mut added_iface_fut),
            Poll::Ready(Ok(()))
        );
    }

    #[test_case(NegativeTestFailureMode::RequestFailure; "request failure")]
    #[test_case(NegativeTestFailureMode::ServiceFailure; "service failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn add_iface_negative_test(failure_mode: NegativeTestFailureMode) {
        let mut test_values = test_setup();

        // Add an interface
        let added_iface_fut = test_values.iface_manager.handle_added_iface(123);
        let mut added_iface_fut = pin!(added_iface_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut added_iface_fut), Poll::Pending);

        let service_fut =
            iface_manager_api_negative_test(test_values.receiver, failure_mode.clone());
        let mut service_fut = pin!(service_fut);

        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {}
            _ => {
                // Run the request and the servicing of the request
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut added_iface_fut),
                    Poll::Pending
                );
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut service_fut),
                    Poll::Ready(())
                );
            }
        }

        // Verify that the request completes in error.
        assert_matches!(
            test_values.exec.run_until_stalled(&mut added_iface_fut),
            Poll::Ready(Err(_))
        );
    }

    #[fuchsia::test]
    fn test_remove_iface_success() {
        let mut test_values = test_setup();

        // Report the removal of an interface.
        let removed_iface_fut = test_values.iface_manager.handle_removed_iface(123);
        let mut removed_iface_fut = pin!(removed_iface_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut removed_iface_fut), Poll::Pending);

        // Verify that the service sees the query
        let next_message = test_values.receiver.next();
        let mut next_message = pin!(next_message);

        assert_matches!(
            test_values.exec.run_until_stalled(&mut next_message),
            Poll::Ready(
                Some(IfaceManagerRequest::RemoveIface(RemoveIfaceRequest{ iface_id: 123, responder }))
            ) => {
                responder.send(()).expect("failed to respond while adding iface");
            }
        );

        // Verify that the client side finishes
        assert_matches!(
            test_values.exec.run_until_stalled(&mut removed_iface_fut),
            Poll::Ready(Ok(()))
        );
    }

    #[test_case(NegativeTestFailureMode::RequestFailure; "request failure")]
    #[test_case(NegativeTestFailureMode::ServiceFailure; "service failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn remove_iface_negative_test(failure_mode: NegativeTestFailureMode) {
        let mut test_values = test_setup();

        // Report the removal of an interface.
        let removed_iface_fut = test_values.iface_manager.handle_removed_iface(123);
        let mut removed_iface_fut = pin!(removed_iface_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut removed_iface_fut), Poll::Pending);

        let service_fut =
            iface_manager_api_negative_test(test_values.receiver, failure_mode.clone());
        let mut service_fut = pin!(service_fut);

        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {}
            _ => {
                // Run the request and the servicing of the request
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut removed_iface_fut),
                    Poll::Pending
                );
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut service_fut),
                    Poll::Ready(())
                );
            }
        }

        // Verify that the client side finishes
        assert_matches!(
            test_values.exec.run_until_stalled(&mut removed_iface_fut),
            Poll::Ready(Err(_))
        );
    }

    #[fuchsia::test]
    fn test_get_scan_proxy_success() {
        let mut test_values = test_setup();

        // Request a scan
        let scan_proxy_fut = test_values.iface_manager.get_sme_proxy_for_scan();
        let mut scan_proxy_fut = pin!(scan_proxy_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut scan_proxy_fut), Poll::Pending);

        // Verify that the service sees the request.
        let next_message = test_values.receiver.next();
        let mut next_message = pin!(next_message);

        assert_matches!(
            test_values.exec.run_until_stalled(&mut next_message),
            Poll::Ready(Some(IfaceManagerRequest::GetScanProxy(ScanProxyRequest{
                responder
            }))) => {
                let (proxy, _) = create_proxy::<fidl_sme::ClientSmeMarker>();
                let (defect_sender, _defect_receiver) = mpsc::channel(100);
                responder.send(Ok(SmeForScan{proxy, iface_id: 0, defect_sender})).expect("failed to send scan sme proxy");
            }
        );

        // Verify that the client side gets the scan proxy
        assert_matches!(
            test_values.exec.run_until_stalled(&mut scan_proxy_fut),
            Poll::Ready(Ok(_))
        );
    }

    #[test_case(NegativeTestFailureMode::RequestFailure; "request failure")]
    #[test_case(NegativeTestFailureMode::OperationFailure; "operation failure")]
    #[test_case(NegativeTestFailureMode::ServiceFailure; "service failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn scan_proxy_negative_test(failure_mode: NegativeTestFailureMode) {
        let mut test_values = test_setup();

        // Request a scan
        let scan_proxy_fut = test_values.iface_manager.get_sme_proxy_for_scan();
        let mut scan_proxy_fut = pin!(scan_proxy_fut);

        let service_fut =
            iface_manager_api_negative_test(test_values.receiver, failure_mode.clone());
        let mut service_fut = pin!(service_fut);

        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {}
            _ => {
                // Run the request and the servicing of the request
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut scan_proxy_fut),
                    Poll::Pending
                );
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut service_fut),
                    Poll::Ready(())
                );
            }
        }

        // Verify that an error is returned.
        assert_matches!(
            test_values.exec.run_until_stalled(&mut scan_proxy_fut),
            Poll::Ready(Err(_))
        );
    }

    #[fuchsia::test]
    fn test_stop_client_connections_succeeds() {
        let mut test_values = test_setup();

        // Request a scan
        let stop_fut = test_values.iface_manager.stop_client_connections(
            client_types::DisconnectReason::FidlStopClientConnectionsRequest,
        );
        let mut stop_fut = pin!(stop_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Pending);

        // Verify that the service sees the request.
        let next_message = test_values.receiver.next();
        let mut next_message = pin!(next_message);

        assert_matches!(
            test_values.exec.run_until_stalled(&mut next_message),
            Poll::Ready(Some(IfaceManagerRequest::StopClientConnections(StopClientConnectionsRequest{
                responder, reason
            }))) => {
                assert_eq!(reason, client_types::DisconnectReason::FidlStopClientConnectionsRequest);
                responder.send(Ok(())).expect("failed sending stop client connections response");
            }
        );

        // Verify that the client side gets the response.
        assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Ready(Ok(())));
    }

    #[test_case(NegativeTestFailureMode::RequestFailure; "request failure")]
    #[test_case(NegativeTestFailureMode::OperationFailure; "operation failure")]
    #[test_case(NegativeTestFailureMode::ServiceFailure; "service failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn stop_client_connections_negative_test(failure_mode: NegativeTestFailureMode) {
        let mut test_values = test_setup();

        // Request a scan
        let stop_fut = test_values.iface_manager.stop_client_connections(
            client_types::DisconnectReason::FidlStopClientConnectionsRequest,
        );
        let mut stop_fut = pin!(stop_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Pending);

        let service_fut =
            iface_manager_api_negative_test(test_values.receiver, failure_mode.clone());
        let mut service_fut = pin!(service_fut);

        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {}
            _ => {
                // Run the request and the servicing of the request
                assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Pending);
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut service_fut),
                    Poll::Ready(())
                );
            }
        }

        // Verify that the client side gets the response.
        assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_start_client_connections_succeeds() {
        let mut test_values = test_setup();

        // Start client connections
        let start_fut = test_values.iface_manager.start_client_connections();
        let mut start_fut = pin!(start_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut start_fut), Poll::Pending);

        // Verify that the service sees the request.
        let next_message = test_values.receiver.next();
        let mut next_message = pin!(next_message);

        assert_matches!(
            test_values.exec.run_until_stalled(&mut next_message),
            Poll::Ready(Some(IfaceManagerRequest::StartClientConnections(StartClientConnectionsRequest{
                responder
            }))) => {
                responder.send(Ok(())).expect("failed sending stop client connections response");
            }
        );

        // Verify that the client side gets the response.
        assert_matches!(test_values.exec.run_until_stalled(&mut start_fut), Poll::Ready(Ok(())));
    }

    #[test_case(NegativeTestFailureMode::RequestFailure; "request failure")]
    #[test_case(NegativeTestFailureMode::OperationFailure; "operation failure")]
    #[test_case(NegativeTestFailureMode::ServiceFailure; "service failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn start_client_connections_negative_test(failure_mode: NegativeTestFailureMode) {
        let mut test_values = test_setup();

        // Start client connections
        let start_fut = test_values.iface_manager.start_client_connections();
        let mut start_fut = pin!(start_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut start_fut), Poll::Pending);

        let service_fut =
            iface_manager_api_negative_test(test_values.receiver, failure_mode.clone());
        let mut service_fut = pin!(service_fut);

        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {}
            _ => {
                // Run the request and the servicing of the request
                assert_matches!(test_values.exec.run_until_stalled(&mut start_fut), Poll::Pending);
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut service_fut),
                    Poll::Ready(())
                );
            }
        }

        // Verify that the client side gets the response.
        assert_matches!(test_values.exec.run_until_stalled(&mut start_fut), Poll::Ready(Err(_)));
    }

    fn create_ap_config() -> ap_fsm::ApConfig {
        ap_fsm::ApConfig {
            id: types::NetworkIdentifier {
                ssid: Ssid::try_from("foo").unwrap(),
                security_type: types::SecurityType::None,
            },
            credential: vec![],
            radio_config: RadioConfig::new(
                fidl_fuchsia_wlan_ieee80211::WlanPhyType::Ht,
                Bandwidth::Cbw20,
                6,
                fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz,
            ),
            mode: types::ConnectivityMode::Unrestricted,
            band: types::OperatingBand::Any,
        }
    }

    #[fuchsia::test]
    fn test_start_ap_succeeds() {
        let mut test_values = test_setup();

        // Start an AP
        let start_fut = test_values.iface_manager.start_ap(create_ap_config());
        let mut start_fut = pin!(start_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut start_fut), Poll::Pending);

        // Verify the service sees the request
        let next_message = test_values.receiver.next();
        let mut next_message = pin!(next_message);

        assert_matches!(
            test_values.exec.run_until_stalled(&mut next_message),
            Poll::Ready(Some(IfaceManagerRequest::StartAp(StartApRequest{
                config, responder
            }))) => {
                assert_eq!(config, create_ap_config());

                let (_, receiver) = oneshot::channel();
                responder.send(Ok(receiver)).expect("failed to send start AP response");
            }
        );

        // Verify that the client gets the response
        assert_matches!(test_values.exec.run_until_stalled(&mut start_fut), Poll::Ready(Ok(_)));
    }

    #[test_case(NegativeTestFailureMode::RequestFailure; "request failure")]
    #[test_case(NegativeTestFailureMode::OperationFailure; "operation failure")]
    #[test_case(NegativeTestFailureMode::ServiceFailure; "service failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn start_ap_negative_test(failure_mode: NegativeTestFailureMode) {
        let mut test_values = test_setup();

        // Start an AP
        let start_fut = test_values.iface_manager.start_ap(create_ap_config());
        let mut start_fut = pin!(start_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut start_fut), Poll::Pending);

        let service_fut =
            iface_manager_api_negative_test(test_values.receiver, failure_mode.clone());
        let mut service_fut = pin!(service_fut);

        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {}
            _ => {
                // Run the request and the servicing of the request
                assert_matches!(test_values.exec.run_until_stalled(&mut start_fut), Poll::Pending);
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut service_fut),
                    Poll::Ready(())
                );
            }
        }

        // Verify that the client gets the response
        assert_matches!(test_values.exec.run_until_stalled(&mut start_fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_stop_ap_succeeds() {
        let mut test_values = test_setup();

        // Stop an AP
        let stop_fut = test_values
            .iface_manager
            .stop_ap(Ssid::try_from("foo").unwrap(), "bar".as_bytes().to_vec());
        let mut stop_fut = pin!(stop_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Pending);

        // Verify the service sees the request
        let next_message = test_values.receiver.next();
        let mut next_message = pin!(next_message);

        assert_matches!(
            test_values.exec.run_until_stalled(&mut next_message),
            Poll::Ready(Some(IfaceManagerRequest::StopAp(StopApRequest{
                ssid, password, responder
            }))) => {
                assert_eq!(ssid, Ssid::try_from("foo").unwrap());
                assert_eq!(password, "bar".as_bytes().to_vec());

                responder.send(Ok(())).expect("failed to send stop AP response");
            }
        );

        // Verify that the client gets the response
        assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Ready(Ok(_)));
    }

    #[test_case(NegativeTestFailureMode::RequestFailure; "request failure")]
    #[test_case(NegativeTestFailureMode::OperationFailure; "operation failure")]
    #[test_case(NegativeTestFailureMode::ServiceFailure; "service failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn stop_ap_negative_test(failure_mode: NegativeTestFailureMode) {
        let mut test_values = test_setup();

        // Stop an AP
        let stop_fut = test_values
            .iface_manager
            .stop_ap(Ssid::try_from("foo").unwrap(), "bar".as_bytes().to_vec());
        let mut stop_fut = pin!(stop_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Pending);

        let service_fut =
            iface_manager_api_negative_test(test_values.receiver, failure_mode.clone());
        let mut service_fut = pin!(service_fut);

        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {}
            _ => {
                // Run the request and the servicing of the request
                assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Pending);
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut service_fut),
                    Poll::Ready(())
                );
            }
        }

        // Verify that the client gets the response
        assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_stop_all_aps_succeeds() {
        let mut test_values = test_setup();

        // Stop an AP
        let stop_fut = test_values.iface_manager.stop_all_aps();
        let mut stop_fut = pin!(stop_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Pending);

        // Verify the service sees the request
        let next_message = test_values.receiver.next();
        let mut next_message = pin!(next_message);
        assert_matches!(
            test_values.exec.run_until_stalled(&mut next_message),
            Poll::Ready(Some(IfaceManagerRequest::StopAllAps(StopAllApsRequest{
                responder
            }))) => {
                responder.send(Ok(())).expect("failed to send stop AP response");
            }
        );

        // Verify that the client gets the response
        assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Ready(Ok(_)));
    }

    #[test_case(NegativeTestFailureMode::RequestFailure; "request failure")]
    #[test_case(NegativeTestFailureMode::OperationFailure; "operation failure")]
    #[test_case(NegativeTestFailureMode::ServiceFailure; "service failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn stop_all_aps_negative_test(failure_mode: NegativeTestFailureMode) {
        let mut test_values = test_setup();

        // Stop an AP
        let stop_fut = test_values.iface_manager.stop_all_aps();
        let mut stop_fut = pin!(stop_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Pending);

        let service_fut =
            iface_manager_api_negative_test(test_values.receiver, failure_mode.clone());
        let mut service_fut = pin!(service_fut);

        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {}
            _ => {
                // Run the request and the servicing of the request
                assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Pending);
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut service_fut),
                    Poll::Ready(())
                );
            }
        }

        // Verify that the client gets the response
        assert_matches!(test_values.exec.run_until_stalled(&mut stop_fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_set_country_succeeds() {
        let mut test_values = test_setup();

        // Set country code
        let set_country_fut = test_values.iface_manager.set_country(None);
        let mut set_country_fut = pin!(set_country_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut set_country_fut), Poll::Pending);

        // Verify the service sees the request
        let next_message = test_values.receiver.next();
        let mut next_message = pin!(next_message);

        assert_matches!(
            test_values.exec.run_until_stalled(&mut next_message),
            Poll::Ready(Some(IfaceManagerRequest::SetCountry(SetCountryRequest{
                country_code: None,
                responder
            }))) => {
                responder.send(Ok(())).expect("failed to send stop AP response");
            }
        );

        // Verify that the client gets the response
        assert_matches!(
            test_values.exec.run_until_stalled(&mut set_country_fut),
            Poll::Ready(Ok(_))
        );
    }

    #[test_case(NegativeTestFailureMode::RequestFailure; "request failure")]
    #[test_case(NegativeTestFailureMode::OperationFailure; "operation failure")]
    #[test_case(NegativeTestFailureMode::ServiceFailure; "service failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn set_country_negative_test(failure_mode: NegativeTestFailureMode) {
        let mut test_values = test_setup();

        // Set country code
        let set_country_fut = test_values.iface_manager.set_country(None);
        let mut set_country_fut = pin!(set_country_fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut set_country_fut), Poll::Pending);
        let service_fut =
            iface_manager_api_negative_test(test_values.receiver, failure_mode.clone());
        let mut service_fut = pin!(service_fut);

        match failure_mode {
            NegativeTestFailureMode::RequestFailure => {}
            _ => {
                // Run the request and the servicing of the request
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut set_country_fut),
                    Poll::Pending
                );
                assert_matches!(
                    test_values.exec.run_until_stalled(&mut service_fut),
                    Poll::Ready(())
                );
            }
        }

        // Verify that the client gets the response
        assert_matches!(
            test_values.exec.run_until_stalled(&mut set_country_fut),
            Poll::Ready(Err(_))
        );
    }

    #[fuchsia::test]
    fn test_sme_for_scan() {
        let mut exec = fasync::TestExecutor::new();

        // Build an SME specifically for scanning.
        let (proxy, server_end) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let (defect_sender, _defect_receiver) = mpsc::channel(100);
        let sme = SmeForScan::new(proxy, 0, defect_sender);
        let mut sme_stream = server_end.into_stream();

        // Construct a scan request.
        let scan_request = fidl_sme::ScanRequest::Active(fidl_sme::ActiveScanRequest {
            ssids: vec![vec![]],
            channels: vec![],
        });

        // Issue the scan request.
        let scan_result_fut = sme.scan(&scan_request);
        let mut scan_result_fut = pin!(scan_result_fut);
        assert_matches!(exec.run_until_stalled(&mut scan_result_fut), Poll::Pending);

        // Poll the server end of the SME and expect that a scan request has been forwarded.
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan {
                req, ..
            }))) => {
                assert_eq!(scan_request, req)
            }
        );
    }

    #[fuchsia::test]
    fn sme_for_scan_defects() {
        let _exec = fasync::TestExecutor::new();

        // Build an SME specifically for scanning.
        let (proxy, _) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let (defect_sender, mut defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForScan::new(proxy, iface_id, defect_sender);

        sme.log_aborted_scan_defect();
        sme.log_failed_scan_defect();
        sme.log_empty_scan_defect();

        assert_eq!(
            defect_receiver.try_next().expect("missing canceled scan error"),
            Some(Defect::Iface(IfaceFailure::CanceledScan { iface_id })),
        );
        assert_eq!(
            defect_receiver.try_next().expect("missing failed scan error"),
            Some(Defect::Iface(IfaceFailure::FailedScan { iface_id })),
        );
        assert_eq!(
            defect_receiver.try_next().expect("missing empty scan results error"),
            Some(Defect::Iface(IfaceFailure::EmptyScanResults { iface_id })),
        );
    }

    #[fuchsia::test]
    fn sme_for_scan_timeout() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        // Create the SmeForScan
        let (proxy, _server) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let (defect_sender, mut defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForScan::new(proxy, iface_id, defect_sender);

        // Issue the scan request.
        let scan_request = fidl_sme::ScanRequest::Active(fidl_sme::ActiveScanRequest {
            ssids: vec![vec![]],
            channels: vec![],
        });
        let scan_result_fut = sme.scan(&scan_request);
        let mut scan_result_fut = pin!(scan_result_fut);
        assert_matches!(exec.run_until_stalled(&mut scan_result_fut), Poll::Pending);

        // Advance the clock so that the timeout expires.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            SCAN_TIMEOUT + fasync::MonotonicDuration::from_seconds(1),
        ));

        // Verify that the future returns and that a defect is logged.
        assert_matches!(exec.run_until_stalled(&mut scan_result_fut), Poll::Ready(Err(_)));
        assert_eq!(
            defect_receiver.try_next().expect("missing empty scan results error"),
            Some(Defect::Iface(IfaceFailure::Timeout { iface_id, source: TimeoutSource::Scan })),
        );
    }

    #[fuchsia::test]
    fn state_machine_sme_disconnects_successfully() {
        let mut exec = fasync::TestExecutor::new();

        // Build an SME wrapper.
        let (proxy, sme_fut) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let (defect_sender, _defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForClientStateMachine::new(proxy, iface_id, defect_sender);

        // Request a disconnect and run the future until it stalls.
        let fut = sme.disconnect(fidl_sme::UserDisconnectReason::FidlStopClientConnectionsRequest);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ack the disconnect request.
        let mut sme_fut = pin!(sme_fut.into_stream().into_future());
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{
                responder,
                reason: fidl_sme::UserDisconnectReason::FidlStopClientConnectionsRequest
            }) => {
                responder.send().expect("could not send sme response");
            }
        );

        // Verify that the disconnect was successful.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }

    #[fuchsia::test]
    fn state_machine_sme_fails_to_disconnect() {
        let mut exec = fasync::TestExecutor::new();

        // Build an SME wrapper.
        let (proxy, _) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let (defect_sender, _defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForClientStateMachine::new(proxy, iface_id, defect_sender);

        // Request a disconnect and expect an immediate error return.
        let fut = sme.disconnect(fidl_sme::UserDisconnectReason::FidlStopClientConnectionsRequest);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn state_machine_sme_disconnect_timeout() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        // Build an SME wrapper.
        let (proxy, _sme_fut) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let (defect_sender, mut defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForClientStateMachine::new(proxy, iface_id, defect_sender);

        // Request a disconnect and run the future until it stalls.
        let fut = sme.disconnect(fidl_sme::UserDisconnectReason::FidlStopClientConnectionsRequest);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Advance the clock beyond the timeout.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            DISCONNECT_TIMEOUT + fasync::MonotonicDuration::from_seconds(1),
        ));

        // Verify that the future returns and that a defect is logged.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
        assert_eq!(
            defect_receiver.try_next().expect("missing empty scan results error"),
            Some(Defect::Iface(IfaceFailure::Timeout {
                iface_id,
                source: TimeoutSource::Disconnect,
            })),
        );
    }

    #[fuchsia::test]
    fn state_machine_sme_roam_sends_request() {
        let mut exec = fasync::TestExecutor::new();

        // Build an SME wrapper.
        let (proxy, sme_fut) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let (defect_sender, _defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForClientStateMachine::new(proxy, iface_id, defect_sender);

        // Request a roam via the sme proxy
        let roam_request =
            fidl_sme::RoamRequest { bss_description: random_fidl_bss_description!() };
        sme.roam(&roam_request).unwrap();

        // Verify SME gets the request
        let mut sme_fut = pin!(sme_fut.into_stream().into_future());
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Roam {
                req, ..
            }) => {
                assert_eq!(req, roam_request);
            }
        );
    }

    fn generate_connect_request() -> fidl_sme::ConnectRequest {
        let connection_selection = generate_connect_selection();
        fidl_sme::ConnectRequest {
            ssid: connection_selection.target.network.ssid.to_vec(),
            bss_description: Sequestered::release(connection_selection.target.bss.bss_description),
            multiple_bss_candidates: connection_selection.target.network_has_multiple_bss,
            authentication: connection_selection.target.authenticator.clone().into(),
            deprecated_scan_type: fidl_fuchsia_wlan_common::ScanType::Active,
        }
    }

    #[fuchsia::test]
    fn state_machine_sme_connects_successfully() {
        let mut exec = fasync::TestExecutor::new();

        // Build an SME wrapper.
        let (proxy, sme_fut) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let (defect_sender, _defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForClientStateMachine::new(proxy, iface_id, defect_sender);

        // Request a connection.
        let connect_request = generate_connect_request();
        let fut = sme.connect(&connect_request);
        let mut fut = pin!(fut);
        match exec.run_until_stalled(&mut fut) {
            Poll::Pending => {}
            _ => panic!("connect request should be pending."),
        }

        // Ack the connect request.
        let mut sme_fut = pin!(sme_fut.into_stream().into_future());
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{
                txn,
                ..
            }) => {
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
                    .send_on_connect_result(&fidl_sme::ConnectResult {
                        code: fidl_ieee80211::StatusCode::Success,
                        is_credential_rejected: false,
                        is_reconnect: false,
                    })
                    .expect("failed to send connection completion");
            }
        );

        // Expect a successful result.
        match exec.run_until_stalled(&mut fut) {
            Poll::Ready(Ok((result, txn))) => {
                assert_eq!(
                    result,
                    fidl_sme::ConnectResult {
                        code: fidl_ieee80211::StatusCode::Success,
                        is_credential_rejected: false,
                        is_reconnect: false,
                    }
                );
                // This is required or else the test panics on exit with
                // "receivers must not outlive their executor".
                drop(txn)
            }
            Poll::Ready(Err(_)) => panic!("connection should be successful"),
            Poll::Pending => panic!("connect request should not be pending."),
        }
    }

    #[fuchsia::test]
    fn state_machine_sme_connection_failure() {
        let mut exec = fasync::TestExecutor::new();

        // Build an SME wrapper.
        let (proxy, sme_fut) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let (defect_sender, _defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForClientStateMachine::new(proxy, iface_id, defect_sender);

        // Request a connection.
        let connect_request = generate_connect_request();
        let fut = sme.connect(&connect_request);
        let mut fut = pin!(fut);
        match exec.run_until_stalled(&mut fut) {
            Poll::Pending => {}
            _ => panic!("connect request should be pending."),
        }

        // Ack the connect request.
        let mut sme_fut = pin!(sme_fut.into_stream().into_future());
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{
                txn,
                ..
            }) => {
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
                    .send_on_connect_result(&fidl_sme::ConnectResult {
                        code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                        is_credential_rejected: false,
                        is_reconnect: false,
                    })
                    .expect("failed to send connection completion");
            }
        );

        // Expect a successful result.
        match exec.run_until_stalled(&mut fut) {
            Poll::Ready(Ok((result, txn))) => {
                assert_eq!(
                    result,
                    fidl_sme::ConnectResult {
                        code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                        is_credential_rejected: false,
                        is_reconnect: false,
                    }
                );
                // This is required or else the test panics on exit with
                // "receivers must not outlive their executor".
                drop(txn)
            }
            Poll::Ready(Err(_)) => panic!("connection should be successful"),
            Poll::Pending => panic!("connect request should not be pending."),
        }
    }

    #[fuchsia::test]
    fn state_machine_sme_connect_request_fails() {
        let mut exec = fasync::TestExecutor::new();

        // Build an SME wrapper.
        let (proxy, _) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let (defect_sender, _defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForClientStateMachine::new(proxy, iface_id, defect_sender);

        // Request a disconnect and expect an immediate error return.
        let connect_request = generate_connect_request();
        let fut = sme.connect(&connect_request);
        let mut fut = pin!(fut);
        match exec.run_until_stalled(&mut fut) {
            Poll::Ready(Err(_)) => {}
            _ => panic!("connect request should have failed."),
        }
    }

    #[fuchsia::test]
    fn state_machine_sme_connect_timeout() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        // Build an SME wrapper.
        let (proxy, _sme_fut) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let (defect_sender, mut defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForClientStateMachine::new(proxy, iface_id, defect_sender);

        // Request a connection and run the future until it stalls.
        let connect_request = generate_connect_request();
        let fut = sme.connect(&connect_request);
        let mut fut = pin!(fut);
        match exec.run_until_stalled(&mut fut) {
            Poll::Pending => {}
            _ => panic!("connect future completed unexpectedly"),
        }

        // Advance the clock beyond the timeout.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            CONNECT_TIMEOUT + fasync::MonotonicDuration::from_seconds(1),
        ));

        // Verify that the future returns and that a defect is logged.
        match exec.run_until_stalled(&mut fut) {
            Poll::Ready(Err(_)) => {}
            Poll::Ready(Ok(_)) => panic!("connect future completed successfully"),
            Poll::Pending => panic!("connect future did not complete"),
        }
        assert_eq!(
            defect_receiver.try_next().expect("missing connection timeout"),
            Some(Defect::Iface(IfaceFailure::Timeout { iface_id, source: TimeoutSource::Connect })),
        );
    }

    #[fuchsia::test]
    fn wait_for_connect_result_error() {
        let mut exec = fasync::TestExecutor::new();
        let (connect_txn, remote) = create_proxy::<fidl_sme::ConnectTransactionMarker>();
        let mut response_stream = connect_txn.take_event_stream();

        let fut = wait_for_connect_result(&mut response_stream);

        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Drop server end, and verify future completes with error
        drop(remote);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn wait_for_connect_result_ignores_other_events() {
        let mut exec = fasync::TestExecutor::new();
        let (connect_txn, remote) = create_proxy::<fidl_sme::ConnectTransactionMarker>();
        let request_handle = remote.into_stream().control_handle();
        let mut response_stream = connect_txn.take_event_stream();

        let fut = wait_for_connect_result(&mut response_stream);

        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Send some unexpected response
        let ind =
            fidl_internal::SignalReportIndication { rssi_dbm: -20, snr_db: 25, tx_rate_500kbps: 0 };
        request_handle.send_on_signal_report(&ind).unwrap();

        // Future should still be waiting for OnConnectResult event
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Send expected ConnectResult response
        let sme_result = fidl_sme::ConnectResult {
            code: fidl_ieee80211::StatusCode::Success,
            is_credential_rejected: false,
            is_reconnect: false,
        };
        request_handle.send_on_connect_result(&sme_result).unwrap();
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(response)) => {
            assert_eq!(sme_result, response);
        });
    }

    fn poll_ap_sme_req(
        exec: &mut fasync::TestExecutor,
        next_sme_req: &mut StreamFuture<fidl_sme::ApSmeRequestStream>,
    ) -> Poll<fidl_sme::ApSmeRequest> {
        exec.run_until_stalled(next_sme_req).map(|(req, stream)| {
            *next_sme_req = stream.into_future();
            req.expect("did not expect the SME request stream to end")
                .expect("error polling SME request stream")
        })
    }

    #[fuchsia::test]
    fn state_machine_sme_starts_ap_successfully() {
        let mut exec = fasync::TestExecutor::new();

        // Build an SME wrapper.
        let (proxy, sme_fut) = create_proxy::<fidl_sme::ApSmeMarker>();
        let (defect_sender, _defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForApStateMachine::new(proxy, iface_id, defect_sender);

        // Start the AP and run the future until it stalls.
        let config = fidl_sme::ApConfig::from(create_ap_config());
        let fut = sme.start(&config);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Respond to the start request.
        let mut sme_fut = pin!(sme_fut.into_stream().into_future());
        assert_matches!(
            poll_ap_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ApSmeRequest::Start { responder, .. }) => {
                responder.send(fidl_sme::StartApResultCode::Success)
                    .expect("could not send sme response");
            }
        );

        // Verify that the start response was returned.
        assert_matches!(
            exec.run_until_stalled(&mut fut),
            Poll::Ready(Ok(fidl_sme::StartApResultCode::Success))
        );
    }

    #[fuchsia::test]
    fn state_machine_sme_fails_to_request_start_ap() {
        let mut exec = fasync::TestExecutor::new();

        // Build an SME wrapper.
        let (proxy, _) = create_proxy::<fidl_sme::ApSmeMarker>();
        let (defect_sender, _defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForApStateMachine::new(proxy, iface_id, defect_sender);

        // Start the AP and observe an immediate failure.
        let config = fidl_sme::ApConfig::from(create_ap_config());
        let fut = sme.start(&config);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn state_machine_sme_start_ap_timeout() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        // Build an SME wrapper.
        let (proxy, _sme_fut) = create_proxy::<fidl_sme::ApSmeMarker>();
        let (defect_sender, mut defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForApStateMachine::new(proxy, iface_id, defect_sender);

        // Start the AP and run the future until it stalls.
        let config = fidl_sme::ApConfig::from(create_ap_config());
        let fut = sme.start(&config);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Advance the clock beyond the timeout.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            START_AP_TIMEOUT + fasync::MonotonicDuration::from_seconds(1),
        ));

        // Verify that the future returns and that a defect is logged.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
        assert_eq!(
            defect_receiver.try_next().expect("missing connection timeout"),
            Some(Defect::Iface(IfaceFailure::Timeout { iface_id, source: TimeoutSource::ApStart })),
        );
    }

    #[fuchsia::test]
    fn state_machine_sme_stops_ap_successfully() {
        let mut exec = fasync::TestExecutor::new();

        // Build an SME wrapper.
        let (proxy, sme_fut) = create_proxy::<fidl_sme::ApSmeMarker>();
        let (defect_sender, _defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForApStateMachine::new(proxy, iface_id, defect_sender);

        // Stop the AP and run the future until it stalls.
        let fut = sme.stop();
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Respond to the stop request.
        let mut sme_fut = pin!(sme_fut.into_stream().into_future());
        assert_matches!(
            poll_ap_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ApSmeRequest::Stop { responder }) => {
                responder.send(fidl_sme::StopApResultCode::Success)
                    .expect("could not send sme response");
            }
        );

        // Verify that the start response was returned.
        assert_matches!(
            exec.run_until_stalled(&mut fut),
            Poll::Ready(Ok(fidl_sme::StopApResultCode::Success))
        );
    }

    #[fuchsia::test]
    fn state_machine_sme_fails_to_request_stop_ap() {
        let mut exec = fasync::TestExecutor::new();

        // Build an SME wrapper.
        let (proxy, _) = create_proxy::<fidl_sme::ApSmeMarker>();
        let (defect_sender, _defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForApStateMachine::new(proxy, iface_id, defect_sender);

        // Stop the AP and observe an immediate failure.
        let fut = sme.stop();
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn state_machine_sme_stop_ap_timeout() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        // Build an SME wrapper.
        let (proxy, _sme_fut) = create_proxy::<fidl_sme::ApSmeMarker>();
        let (defect_sender, mut defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForApStateMachine::new(proxy, iface_id, defect_sender);

        // Stop the AP and run the future until it stalls.
        let fut = sme.stop();
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Advance the clock beyond the timeout.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            START_AP_TIMEOUT + fasync::MonotonicDuration::from_seconds(1),
        ));

        // Verify that the future returns and that a defect is logged.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
        assert_eq!(
            defect_receiver.try_next().expect("missing connection timeout"),
            Some(Defect::Iface(IfaceFailure::Timeout { iface_id, source: TimeoutSource::ApStop })),
        );
    }

    #[fuchsia::test]
    fn state_machine_sme_successfully_queries_ap_status() {
        let mut exec = fasync::TestExecutor::new();

        // Build an SME wrapper.
        let (proxy, sme_fut) = create_proxy::<fidl_sme::ApSmeMarker>();
        let (defect_sender, _defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForApStateMachine::new(proxy, iface_id, defect_sender);

        // Query AP status and run the future until it stalls.
        let fut = sme.status();
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Respond to the status request.
        let mut sme_fut = pin!(sme_fut.into_stream().into_future());
        assert_matches!(
            poll_ap_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ApSmeRequest::Status{ responder }) => {
                let response = fidl_sme::ApStatusResponse { running_ap: None };
                responder.send(&response).expect("could not send AP status response");
            }
        );

        // Verify that the status response was returned.
        assert_matches!(
            exec.run_until_stalled(&mut fut),
            Poll::Ready(Ok(fidl_sme::ApStatusResponse { running_ap: None }))
        );
    }

    #[fuchsia::test]
    fn state_machine_sme_fails_to_query_ap_status() {
        let mut exec = fasync::TestExecutor::new();

        // Build an SME wrapper.
        let (proxy, _) = create_proxy::<fidl_sme::ApSmeMarker>();
        let (defect_sender, _defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForApStateMachine::new(proxy, iface_id, defect_sender);

        // Query status and observe an immediate failure.
        let fut = sme.status();
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn state_machine_sme_query_ap_status_timeout() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        // Build an SME wrapper.
        let (proxy, _sme_fut) = create_proxy::<fidl_sme::ApSmeMarker>();
        let (defect_sender, mut defect_receiver) = mpsc::channel(100);
        let iface_id = rand::random::<u16>();
        let sme = SmeForApStateMachine::new(proxy, iface_id, defect_sender);

        // Query AP status and run the future until it stalls.
        let fut = sme.status();
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Advance the clock beyond the timeout.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            AP_STATUS_TIMEOUT + fasync::MonotonicDuration::from_seconds(1),
        ));

        // Verify that the future returns and that a defect is logged.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
        assert_eq!(
            defect_receiver.try_next().expect("missing connection timeout"),
            Some(Defect::Iface(IfaceFailure::Timeout {
                iface_id,
                source: TimeoutSource::ApStatus,
            })),
        );
    }
}
