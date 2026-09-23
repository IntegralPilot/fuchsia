// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::legacy::IfaceRef;
use fidl_fuchsia_wlan_product_deprecatedclient as deprecated;
use fidl_fuchsia_wlan_sme as fidl_sme;
use futures::prelude::*;
use log::{debug, error};

const MAX_CONCURRENT_WLAN_REQUESTS: usize = 1000;

/// Takes in stream of deprecated client requests and handles each one.
pub async fn serve_deprecated_client(
    requests: deprecated::DeprecatedClientRequestStream,
    iface: IfaceRef,
) -> Result<(), fidl::Error> {
    requests
        .try_for_each_concurrent(MAX_CONCURRENT_WLAN_REQUESTS, |req| {
            handle_request(iface.clone(), req)
        })
        .await
}

/// Handles an individual request from the deprecated client API.
async fn handle_request(
    iface: IfaceRef,
    req: deprecated::DeprecatedClientRequest,
) -> Result<(), fidl::Error> {
    match req {
        deprecated::DeprecatedClientRequest::Status { responder } => {
            debug!("Deprecated WLAN client API used for status request");
            let r = status(&iface).await;
            responder.send(&r)
        }
    }
}

/// Produces a status representing the state where no client interface is present.
fn no_client_status() -> deprecated::WlanStatus {
    deprecated::WlanStatus { state: deprecated::State::NoClient, current_ap: None }
}

/// Manages the calling of client SME status and translation into a format that is compatible with
/// the deprecated client API.
async fn status(iface: &IfaceRef) -> deprecated::WlanStatus {
    let iface = match iface.get() {
        Ok(iface) => iface,
        Err(_) => return no_client_status(),
    };

    let status = match iface.sme.status().await {
        Ok(status) => status,
        Err(e) => {
            // An error here indicates that the SME channel is broken.
            error!("Failed to query status: {}", e);
            return no_client_status();
        }
    };

    deprecated::WlanStatus {
        state: convert_state(&status),
        current_ap: extract_current_ap(&status),
    }
}

/// Translates a client SME's status information into a deprecated client state.
fn convert_state(status: &fidl_sme::ClientStatusResponse) -> deprecated::State {
    match status {
        fidl_sme::ClientStatusResponse::Connected(_) => deprecated::State::Associated,
        fidl_sme::ClientStatusResponse::Connecting(_) => deprecated::State::Associating,
        fidl_sme::ClientStatusResponse::Roaming(_) => deprecated::State::Associating,
        fidl_sme::ClientStatusResponse::Idle(_) => deprecated::State::Disassociated,
    }
}

