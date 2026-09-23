// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::{
    self as client_sme, ConnectResult, ConnectTransactionEvent, ConnectTransactionStream,
    RoamResult, ScheduledScanReceiver,
};
use crate::{MlmeEventStream, MlmeSink, MlmeStream};
use fidl::endpoints::{RequestStream, ServerEnd};
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_sme::{self as fidl_sme, ClientSmeRequest, TelemetryRequest};
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::prelude::*;
use futures::select;
use ieee80211::MacAddrBytes;
use log::error;
use std::pin::pin;
use std::sync::Arc;
use wlan_common::scan::write_vmo;

pub type Endpoint = ServerEnd<fidl_sme::ClientSmeMarker>;
type Sme = client_sme::ClientSme;

#[allow(clippy::too_many_arguments, reason = "mass allow for https://fxbug.dev/381896734")]
pub fn serve(
    cfg: crate::Config,
    device_info: fidl_mlme::DeviceInfo,
    security_support: fidl_common::SecuritySupport,
    spectrum_management_support: fidl_common::SpectrumManagementSupport,
    event_stream: MlmeEventStream,
    new_fidl_clients: mpsc::UnboundedReceiver<Endpoint>,
    new_telemetry_fidl_clients: mpsc::UnboundedReceiver<
        fidl::endpoints::ServerEnd<fidl_sme::TelemetryMarker>,
    >,
    inspector: fuchsia_inspect::Inspector,
    inspect_node: fuchsia_inspect::Node,
) -> (MlmeSink, MlmeStream, impl Future<Output = Result<(), anyhow::Error>>) {
    let wpa3_supported =
        security_support.mfp.as_ref().is_some_and(|mfp| mfp.supported.unwrap_or(false))
            && security_support.sae.as_ref().is_some_and(|sae| {
                sae.driver_handler_supported.unwrap_or(false)
                    || sae.sme_handler_supported.unwrap_or(false)
            });
    let owe_supported =
        security_support.mfp.as_ref().is_some_and(|mfp| mfp.supported.unwrap_or(false))
            && security_support.owe.as_ref().is_some_and(|owe| owe.supported.unwrap_or(false));
    let cfg = client_sme::ClientConfig::from_config(cfg, wpa3_supported, owe_supported);
    let (sme, mlme_sink, mlme_stream, time_stream) = Sme::new(
        cfg,
        device_info,
        inspector,
        inspect_node,
        security_support,
        spectrum_management_support,
    );
    let fut = async move {
        let sme = Arc::new(Mutex::new(sme));
        let mlme_sme = super::serve_mlme_sme(event_stream, Arc::clone(&sme), time_stream);
        let sme_fidl = super::serve_fidl(&*sme, new_fidl_clients, handle_fidl_request);
        let telemetry_fidl =
            super::serve_fidl(&*sme, new_telemetry_fidl_clients, handle_telemetry_fidl_request);
        let mlme_sme = pin!(mlme_sme);
        let sme_fidl = pin!(sme_fidl);
        select! {
            mlme_sme = mlme_sme.fuse() => mlme_sme?,
            sme_fidl = sme_fidl.fuse() => match sme_fidl? {},
            telemetry_fidl = telemetry_fidl.fuse() => match telemetry_fidl? {},
        }
        Ok(())
    };
    (mlme_sink, mlme_stream, fut)
}

