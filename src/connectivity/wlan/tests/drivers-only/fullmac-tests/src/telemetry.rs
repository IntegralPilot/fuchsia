// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::FullmacDriverFixture;
use assert_matches::assert_matches;
use drivers_only_common::sme_helpers;
use fullmac_helpers::config::FullmacDriverConfig;
use {fidl_fuchsia_wlan_fullmac as fidl_fullmac, fidl_fuchsia_wlan_stats as fidl_stats};

#[fuchsia::test]
async fn test_get_iface_stats() {
    let mut fullmac_driver =
        FullmacDriverFixture::create(FullmacDriverConfig { ..Default::default() }).await;
    let telemetry_proxy = sme_helpers::get_telemetry(&fullmac_driver.generic_sme_proxy).await;
    let telemetry_fut = telemetry_proxy.get_iface_stats();

    let driver_connection_stats = fidl_stats::ConnectionStats {
        connection_id: Some(rand::random()),
        rx_unicast_total: Some(rand::random()),
        rx_unicast_drop: Some(rand::random()),
        rx_multicast: Some(rand::random()),
        tx_total: Some(rand::random()),
        tx_drop: Some(rand::random()),
        ..Default::default()
    };
    let driver_iface_stats = fidl_stats::IfaceStats {
        connection_stats: Some(driver_connection_stats.clone()),
        ..Default::default()
    };

    let driver_fut = async {
        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::GetIfaceStats { responder } => {
                responder.send(Ok(&driver_iface_stats))
                    .expect("Could not respond to GetIfaceHistogramStats");
        });
    };

    let (sme_iface_stats_result, _) = futures::join!(telemetry_fut, driver_fut);
    let sme_iface_stats = sme_iface_stats_result.expect("SME Error getting iface stats").unwrap();

    assert_eq!(
        sme_iface_stats,
        fidl_stats::IfaceStats {
            connection_stats: Some(driver_connection_stats),
            ..Default::default()
        }
    );
}

#[fuchsia::test]
async fn test_get_iface_histogram_stats() {
    let mut fullmac_driver =
        FullmacDriverFixture::create(FullmacDriverConfig { ..Default::default() }).await;
    let telemetry_proxy = sme_helpers::get_telemetry(&fullmac_driver.generic_sme_proxy).await;
    let telemetry_fut = telemetry_proxy.get_histogram_stats();

    // This contains every combination of hist scope and antenna frequency.
    // E.g., (per antenna, 2g), (station, 2g), (per antenna, 5g), (station, 5g)
    let driver_iface_histogram_stats = fidl_stats::IfaceHistogramStats {
        noise_floor_histograms: Some(vec![fidl_stats::NoiseFloorHistogram {
            hist_scope: fidl_stats::HistScope::PerAntenna,
            antenna_id: Some(Box::new(fidl_stats::AntennaId {
                freq: fidl_stats::AntennaFreq::Antenna2G,
                index: rand::random(),
            })),
            noise_floor_samples: vec![fidl_stats::HistBucket {
                bucket_index: rand::random(),
                num_samples: rand::random(),
            }],
            invalid_samples: rand::random(),
        }]),
        rssi_histograms: Some(vec![fidl_stats::RssiHistogram {
            hist_scope: fidl_stats::HistScope::Station,
            antenna_id: None,
            rssi_samples: vec![fidl_stats::HistBucket {
                bucket_index: rand::random(),
                num_samples: rand::random(),
            }],
            invalid_samples: rand::random(),
        }]),
        rx_rate_index_histograms: Some(vec![fidl_stats::RxRateIndexHistogram {
            hist_scope: fidl_stats::HistScope::PerAntenna,
            antenna_id: Some(Box::new(fidl_stats::AntennaId {
                freq: fidl_stats::AntennaFreq::Antenna5G,
                index: rand::random(),
            })),
            rx_rate_index_samples: vec![fidl_stats::HistBucket {
                bucket_index: rand::random(),
                num_samples: rand::random(),
            }],
            invalid_samples: rand::random(),
        }]),
        snr_histograms: Some(vec![fidl_stats::SnrHistogram {
            hist_scope: fidl_stats::HistScope::Station,
            antenna_id: None,
            snr_samples: vec![fidl_stats::HistBucket {
                bucket_index: rand::random(),
                num_samples: rand::random(),
            }],
            invalid_samples: rand::random(),
        }]),
        ..Default::default()
    };

    let driver_fut = async {
        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::GetIfaceHistogramStats { responder } => {
                responder.send(Ok(&driver_iface_histogram_stats))
                    .expect("Could not respond to GetIfaceHistogramStats");
        });
    };

    let (sme_histogram_stats_result, _) = futures::join!(telemetry_fut, driver_fut);
    let sme_histogram_stats =
        sme_histogram_stats_result.expect("SME error when getting histogram stats").unwrap();

    assert_eq!(driver_iface_histogram_stats, sme_histogram_stats);
}
