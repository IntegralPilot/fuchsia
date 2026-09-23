// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::convert::fullmac_to_mlme;
use crate::{FullmacDriverEvent, FullmacDriverEventSink};
use anyhow::Context;
use fidl_fuchsia_wlan_fullmac as fidl_fullmac;
use futures::StreamExt;
use log::error;

pub async fn serve_wlan_fullmac_impl_ifc_request_handler(
    mut fullmac_ifc_request_stream: fidl_fullmac::WlanFullmacImplIfcRequestStream,
    driver_event_sink: FullmacDriverEventSink,
) {
    while let Some(Ok(req)) = fullmac_ifc_request_stream.next().await {
        if let Err(e) = handle_one_request(req, &driver_event_sink) {
            error!("Failed to handle driver event: {}", e);
        }
    }
}

fn handle_one_request(
    req: fidl_fullmac::WlanFullmacImplIfcRequest,
    driver_event_sink: &FullmacDriverEventSink,
) -> anyhow::Result<()> {
    match req {
        fidl_fullmac::WlanFullmacImplIfcRequest::OnScanResult { payload, responder } => {
            responder.send().context("Failed to respond to OnScanResult")?;
            driver_event_sink.0.send(FullmacDriverEvent::OnScanResult {
                result: fullmac_to_mlme::convert_scan_result(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::OnScanEnd { payload, responder } => {
            responder.send().context("Failed to respond to OnScanEnd")?;
            driver_event_sink.0.send(FullmacDriverEvent::OnScanEnd {
                end: fullmac_to_mlme::convert_scan_end(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::OnScheduledScanMatchesAvailable {
            payload,
            ..
        } => {
            driver_event_sink.0.send(FullmacDriverEvent::OnScheduledScanMatchesAvailable {
                txn_id: payload.txn_id.context("missing txn_id")?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::OnScheduledScanStoppedByFirmware {
            payload,
            ..
        } => {
            driver_event_sink.0.send(FullmacDriverEvent::OnScheduledScanStoppedByFirmware {
                txn_id: payload.txn_id.context("missing txn_id")?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::ConnectConf { payload, responder } => {
            responder.send().context("Failed to respond to ConnectConf")?;
            driver_event_sink.0.send(FullmacDriverEvent::ConnectConf {
                resp: fullmac_to_mlme::convert_connect_confirm(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::RoamConf { payload, responder } => {
            responder.send().context("Failed to respond to RoamConf")?;
            driver_event_sink.0.send(FullmacDriverEvent::RoamConf {
                conf: fullmac_to_mlme::convert_roam_confirm(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::RoamStartInd { payload, responder } => {
            responder.send().context("Failed to respond to RoamStartInd")?;
            driver_event_sink.0.send(FullmacDriverEvent::RoamStartInd {
                ind: fullmac_to_mlme::convert_roam_start_indication(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::RoamResultInd { payload, responder } => {
            responder.send().context("Failed to respond to RoamResultInd")?;
            driver_event_sink.0.send(FullmacDriverEvent::RoamResultInd {
                ind: fullmac_to_mlme::convert_roam_result_indication(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::AuthInd { payload, responder } => {
            responder.send().context("Failed to respond to AuthInd")?;
            driver_event_sink.0.send(FullmacDriverEvent::AuthInd {
                ind: fullmac_to_mlme::convert_authenticate_indication(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::DeauthConf { payload, responder } => {
            responder.send().context("Failed to respond to DeauthConf")?;
            driver_event_sink.0.send(FullmacDriverEvent::DeauthConf {
                resp: fullmac_to_mlme::convert_deauthenticate_confirm(payload),
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::DeauthInd { payload, responder } => {
            responder.send().context("Failed to respond to DeauthInd")?;
            driver_event_sink.0.send(FullmacDriverEvent::DeauthInd {
                ind: fullmac_to_mlme::convert_deauthenticate_indication(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::AssocInd { payload, responder } => {
            responder.send().context("Failed to respond to AssocInd")?;
            driver_event_sink.0.send(FullmacDriverEvent::AssocInd {
                ind: fullmac_to_mlme::convert_associate_indication(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::DisassocConf { payload, responder } => {
            responder.send().context("Failed to respond to DisassocConf")?;
            driver_event_sink.0.send(FullmacDriverEvent::DisassocConf {
                resp: fullmac_to_mlme::convert_disassociate_confirm(payload),
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::DisassocInd { payload, responder } => {
            responder.send().context("Failed to respond to DisassocInd")?;
            driver_event_sink.0.send(FullmacDriverEvent::DisassocInd {
                ind: fullmac_to_mlme::convert_disassociate_indication(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::StartConf { payload, responder } => {
            responder.send().context("Failed to respond to StartConf")?;
            driver_event_sink.0.send(FullmacDriverEvent::StartConf {
                resp: fullmac_to_mlme::convert_start_confirm(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::StopConf { payload, responder } => {
            responder.send().context("Failed to respond to StopConf")?;
            driver_event_sink.0.send(FullmacDriverEvent::StopConf {
                resp: fullmac_to_mlme::convert_stop_confirm(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::EapolConf { payload, responder } => {
            responder.send().context("Failed to respond to EapolConf")?;
            driver_event_sink.0.send(FullmacDriverEvent::EapolConf {
                resp: fullmac_to_mlme::convert_eapol_confirm(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::OnChannelSwitch { ind, responder } => {
            responder.send().context("Failed to respond to OnChannelSwitch")?;
            driver_event_sink.0.send(FullmacDriverEvent::OnChannelSwitch {
                resp: fullmac_to_mlme::convert_channel_switch_info(ind),
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::SignalReport { ind, responder } => {
            responder.send().context("Failed to respond to SignalReport")?;
            driver_event_sink.0.send(FullmacDriverEvent::SignalReport {
                ind: fullmac_to_mlme::convert_signal_report_indication(ind),
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::EapolInd { payload, responder } => {
            responder.send().context("Failed to respond to EapolInd")?;
            driver_event_sink.0.send(FullmacDriverEvent::EapolInd {
                ind: fullmac_to_mlme::convert_eapol_indication(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::OnPmkAvailable { payload, responder } => {
            responder.send().context("Failed to respond to OnPmkAvailable")?;
            driver_event_sink.0.send(FullmacDriverEvent::OnPmkAvailable {
                info: fullmac_to_mlme::convert_pmk_info(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::SaeHandshakeInd { payload, responder } => {
            responder.send().context("Failed to respond to SaeHandshakeInd")?;
            driver_event_sink.0.send(FullmacDriverEvent::SaeHandshakeInd {
                ind: fullmac_to_mlme::convert_sae_handshake_indication(payload)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::SaeFrameRx { frame, responder } => {
            responder.send().context("Failed to respond to SaeFrameRx")?;
            driver_event_sink.0.send(FullmacDriverEvent::SaeFrameRx {
                frame: fullmac_to_mlme::convert_sae_frame(frame)?,
            });
        }
        fidl_fullmac::WlanFullmacImplIfcRequest::OnWmmStatusResp {
            status,
            wmm_params,
            responder,
        } => {
            responder.send().context("Failed to respond to OnWmmStatusResp")?;
            driver_event_sink.0.send(FullmacDriverEvent::OnWmmStatusResp {
                status,
                resp: fullmac_to_mlme::convert_wmm_params(wmm_params),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use futures::channel::mpsc;
    use futures::task::Poll;
    use std::pin::pin;
    use wlan_common::sink::UnboundedSink;

    // MLME owns the driver event receiver, but its lifetime is not exactly synchronized with the
    // lifetime of the WlanFullmacImplIfc server task.
    //
    // In case the WlanFullmacImplIfc server task is dropped after MLME is dropped, ensure that
    // nothing bad happens if we receive a request in the time between MLME and the
    // WlanFullmacImplIfc server dropping.
    //
    // This essentially checks that if the receiver is dropped, the server can still receive a
    // request and be dropped without issue.
    #[fuchsia::test]
    fn receive_request_after_driver_event_receiver_dropped() {
        let mut exec = fasync::TestExecutor::new();
        let (driver_event_sender, driver_event_receiver) = mpsc::unbounded();
        let (ifc_proxy, ifc_req_stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fullmac::WlanFullmacImplIfcMarker>();

        let driver_event_sink = FullmacDriverEventSink(UnboundedSink::new(driver_event_sender));
        let mut server_fut =
            pin!(serve_wlan_fullmac_impl_ifc_request_handler(ifc_req_stream, driver_event_sink));

        assert_matches!(exec.run_until_stalled(&mut server_fut), Poll::Pending);

        std::mem::drop(driver_event_receiver);

        let scan_end = fidl_fullmac::WlanFullmacImplIfcOnScanEndRequest {
            txn_id: Some(42u64),
            code: Some(fidl_fullmac::WlanScanResult::Success),
            ..Default::default()
        };

        let mut req_fut = pin!(ifc_proxy.on_scan_end(&scan_end));
        assert_matches!(exec.run_until_stalled(&mut req_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut server_fut), Poll::Pending);

        // check that server_fut completes the FIDL request without error
        assert_matches!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Ok(())));
    }
}