async fn handle_fidl_request(
    sme: &Mutex<Sme>,
    request: fidl_sme::ClientSmeRequest,
) -> Result<(), fidl::Error> {
    #[allow(clippy::unit_arg, reason = "mass allow for https://fxbug.dev/381896734")]
    match request {
        ClientSmeRequest::Scan { req, responder } => Ok(scan(sme, req, |result| match result {
            Ok(scan_results) => responder.send(Ok(write_vmo(scan_results)?)).map_err(|e| e.into()),
            Err(e) => responder.send(Err(e)).map_err(|e| e.into()),
        })
        .await
        .unwrap_or_else(|e| error!("Error handling a scan transaction: {:?}", e))),
        ClientSmeRequest::Connect { req, txn, .. } => Ok(connect(sme, txn, req)
            .await
            .unwrap_or_else(|e| error!("Error handling a connect transaction: {:?}", e))),
        ClientSmeRequest::Roam { req, .. } => Ok(roam(sme, req)),
        ClientSmeRequest::Disconnect { responder, reason } => {
            disconnect(sme, reason, responder);
            Ok(())
        }
        ClientSmeRequest::Status { responder } => responder.send(&status(sme)),
        ClientSmeRequest::WmmStatus { responder } => wmm_status(sme, responder).await,
        ClientSmeRequest::StartScheduledScan { req, txn, responder } => {
            start_scheduled_scan(sme, req, txn, responder).await
        }

        ClientSmeRequest::GetScheduledScanEnabled { responder } => {
            let receiver = sme.lock().on_get_scheduled_scan_enabled_command();
            let resp = match receiver.await {
                Ok(result) => result,
                Err(_) => Err(zx::sys::ZX_ERR_CANCELED),
            };
            responder
                .send(resp.as_ref().map(|r| !r.active_txn_ids.is_empty()).map_err(|e| *e))
                .unwrap_or_else(|e| error!("Error sending response: {:?}", e));
            Ok(())
        }
        ClientSmeRequest::ScanForController { req, responder } => {
            Ok(scan(sme, req, |result| match result {
                Ok(results) => responder.send(Ok(&results[..])).map_err(|e| e.into()),
                Err(e) => responder.send(Err(e)).map_err(|e| e.into()),
            })
            .await
            .unwrap_or_else(|e| error!("Error handling a test scan transaction: {:?}", e)))
        }
        ClientSmeRequest::SetMacAddress { mac_addr, responder } => {
            Ok(set_mac_address(sme, mac_addr, responder).await?)
        }
        ClientSmeRequest::InstallApfPacketFilter { program, responder } => {
            let receiver = sme.lock().install_apf_packet_filter(program);
            let resp = match receiver.await {
                Ok(result) => result,
                Err(_) => Err(zx::sys::ZX_ERR_CANCELED),
            };
            responder.send(resp).unwrap_or_else(|e| error!("Error sending response: {:?}", e));
            Ok(())
        }
        ClientSmeRequest::ReadApfPacketFilterData { responder } => {
            let receiver = sme.lock().read_apf_packet_filter_data();
            let resp = match receiver.await {
                Ok(result) => result.map(|r| r.memory),
                Err(_) => Err(zx::sys::ZX_ERR_CANCELED),
            };
            responder
                .send(resp.as_ref().map(|m| &m[..]).map_err(|e| *e))
                .unwrap_or_else(|e| error!("Error sending response: {:?}", e));
            Ok(())
        }
        ClientSmeRequest::SetApfPacketFilterEnabled { enabled, responder } => {
            let receiver = sme.lock().set_apf_packet_filter_enabled(enabled);
            let resp = match receiver.await {
                Ok(result) => result,
                Err(_) => Err(zx::sys::ZX_ERR_CANCELED),
            };
            responder.send(resp).unwrap_or_else(|e| error!("Error sending response: {:?}", e));
            Ok(())
        }
        ClientSmeRequest::GetApfPacketFilterEnabled { responder } => {
            let receiver = sme.lock().get_apf_packet_filter_enabled();
            let resp = match receiver.await {
                Ok(result) => result.map(|r| r.enabled),
                Err(_) => Err(zx::sys::ZX_ERR_CANCELED),
            };
            responder
                .send(resp.as_ref().map(|e| *e).map_err(|e| *e))
                .unwrap_or_else(|e| error!("Error sending response: {:?}", e));
            Ok(())
        }
    }
}

async fn handle_telemetry_fidl_request(
    sme: &Mutex<Sme>,
    request: TelemetryRequest,
) -> Result<(), fidl::Error> {
    match request {
        TelemetryRequest::QueryTelemetrySupport { responder, .. } => {
            let support_fut = sme.lock().query_telemetry_support();
            let support = support_fut
                .await
                .map_err(|_| zx::Status::CONNECTION_ABORTED.into_raw())
                .and_then(|result| result);
            responder.send(support.as_ref().map_err(|e| *e))
        }
        TelemetryRequest::GetIfaceStats { responder, .. } => {
            let iface_stats_fut = sme.lock().iface_stats();
            let iface_stats = iface_stats_fut
                .await
                .map_err(|_| zx::Status::CONNECTION_ABORTED.into_raw())
                .and_then(|stats| match stats {
                    fidl_mlme::GetIfaceStatsResponse::Stats(stats) => Ok(stats),
                    fidl_mlme::GetIfaceStatsResponse::ErrorStatus(err) => Err(err),
                });
            responder.send(iface_stats.as_ref().map_err(|e| *e))
        }
        TelemetryRequest::GetHistogramStats { responder, .. } => {
            let histogram_stats_fut = sme.lock().histogram_stats();
            let histogram_stats = histogram_stats_fut
                .await
                .map_err(|_| zx::Status::CONNECTION_ABORTED.into_raw())
                .and_then(|stats| match stats {
                    fidl_mlme::GetIfaceHistogramStatsResponse::Stats(stats) => Ok(stats),
                    fidl_mlme::GetIfaceHistogramStatsResponse::ErrorStatus(err) => Err(err),
                });
            responder.send(histogram_stats.as_ref().map_err(|e| *e))
        }
        TelemetryRequest::GetSignalReport { responder, .. } => {
            let signal_report_fut = sme.lock().signal_report();
            let signal_report = signal_report_fut
                .await
                .map_err(|_| zx::Status::CONNECTION_ABORTED.into_raw())
                .and_then(|result| result);
            responder.send(signal_report.as_ref().map_err(|e| *e))
        }
        TelemetryRequest::CloneInspectVmo { responder } => {
            let inspect_vmo =
                sme.lock().on_clone_inspect_vmo().ok_or_else(|| zx::Status::INTERNAL.into_raw());
            responder.send(inspect_vmo)
        }
    }
}