/// Parses a Client SME's status and extracts AP SSID and RSSI if applicable.
fn extract_current_ap(status: &fidl_sme::ClientStatusResponse) -> Option<Box<deprecated::Ap>> {
    match status {
        fidl_sme::ClientStatusResponse::Connected(serving_ap_info) => {
            let ssid = String::from_utf8_lossy(&serving_ap_info.ssid).to_string();
            let rssi_dbm = serving_ap_info.rssi_dbm;
            Some(Box::new(deprecated::Ap { ssid, rssi_dbm }))
        }
        fidl_sme::ClientStatusResponse::Connecting(_)
        | fidl_sme::ClientStatusResponse::Roaming(_)
        | fidl_sme::ClientStatusResponse::Idle(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy::Iface;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy;
    use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
    use fuchsia_async as fasync;
    use futures::task::Poll;
    use std::pin::pin;

    struct TestValues {
        iface: IfaceRef,
        sme_stream: fidl_sme::ClientSmeRequestStream,
    }

    fn test_setup() -> TestValues {
        let (sme, server) = create_proxy::<fidl_sme::ClientSmeMarker>();

        let iface = Iface { sme, iface_id: 0 };
        let iface_ref = IfaceRef::new();
        iface_ref.set_if_empty(iface);

        TestValues { iface: iface_ref, sme_stream: server.into_stream() }
    }

    #[fuchsia::test]
    fn test_no_client() {
        let mut exec = fasync::TestExecutor::new();
        let iface = IfaceRef::new();
        let status_fut = status(&iface);
        let mut status_fut = pin!(status_fut);

        // Expect that no client is reported and the AP status information is empty.
        assert_matches!(
            exec.run_until_stalled(&mut status_fut),
            Poll::Ready(deprecated::WlanStatus {
                state: deprecated::State::NoClient,
                current_ap: None,
            })
        );
    }

    #[fuchsia::test]
    fn test_broken_sme() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();

        // Drop the SME request stream so that the client request will fail.
        drop(test_values.sme_stream);

        let status_fut = status(&test_values.iface);
        let mut status_fut = pin!(status_fut);

        // Expect that no client is reported and the AP status information is empty.
        assert_matches!(
            exec.run_until_stalled(&mut status_fut),
            Poll::Ready(deprecated::WlanStatus {
                state: deprecated::State::NoClient,
                current_ap: None,
            })
        );
    }

    #[fuchsia::test]
    fn test_disconnected_client() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();
        let status_fut = status(&test_values.iface);
        let mut status_fut = pin!(status_fut);

        // Expect an SME status request and send back a response indicating that the SME is neither
        // connected nor connecting.
        assert_matches!(exec.run_until_stalled(&mut status_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Status { responder }))) => {
                responder.send(&fidl_sme::ClientStatusResponse::Idle(fidl_sme::Empty{})).expect("could not send sme response")
            }
        );

        // Expect a disconnected status.
        assert_matches!(
            exec.run_until_stalled(&mut status_fut),
            Poll::Ready(deprecated::WlanStatus {
                state: deprecated::State::Disassociated,
                current_ap: None,
            })
        );
    }

    #[fuchsia::test]
    fn test_connecting_client() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();
        let status_fut = status(&test_values.iface);
        let mut status_fut = pin!(status_fut);

        // Expect an SME status request and send back a response indicating that the SME is
        // connecting.
        assert_matches!(exec.run_until_stalled(&mut status_fut), Poll::Pending);
        assert_matches!(
                    exec.run_until_stalled(&mut test_values.sme_stream.next()),
                    Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Status { responder }))) => {
                        responder.send(&fidl_sme::ClientStatusResponse::Connecting("test_ssid".as_bytes().to_vec()))
        .expect("could not send sme response")
                    }
                );

        // Expect a connecting status.
        assert_matches!(
            exec.run_until_stalled(&mut status_fut),
            Poll::Ready(deprecated::WlanStatus {
                state: deprecated::State::Associating,
                current_ap: None,
            })
        );
    }

    #[fuchsia::test]
    fn test_connected_client() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();
        let ssid = "test_ssid";
        let rssi_dbm = -70;
        let status_fut = status(&test_values.iface);
        let mut status_fut = pin!(status_fut);

        // Expect an SME status request and send back a response indicating that the SME is
        // connected.
        assert_matches!(exec.run_until_stalled(&mut status_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Status { responder }))) => {
                responder.send(&fidl_sme::ClientStatusResponse::Connected(
                    fidl_sme::ServingApInfo{
                        bssid: [0, 0, 0, 0, 0, 0],
                        ssid: ssid.as_bytes().to_vec(),
                        rssi_dbm,
                        snr_db: 0,
                        primary: fidl_ieee80211::ChannelNumber {
                            band: fidl_ieee80211::WlanBand::TwoGhz,
                            number: 1,
                        },
                        protection: fidl_sme::Protection::Unknown,
                        bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
                        vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                            band: fidl_ieee80211::WlanBand::TwoGhz,
                            number: 0,
                        },
                    })).expect("could not send sme response")
            }
        );

        // Expect a connected status.
        let expected_current_ap =
            Some(Box::new(deprecated::Ap { ssid: ssid.to_string(), rssi_dbm }));
        assert_matches!(
            exec.run_until_stalled(&mut status_fut),
            Poll::Ready(deprecated::WlanStatus {
                state: deprecated::State::Associated,
                current_ap,
            }) => {
                assert_eq!(current_ap, expected_current_ap);
            }
        );
    }
}