async fn scan(
    sme: &Mutex<Sme>,
    request: fidl_sme::ScanRequest,
    responder: impl FnOnce(
        Result<Vec<fidl_sme::ScanResult>, fidl_sme::ScanErrorCode>,
    ) -> Result<(), anyhow::Error>,
) -> Result<(), anyhow::Error> {
    let receiver = sme.lock().on_scan_command(request);
    let receive_result = match receiver.await {
        Ok(receive_result) => receive_result,
        Err(e) => {
            error!("Scan receiver error: {:?}", e);
            responder(Err(fidl_sme::ScanErrorCode::InternalError))?;
            return Ok(());
        }
    };

    match receive_result {
        Ok(scan_results) => {
            let results = scan_results.into_iter().map(Into::into).collect::<Vec<_>>();
            responder(Ok(results))
        }
        Err(mlme_scan_result_code) => {
            let scan_error_code = match mlme_scan_result_code {
                fidl_mlme::ScanResultCode::Success | fidl_mlme::ScanResultCode::InvalidArgs => {
                    error!("Internal scan error: {:?}", mlme_scan_result_code);
                    fidl_sme::ScanErrorCode::InternalError
                }
                fidl_mlme::ScanResultCode::NotSupported => fidl_sme::ScanErrorCode::NotSupported,
                fidl_mlme::ScanResultCode::InternalError => {
                    fidl_sme::ScanErrorCode::InternalMlmeError
                }
                fidl_mlme::ScanResultCode::ShouldWait => fidl_sme::ScanErrorCode::ShouldWait,
                fidl_mlme::ScanResultCode::CanceledByDriverOrFirmware => {
                    fidl_sme::ScanErrorCode::CanceledByDriverOrFirmware
                }
            };
            responder(Err(scan_error_code))
        }
    }?;
    Ok(())
}

async fn connect(
    sme: &Mutex<Sme>,
    txn: Option<ServerEnd<fidl_sme::ConnectTransactionMarker>>,
    req: fidl_sme::ConnectRequest,
) -> Result<(), anyhow::Error> {
    #[allow(clippy::manual_map, reason = "mass allow for https://fxbug.dev/381896734")]
    let handle = match txn {
        None => None,
        Some(txn) => Some(txn.into_stream().control_handle()),
    };
    let connect_txn_stream = sme.lock().on_connect_command(req);
    serve_connect_txn_stream(handle, connect_txn_stream).await?;
    Ok(())
}

/// Serves a scheduled scan session by processing a stream for events sent by MLME and processed in
/// SME's scan scheduler, and a stream for client-initiated cancellations.
async fn serve_sched_scan_session(
    txn_handle: fidl_sme::ScheduledScanTransactionControlHandle,
    txn_stream: fidl_sme::ScheduledScanTransactionRequestStream,
    session: ScheduledScanReceiver,
) -> Result<(), anyhow::Error> {
    // Client will close the transaction channel to cancel session.
    let mut cancellation_stream = txn_stream.fuse();
    let mut session_stream = session.fuse();
    loop {
        futures::select! {
            scan_results = session_stream.next() => {
                if let Some(vmo) = scan_results {
                    txn_handle.send_on_scheduled_scan_matches_available(vmo)?;
                } else {
                    // Stream closed naturally (SME dropped sender because firmware stopped it).
                    txn_handle.shutdown_with_epitaph(Ok(()));
                    break;
                }
            },
            _ = cancellation_stream.next() => {
                // Client dropped transaction stream. Cleanup is handled by ScheduledScanReceiver's
                // Drop implementation.
                break;
            }
        }
    }
    Ok(())
}

/// Creates a new scheduled scan session and starts serving it.
async fn start_scheduled_scan(
    sme: &Mutex<Sme>,
    req: fidl_common::ScheduledScanRequest,
    txn_server: ServerEnd<fidl_sme::ScheduledScanTransactionMarker>,
    responder: fidl_sme::ClientSmeStartScheduledScanResponder,
) -> Result<(), fidl::Error> {
    let txn_receiver_stream = txn_server.into_stream();
    let txn_sender_handle = txn_receiver_stream.control_handle();
    let (receiver, session) = sme.lock().on_start_scheduled_scan_command(req);
    match receiver.await.unwrap_or_else(|_| Err(zx::Status::CANCELED.into_raw())) {
        Ok(()) => {
            responder.send(Ok(()))?;
            serve_sched_scan_session(txn_sender_handle, txn_receiver_stream, session)
                .await
                .unwrap_or_else(|e| {
                    error!("Error serving sched scan txn stream: {:?}", e);
                });
        }
        Err(status) => {
            responder.send(Err(status))?;
            txn_sender_handle.shutdown_with_epitaph(zx::Status::err_from_raw(status));
        }
    }
    Ok(())
}

async fn serve_connect_txn_stream(
    handle: Option<fidl_sme::ConnectTransactionControlHandle>,
    mut connect_txn_stream: ConnectTransactionStream,
) -> Result<(), anyhow::Error> {
    if let Some(handle) = handle {
        loop {
            match connect_txn_stream.next().await {
                Some(event) => match event {
                    ConnectTransactionEvent::OnConnectResult { result, is_reconnect } => {
                        let connect_result = convert_connect_result(&result, is_reconnect);
                        handle.send_on_connect_result(&connect_result)
                    }
                    ConnectTransactionEvent::OnRoamResult { result } => {
                        let roam_result = convert_roam_result(&result);
                        handle.send_on_roam_result(&roam_result)
                    }
                    ConnectTransactionEvent::OnDisconnect { info } => {
                        handle.send_on_disconnect(&info)
                    }
                    ConnectTransactionEvent::OnSignalReport { ind } => {
                        handle.send_on_signal_report(&ind)
                    }
                    ConnectTransactionEvent::OnChannelSwitched { info } => {
                        handle.send_on_channel_switched(&info)
                    }
                }?,
                // SME has dropped the ConnectTransaction endpoint, likely due to a disconnect.
                None => return Ok(()),
            }
        }
    }
    Ok(())
}

fn roam(sme: &Mutex<Sme>, req: fidl_sme::RoamRequest) {
    sme.lock().on_roam_command(req);
}

fn disconnect(
    sme: &Mutex<Sme>,
    policy_disconnect_reason: fidl_sme::UserDisconnectReason,
    responder: fidl_sme::ClientSmeDisconnectResponder,
) {
    sme.lock().on_disconnect_command(policy_disconnect_reason, responder);
}

fn status(sme: &Mutex<Sme>) -> fidl_sme::ClientStatusResponse {
    sme.lock().status().into()
}

async fn wmm_status(
    sme: &Mutex<Sme>,
    responder: fidl_sme::ClientSmeWmmStatusResponder,
) -> Result<(), fidl::Error> {
    let receiver = sme.lock().wmm_status();
    let wmm_status = match receiver.await {
        Ok(result) => result,
        Err(_) => Err(zx::sys::ZX_ERR_CANCELED),
    };
    responder.send(wmm_status.as_ref().map_err(|e| *e))
}

async fn set_mac_address(
    sme: &Mutex<Sme>,
    mac_addr: [u8; 6],
    responder: fidl_sme::ClientSmeSetMacAddressResponder,
) -> Result<(), fidl::Error> {
    let receiver = sme.lock().set_mac_address(mac_addr);
    let resp = match receiver.await {
        Ok(Ok(())) => {
            sme.lock().update_mac_address(mac_addr);
            Ok(())
        }
        Ok(result @ Err(_)) => result,
        Err(_) => Err(zx::sys::ZX_ERR_CANCELED),
    };
    responder.send(resp)
}

fn convert_connect_result(result: &ConnectResult, is_reconnect: bool) -> fidl_sme::ConnectResult {
    let (code, is_credential_rejected) = match result {
        ConnectResult::Success => (fidl_ieee80211::StatusCode::Success, false),
        ConnectResult::Canceled => (fidl_ieee80211::StatusCode::Canceled, false),
        ConnectResult::Failed(failure) => {
            (failure.status_code(), failure.likely_due_to_credential_rejected())
        }
    };
    fidl_sme::ConnectResult { code, is_credential_rejected, is_reconnect }
}

fn convert_roam_result(result: &RoamResult) -> fidl_sme::RoamResult {
    match result {
        RoamResult::Success(bss) => {
            let bss_description =
                Some(Box::new(fidl_ieee80211::BssDescription::from(*bss.clone())));
            fidl_sme::RoamResult {
                bssid: bss.bssid.to_array(),
                status_code: fidl_ieee80211::StatusCode::Success,
                // Must always be false on roam success.
                original_association_maintained: false,
                bss_description,
                disconnect_info: None,
                is_credential_rejected: false,
            }
        }
        RoamResult::Failed(failure) => {
            #[allow(clippy::manual_map, reason = "mass allow for https://fxbug.dev/381896734")]
            fidl_sme::RoamResult {
                bssid: failure.selected_bssid.to_array(),
                status_code: failure.status_code,
                // Current implementation assumes that all roam attempts incur disassociation from the
                // original BSS. When this changes (e.g. due to Fast BSS Transition support), this
                // hard-coded field should be set from the RoamResult enum.
                original_association_maintained: false,
                bss_description: match &failure.selected_bss {
                    Some(bss) => Some(Box::new(bss.clone().into())),
                    None => None,
                },
                disconnect_info: Some(Box::new(failure.disconnect_info)),
                is_credential_rejected: failure.likely_due_to_credential_rejected(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{ConnectFailure, EstablishRsnaFailure, EstablishRsnaFailureReason};
    use crate::test_utils;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_wlan_internal as fidl_internal;
    use fidl_fuchsia_wlan_mlme::ScanResultCode;
    use fidl_fuchsia_wlan_sme::{self as fidl_sme};
    use fuchsia_async as fasync;
    use futures::stream::StreamFuture;
    use futures::task::Poll;
    use rand::RngExt as _;
    use rand::prelude::ThreadRng;
    use std::pin::pin;
    use test_case::test_case;
    use wlan_common::random_bss_description;
    use wlan_common::scan::{self, Incompatible};
    use wlan_rsn::auth;

    #[test]
    fn test_convert_connect_result() {
        assert_eq!(
            convert_connect_result(&ConnectResult::Success, false),
            fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::Success,
                is_credential_rejected: false,
                is_reconnect: false,
            }
        );
        assert_eq!(
            convert_connect_result(&ConnectResult::Canceled, true),
            fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::Canceled,
                is_credential_rejected: false,
                is_reconnect: true,
            }
        );
        let connect_result =
            ConnectResult::Failed(ConnectFailure::ScanFailure(ScanResultCode::ShouldWait));
        assert_eq!(
            convert_connect_result(&connect_result, false),
            fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::Canceled,
                is_credential_rejected: false,
                is_reconnect: false,
            }
        );

        let connect_result =
            ConnectResult::Failed(ConnectFailure::EstablishRsnaFailure(EstablishRsnaFailure {
                auth_method: Some(auth::MethodName::Psk),
                reason: EstablishRsnaFailureReason::InternalError,
            }));
        assert_eq!(
            convert_connect_result(&connect_result, false),
            fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::EstablishRsnaFailure,
                is_credential_rejected: false,
                is_reconnect: false,
            }
        );

        let connect_result =
            ConnectResult::Failed(ConnectFailure::EstablishRsnaFailure(EstablishRsnaFailure {
                auth_method: Some(auth::MethodName::Psk),
                reason: EstablishRsnaFailureReason::RsnaResponseTimeout(
                    wlan_rsn::Error::LikelyWrongCredential,
                ),
            }));
        assert_eq!(
            convert_connect_result(&connect_result, false),
            fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::EstablishRsnaFailure,
                is_credential_rejected: true,
                is_reconnect: false,
            }
        );

        let connect_result =
            ConnectResult::Failed(ConnectFailure::EstablishRsnaFailure(EstablishRsnaFailure {
                auth_method: Some(auth::MethodName::Psk),
                reason: EstablishRsnaFailureReason::RsnaCompletionTimeout(
                    wlan_rsn::Error::LikelyWrongCredential,
                ),
            }));
        assert_eq!(
            convert_connect_result(&connect_result, false),
            fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::EstablishRsnaFailure,
                is_credential_rejected: true,
                is_reconnect: false,
            }
        );

        let connect_result =
            ConnectResult::Failed(ConnectFailure::EstablishRsnaFailure(EstablishRsnaFailure {
                auth_method: Some(auth::MethodName::Psk),
                reason: EstablishRsnaFailureReason::RsnaCompletionTimeout(
                    wlan_rsn::Error::MissingGtkProvider,
                ),
            }));
        assert_eq!(
            convert_connect_result(&connect_result, false),
            fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::EstablishRsnaFailure,
                is_credential_rejected: false,
                is_reconnect: false,
            }
        );

        let connect_result =
            ConnectResult::Failed(ConnectFailure::ScanFailure(ScanResultCode::InternalError));
        assert_eq!(
            convert_connect_result(&connect_result, false),
            fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                is_credential_rejected: false,
                is_reconnect: false,
            }
        );
    }

    // TODO(https://fxbug.dev/42164611): There is no test coverage for consistency between MLME scan results
    // and SME scan results produced by wlanstack. In particular, the timestamp_nanos field
    // of fidl_mlme::ScanResult is dropped in SME, and no tests reveal this problem.

    #[test_case(1, true; "with 1 result")]
    #[test_case(2, true; "with 2 results")]
    #[test_case(30, true; "with 30 results")]
    #[test_case(4000, true; "with 4000 results")]
    #[test_case(50000, false; "with 50000 results")]
    #[test_case(100000, false; "with 100000 results")]
    fn scan_results_are_effectively_unbounded(number_of_scan_results: usize, randomize: bool) {
        let mut exec = fasync::TestExecutor::new();
        let (client_sme_proxy, mut client_sme_stream) =
            create_proxy_and_stream::<fidl_sme::ClientSmeMarker>();

        // Request scan
        async fn request_and_collect_result(
            client_sme_proxy: &fidl_sme::ClientSmeProxy,
        ) -> fidl_sme::ClientSmeScanResult {
            client_sme_proxy
                .scan(&fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest {
                    channels: vec![],
                }))
                .await
                .expect("FIDL request failed")
        }

        let result_fut = request_and_collect_result(&client_sme_proxy);
        let mut result_fut = pin!(result_fut);

        assert_matches!(exec.run_until_stalled(&mut result_fut), Poll::Pending);

        // Generate and send scan results
        let mut rng = rand::rng();
        let scan_result_list = if randomize {
            (0..number_of_scan_results).map(|_| random_scan_result(&mut rng).into()).collect()
        } else {
            vec![random_scan_result(&mut rng).into(); number_of_scan_results]
        };
        assert_matches!(exec.run_until_stalled(&mut client_sme_stream.next()),
                        Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan {
                            req: _, responder,
                        }))) => {
                            let vmo = write_vmo(scan_result_list.clone()).expect("failed to write VMO");
                            responder.send(Ok(vmo)).expect("failed to send scan results");
                        }
        );

        // Verify scan results
        assert_matches!(exec.run_until_stalled(&mut result_fut), Poll::Ready(Ok(vmo)) => {
            assert_eq!(scan_result_list, scan::read_vmo(vmo).expect("failed to read VMO"));
        })
    }

    #[test]
    fn test_serve_connect_txn_stream() {
        let mut exec = fasync::TestExecutor::new();

        let (sme_proxy, sme_connect_txn_stream) = mpsc::unbounded();
        let (fidl_client_proxy, fidl_connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let fidl_client_fut = fidl_client_proxy.take_event_stream().into_future();
        let mut fidl_client_fut = pin!(fidl_client_fut);
        let fidl_connect_txn_handle = fidl_connect_txn_stream.control_handle();

        let test_fut =
            serve_connect_txn_stream(Some(fidl_connect_txn_handle), sme_connect_txn_stream);
        let mut test_fut = pin!(test_fut);

        // Test sending OnConnectResult
        sme_proxy
            .unbounded_send(ConnectTransactionEvent::OnConnectResult {
                result: ConnectResult::Success,
                is_reconnect: true,
            })
            .expect("expect sending ConnectTransactionEvent to succeed");
        assert_matches!(exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let event = assert_matches!(poll_stream_fut(&mut exec, &mut fidl_client_fut), Poll::Ready(Some(Ok(event))) => event);
        assert_matches!(
            event,
            fidl_sme::ConnectTransactionEvent::OnConnectResult {
                result: fidl_sme::ConnectResult {
                    code: fidl_ieee80211::StatusCode::Success,
                    is_credential_rejected: false,
                    is_reconnect: true,
                }
            }
        );

        // Test sending OnDisconnect
        let input_info = fidl_sme::DisconnectInfo {
            is_sme_reconnecting: true,
            disconnect_source: fidl_sme::DisconnectSource::Mlme(fidl_sme::DisconnectCause {
                reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason,
                mlme_event_name: fidl_sme::DisconnectMlmeEventName::DeauthenticateIndication,
            }),
        };
        sme_proxy
            .unbounded_send(ConnectTransactionEvent::OnDisconnect { info: input_info })
            .expect("expect sending ConnectTransactionEvent to succeed");
        assert_matches!(exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let event = assert_matches!(poll_stream_fut(&mut exec, &mut fidl_client_fut), Poll::Ready(Some(Ok(event))) => event);
        assert_matches!(event, fidl_sme::ConnectTransactionEvent::OnDisconnect { info: output_info } => {
            assert_eq!(input_info, output_info);
        });

        // Test sending OnSignalReport
        let input_ind =
            fidl_internal::SignalReportIndication { rssi_dbm: -40, snr_db: 30, tx_rate_500kbps: 0 };
        sme_proxy
            .unbounded_send(ConnectTransactionEvent::OnSignalReport { ind: input_ind })
            .expect("expect sending ConnectTransactionEvent to succeed");
        assert_matches!(exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let event = assert_matches!(poll_stream_fut(&mut exec, &mut fidl_client_fut), Poll::Ready(Some(Ok(event))) => event);
        assert_matches!(event, fidl_sme::ConnectTransactionEvent::OnSignalReport { ind } => {
            assert_eq!(input_ind, ind);
        });

        // Test sending OnChannelSwitched
        let input_info = fidl_internal::ChannelSwitchInfo {
            new_primary_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 8,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 0,
            },
        };
        sme_proxy
            .unbounded_send(ConnectTransactionEvent::OnChannelSwitched { info: input_info.clone() })
            .expect("expect sending ConnectTransactionEvent to succeed");
        assert_matches!(exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let event = assert_matches!(poll_stream_fut(&mut exec, &mut fidl_client_fut), Poll::Ready(Some(Ok(event))) => event);
        assert_matches!(event, fidl_sme::ConnectTransactionEvent::OnChannelSwitched { info } => {
            assert_eq!(input_info, info);
        });

        // When SME proxy is dropped, the fut should terminate
        std::mem::drop(sme_proxy);
        assert_matches!(exec.run_until_stalled(&mut test_fut), Poll::Ready(Ok(())));
    }

    fn poll_stream_fut<S: Stream + std::marker::Unpin>(
        exec: &mut fasync::TestExecutor,
        stream_fut: &mut StreamFuture<S>,
    ) -> Poll<Option<S::Item>> {
        exec.run_until_stalled(stream_fut).map(|(item, stream)| {
            *stream_fut = stream.into_future();
            item
        })
    }

    // Create roughly over 2k bytes ScanResult
    fn random_scan_result(rng: &mut ThreadRng) -> wlan_common::scan::ScanResult {
        use wlan_common::security::SecurityDescriptor;

        // TODO(https://fxbug.dev/42164451): Merge this with a similar function in wlancfg.
        wlan_common::scan::ScanResult {
            compatibility: match rng.random_range(0..4) {
                0 => wlan_common::scan::Compatible::expect_ok([SecurityDescriptor::OPEN]),
                1 => wlan_common::scan::Compatible::expect_ok([SecurityDescriptor::WPA2_PERSONAL]),
                2 => wlan_common::scan::Compatible::expect_ok([
                    SecurityDescriptor::WPA2_PERSONAL,
                    SecurityDescriptor::WPA3_PERSONAL,
                ]),
                _ => Incompatible::unknown(),
            },
            timestamp: zx::MonotonicInstant::from_nanos(rng.random()),
            bss_description: random_bss_description!(),
        }
    }

    #[test]
    fn test_handle_fidl_request_apf() {
        let mut exec = fasync::TestExecutor::new();
        let inspector = fuchsia_inspect::Inspector::default();
        let (sme, _mlme_sink, mut mlme_stream, _time_stream) = client_sme::ClientSme::new(
            client_sme::ClientConfig::default(),
            test_utils::fake_device_info([0; 6].into()),
            inspector.clone(),
            inspector.root().create_child("sme"),
            wlan_common::test_utils::fake_features::fake_security_support(),
            wlan_common::test_utils::fake_features::fake_spectrum_management_support_empty(),
        );
        let sme = Mutex::new(sme);

        // Test InstallApfPacketFilter
        let (proxy, stream) = create_proxy_and_stream::<fidl_sme::ClientSmeMarker>();
        let program = vec![1, 2, 3];
        let mut install_fut = proxy.install_apf_packet_filter(&program);
        let mut stream = pin!(stream);

        // Run handle_fidl_request for the install request
        assert_matches!(exec.run_until_stalled(&mut stream.next()), Poll::Ready(Some(Ok(req))) => {
            let mut handle_fut = pin!(handle_fidl_request(&sme, req));
            assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Pending);

            // Forward from SME to MLME
            assert_matches!(exec.run_until_stalled(&mut mlme_stream.next()), Poll::Ready(Some(crate::MlmeRequest::InstallApfPacketFilter(req, responder))) => {
                assert_eq!(req.program, program);
                responder.respond(Ok(()));
            });

            assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Ready(Ok(())));
        });
        assert_matches!(exec.run_until_stalled(&mut install_fut), Poll::Ready(Ok(Ok(()))));

        // Test ReadApfPacketFilterData
        let mut read_fut = proxy.read_apf_packet_filter_data();
        assert_matches!(exec.run_until_stalled(&mut stream.next()), Poll::Ready(Some(Ok(req))) => {
            let mut handle_fut = pin!(handle_fidl_request(&sme, req));
            assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Pending);

            // Forward from SME to MLME
            assert_matches!(exec.run_until_stalled(&mut mlme_stream.next()), Poll::Ready(Some(crate::MlmeRequest::ReadApfPacketFilterData(responder))) => {
                responder.respond(Ok(fidl_mlme::MlmeReadApfPacketFilterDataResponse {
                    memory: vec![4, 5, 6],
                }));
            });

            assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Ready(Ok(())));
        });
        assert_matches!(exec.run_until_stalled(&mut read_fut), Poll::Ready(Ok(Ok(data))) => {
            assert_eq!(data, vec![4, 5, 6]);
        });

        // Test SetApfPacketFilterEnabled
        let mut set_enabled_fut = proxy.set_apf_packet_filter_enabled(true);
        assert_matches!(exec.run_until_stalled(&mut stream.next()), Poll::Ready(Some(Ok(req))) => {
            let mut handle_fut = pin!(handle_fidl_request(&sme, req));
            assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Pending);

            // Forward from SME to MLME
            assert_matches!(exec.run_until_stalled(&mut mlme_stream.next()), Poll::Ready(Some(crate::MlmeRequest::SetApfPacketFilterEnabled(req, responder))) => {
                assert!(req.enabled);
                responder.respond(Ok(()));
            });

            assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Ready(Ok(())));
        });
        assert_matches!(exec.run_until_stalled(&mut set_enabled_fut), Poll::Ready(Ok(Ok(()))));

        // Test GetApfPacketFilterEnabled
        let mut get_enabled_fut = proxy.get_apf_packet_filter_enabled();
        assert_matches!(exec.run_until_stalled(&mut stream.next()), Poll::Ready(Some(Ok(req))) => {
            let mut handle_fut = pin!(handle_fidl_request(&sme, req));
            assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Pending);

            // Forward from SME to MLME
            assert_matches!(exec.run_until_stalled(&mut mlme_stream.next()), Poll::Ready(Some(crate::MlmeRequest::GetApfPacketFilterEnabled(responder))) => {
                responder.respond(Ok(fidl_mlme::MlmeGetApfPacketFilterEnabledResponse {
                    enabled: true,
                }));
            });

            assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Ready(Ok(())));
        });
        assert_matches!(exec.run_until_stalled(&mut get_enabled_fut), Poll::Ready(Ok(Ok(true))));
    }

    #[test_case(Ok(()); "ok")]
    #[test_case(Err(123); "err")]
    fn test_handle_fidl_request_set_mac_address(set_mac_address_result: Result<(), i32>) {
        let mut exec = fasync::TestExecutor::new();
        let inspector = fuchsia_inspect::Inspector::default();
        let (sme, _mlme_sink, mut mlme_stream, _time_stream) = client_sme::ClientSme::new(
            client_sme::ClientConfig::default(),
            test_utils::fake_device_info([0; 6].into()),
            inspector.clone(),
            inspector.root().create_child("sme"),
            wlan_common::test_utils::fake_features::fake_security_support(),
            wlan_common::test_utils::fake_features::fake_spectrum_management_support_empty(),
        );
        let sme = Mutex::new(sme);

        // Send SetMacAddress request
        let (proxy, stream) = create_proxy_and_stream::<fidl_sme::ClientSmeMarker>();
        let new_mac = [1, 2, 3, 4, 5, 6];
        let mut set_mac_fut = proxy.set_mac_address(&new_mac);
        let mut stream = pin!(stream);

        // Handle SetMacAddress request
        assert_matches!(exec.run_until_stalled(&mut stream.next()), Poll::Ready(Some(Ok(req))) => {
            let mut handle_fut = pin!(handle_fidl_request(&sme, req));
            assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Pending);

            assert_matches!(exec.run_until_stalled(&mut mlme_stream.next()), Poll::Ready(Some(crate::MlmeRequest::SetMacAddress(mac, responder))) => {
                assert_eq!(mac, new_mac);
                responder.respond(set_mac_address_result);
            });

            assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Ready(Ok(())));
        });

        if set_mac_address_result.is_ok() {
            assert_matches!(exec.run_until_stalled(&mut set_mac_fut), Poll::Ready(Ok(Ok(()))));
            // Verify the device_info in client_sme is updated
            assert_eq!(sme.lock().device_info().sta_addr, new_mac);
        } else {
            assert_matches!(exec.run_until_stalled(&mut set_mac_fut), Poll::Ready(Ok(Err(123))));
            assert_eq!(sme.lock().device_info().sta_addr, [0; 6]);
        }
    }

    #[test]
    fn test_handle_fidl_request_set_mac_address_cancelled() {
        let mut exec = fasync::TestExecutor::new();
        let inspector = fuchsia_inspect::Inspector::default();
        let (sme, _mlme_sink, mut mlme_stream, _time_stream) = client_sme::ClientSme::new(
            client_sme::ClientConfig::default(),
            test_utils::fake_device_info([0; 6].into()),
            inspector.clone(),
            inspector.root().create_child("sme"),
            wlan_common::test_utils::fake_features::fake_security_support(),
            wlan_common::test_utils::fake_features::fake_spectrum_management_support_empty(),
        );
        let sme = Mutex::new(sme);

        // Send SetMacAddress request
        let (proxy, stream) = create_proxy_and_stream::<fidl_sme::ClientSmeMarker>();
        let new_mac = [1, 2, 3, 4, 5, 6];
        let mut set_mac_fut = proxy.set_mac_address(&new_mac);
        let mut stream = pin!(stream);

        // Handle SetMacAddress request
        assert_matches!(exec.run_until_stalled(&mut stream.next()), Poll::Ready(Some(Ok(req))) => {
            let mut handle_fut = pin!(handle_fidl_request(&sme, req));
            assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Pending);

            assert_matches!(exec.run_until_stalled(&mut mlme_stream.next()), Poll::Ready(Some(crate::MlmeRequest::SetMacAddress(_mac, _responder))) => {
                // Drop the responder to simulate an error
            });

            assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Ready(Ok(())));
        });

        assert_matches!(
            exec.run_until_stalled(&mut set_mac_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_CANCELED)))
        );
        assert_eq!(sme.lock().device_info().sta_addr, [0; 6]);
    }
}
