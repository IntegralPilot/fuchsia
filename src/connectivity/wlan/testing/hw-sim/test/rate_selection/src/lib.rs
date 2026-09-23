// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_fuchsia_wlan_softmac::{WlanTxResult, WlanTxResultCode, WlanTxResultEntry};
use fidl_fuchsia_wlan_tap as fidl_tap;
use fidl_test_wlan_realm::WlanConfig;
use fuchsia_async::Interval;
use futures::StreamExt;
use futures::channel::mpsc;
use ieee80211::{Bssid, MacAddrBytes};
use log::info;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::pin::pin;
use std::sync::LazyLock;
use wlan_common::bss::Protection;
use wlan_common::channel::{Bandwidth, Channel};
use wlan_common::mac;
use wlan_hw_sim::event::buffered::{Buffered, DataFrame};
use wlan_hw_sim::event::{self, Handler};
use wlan_hw_sim::{
    AP_SSID, ApAdvertisement, Beacon, CLIENT_MAC_ADDR, ETH_DST_MAC, connect_or_timeout,
    default_wlantap_config_client, loop_until_iface_is_found, netdevice_helper, test_utils,
};
use zerocopy::IntoBytes;
use zerocopy::byteorder::big_endian::U16 as BigEndianU16;
// Remedy for https://fxbug.dev/42162128 (https://fxbug.dev/42108316)
// Refer to |KMinstrelUpdateIntervalForHwSim| in //src/connectivity/wlan/drivers/wlan/device.cpp
const DATA_FRAME_INTERVAL_NANOS: i64 = 4_000_000;

static BSS_MINSTL: LazyLock<Bssid> =
    LazyLock::new(|| Bssid::from([0x6d, 0x69, 0x6e, 0x73, 0x74, 0x0a]));

// Simulated hardware supports eight ERP transmission vectors with indices 129 to 136, inclusive.
// See the `send_association_response` function.
const REQUIRED_IDXS: &[u16] = &[129, 130, 131, 132, 133, 134, 136]; // Missing 135 is OK.
const SUPPORTED_IDXS: &[u16] = &[129, 130, 131, 132, 133, 134, 135, 136];
const ERP_STARTING_IDX: u16 = 129;
const MAX_SUCCESSFUL_IDX: u16 = 130;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TxVecCount {
    tx_vec_idx: u16,
    count: u64,
}

impl From<(u16, u64)> for TxVecCount {
    fn from((tx_vec_idx, count): (u16, u64)) -> Self {
        TxVecCount { tx_vec_idx, count }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Maxima<T> {
    pub max: T,
    pub penult: T,
}

impl Maxima<TxVecCount> {
    pub fn by_tx_vec_idx(self) -> Maxima<u16> {
        let Maxima {
            max: TxVecCount { tx_vec_idx: max, .. },
            penult: TxVecCount { tx_vec_idx: penult, .. },
        } = self;
        Maxima { max, penult }
    }
}

trait TxVecCountMaxima {
    fn maxima_by_count(&self) -> Option<Maxima<TxVecCount>>;
}

impl TxVecCountMaxima for HashMap<u16, u64> {
    fn maxima_by_count(&self) -> Option<Maxima<TxVecCount>> {
        fn copied((key, value): (&u16, &u64)) -> (u16, u64) {
            (*key, *value)
        }
        self.iter().map(copied).max_by_key(|(_, n)| *n).map(TxVecCount::from).and_then(|max| {
            self.iter()
                .filter(|&(&tx_vec_idx, _)| tx_vec_idx != max.tx_vec_idx)
                .map(copied)
                .max_by_key(|(_, n)| *n)
                .map(TxVecCount::from)
                .map(|penult| Maxima { max, penult })
        })
    }
}

#[derive(Clone, Debug, Default)]
struct MinstrelConvergence {
    snapshot: Option<Maxima<TxVecCount>>,
}

impl MinstrelConvergence {
    pub fn get_and_snapshot(
        &mut self,
        tx_vec_counts: &HashMap<u16, u64>,
    ) -> Option<Maxima<TxVecCount>> {
        tx_vec_counts.maxima_by_count().and_then(|maxima| {
            let is_converged = self.is_converged(tx_vec_counts.len(), &maxima);
            // Take snapshots at a reduced frequency to determine convergence more reliably by
            // ignoring early fluctuations.
            if maxima.max.count % 100 == 0 {
                info!(
                    "snapshotting tx_vec_idx counts with maxima {:?}: {:?}",
                    maxima, tx_vec_counts
                );
                self.snapshot = Some(maxima);
            }
            is_converged.then(|| self.snapshot).flatten()
        })
    }

    // Check for convergence in the snapshots. A snapshot is taken when "100 frames (~4 minstrel
    // cycles) are sent with the same tx vector index". This function examines the most used and
    // the second most used transmission vectors (i.e., data rates) since the last snapshot.
    //
    // Once Minstrel converges, the optimal transmission vector will remain unchanged and, due to
    // the probing mechanism, the second most used transmission vector will also be used regularly
    // (though much less frequently) to ensure that it is still viable.
    fn is_converged(&self, n: usize, maxima: &Maxima<TxVecCount>) -> bool {
        // Due to its randomness, Minstrel may skip vector 135, but the other 7 must be present.
        if n < REQUIRED_IDXS.len() {
            return false;
        }
        if let Some(snapshot) = self.snapshot.as_ref() {
            if maxima.by_tx_vec_idx() != snapshot.by_tx_vec_idx() {
                false
            } else {
                // One transmission vector has become dominant, because the number of data frames
                // transmitted on that vector is at least 15 times as large as any other vector.
                // Note that there are 15 non-probing data frames between any two consecutive
                // probing data frames.
                const FRAMES_PER_PROBE: u64 = 15;

                let max = maxima.max.count - snapshot.max.count;
                let penult = maxima.penult.count - snapshot.penult.count;
                // The penult vector is used regularly (more than once), but much less frequently
                // than the dominant vector.
                penult > 1 && max >= (FRAMES_PER_PROBE * penult)
            }
        } else {
            false
        }
    }
}

fn send_tx_result_and_minstrel_convergence<'h>(
    phy: &'h fidl_tap::WlantapPhyProxy,
    bssid: &'h Bssid,
    tx_vec_counts: &'h mut HashMap<u16, u64>,
    mut sender: mpsc::Sender<Option<Maxima<TxVecCount>>>,
) -> impl Handler<(), fidl_tap::WlantapPhyEvent> + 'h {
    let mut convergence = MinstrelConvergence::default();
    event::on_transmit(event::extract(
        move |_: Buffered<DataFrame>, packet: fidl_tap::WlanTxPacket| {
            let tx_vec_idx = packet.info.tx_vector_idx;
            send_tx_result(
                *bssid,
                tx_vec_idx,
                // Only the lowest indices can succeed in the simulated environment.
                (ERP_STARTING_IDX..=MAX_SUCCESSFUL_IDX).contains(&tx_vec_idx),
                phy,
            )
            .context("failed to send transmission status report")?;
            match tx_vec_counts.entry(tx_vec_idx) {
                Entry::Occupied(mut count) => *count.get_mut() += 1,
                Entry::Vacant(vacancy) => {
                    vacancy.insert(1);
                    info!(
                        "new tx_vec_idx: {} at #{}",
                        tx_vec_idx,
                        tx_vec_counts.values().sum::<u64>()
                    );
                }
            };
            sender
                .try_send(convergence.get_and_snapshot(tx_vec_counts))
                .context("failed to send Minstrel convergence")
        },
    ))
    .expect("failed to simulate station")
}

fn wlan_tx_result_entry(tx_vec_idx: u16) -> WlanTxResultEntry {
    WlanTxResultEntry { tx_vector_idx: tx_vec_idx, attempts: 1 }
}

fn send_tx_result(
    bssid: Bssid,
    tx_vec_idx: u16,
    is_successful: bool,
    proxy: &fidl_tap::WlantapPhyProxy,
) -> Result<(), Error> {
    let result = WlanTxResult {
        peer_addr: bssid.to_array(),
        result_code: if is_successful {
            WlanTxResultCode::Success
        } else {
            WlanTxResultCode::Failed
        },
        tx_result_entry: [
            wlan_tx_result_entry(tx_vec_idx),
            wlan_tx_result_entry(0),
            wlan_tx_result_entry(0),
            wlan_tx_result_entry(0),
            wlan_tx_result_entry(0),
            wlan_tx_result_entry(0),
            wlan_tx_result_entry(0),
            wlan_tx_result_entry(0),
        ],
    };
    proxy.report_tx_result(&result)?;
    Ok(())
}

async fn send_eth_beacons<'a>(
    session: &netdevice_client::Session,
    port: &netdevice_client::Port,
    receiver: &'a mut mpsc::Receiver<Option<Maxima<TxVecCount>>>,
    phy: &'a fidl_tap::WlantapPhyProxy,
) -> Result<Maxima<TxVecCount>, Error> {
    let ethernet_header = mac::EthernetIIHdr {
        da: *ETH_DST_MAC,
        sa: *CLIENT_MAC_ADDR,
        ether_type: BigEndianU16::new(mac::ETHER_TYPE_IPV4),
    };
    let buf = ethernet_header.as_bytes();

    let mut timer_stream =
        Interval::new(zx::MonotonicDuration::from_nanos(DATA_FRAME_INTERVAL_NANOS));
    let mut intervals_since_last_beacon: i64 = i64::MAX / DATA_FRAME_INTERVAL_NANOS;
    loop {
        timer_stream.next().await;

        // The auto-deauthentication timeout is 10.24 seconds. Send a beacon before then to stay
        // connected.
        if zx::MonotonicDuration::from_nanos(
            intervals_since_last_beacon * DATA_FRAME_INTERVAL_NANOS,
        ) >= zx::MonotonicDuration::from_millis(8765)
        {
            intervals_since_last_beacon = 0;
            Beacon {
                channel: Channel::new(1, Bandwidth::Cbw20, fidl_ieee80211::WlanBand::TwoGhz),
                bssid: *BSS_MINSTL,
                ssid: AP_SSID.clone(),
                protection: Protection::Open,
                rssi_dbm: 0,
            }
            .send(phy)
            .expect("failed to send beacon");
        }
        intervals_since_last_beacon += 1;

        netdevice_helper::send(&session, &port, &buf).await;
        if let Some(snapshot) = receiver.next().await.expect("failed to receive convergence") {
            return Ok(snapshot);
        }
    }
}

/// Test rate selection is working correctly by verifying data rate is reduced once the Minstrel
/// algorithm detects transmission failures. Transmission failures are simulated by fake tx status
/// report created by the test.
#[fuchsia::test]
async fn rate_selection() {
    let mut helper = test_utils::TestHelper::begin_test(
        fidl_tap::WlantapPhyConfig { quiet: true, ..default_wlantap_config_client() },
        WlanConfig {
            use_legacy_privacy: Some(false),
            with_regulatory_region: Some(true),
            with_policy: Some(true),
            ..Default::default()
        },
    )
    .await;
    let () = loop_until_iface_is_found(&mut helper).await;

    let () = connect_or_timeout(
        &mut helper,
        zx::MonotonicDuration::from_seconds(30),
        &AP_SSID,
        &BSS_MINSTL,
        &Protection::Open,
        None,
        fidl_policy::SecurityType::None,
    )
    .await;

    let phy = helper.proxy();
    let (sender, mut receiver) = mpsc::channel(1);

    let (session, port) = helper.start_netdevice_session(*CLIENT_MAC_ADDR).await;
    let send_eth_beacons = pin!(send_eth_beacons(&session, &port, &mut receiver, &phy));

    let mut tx_vec_counts = HashMap::new();
    let snapshot = helper
        .run_until_complete_or_timeout(
            zx::MonotonicDuration::from_seconds(30),
            "verify rate selection converges to 130",
            send_tx_result_and_minstrel_convergence(
                &phy,
                &BSS_MINSTL,
                &mut tx_vec_counts,
                sender.clone(),
            ),
            send_eth_beacons,
        )
        .await
        .expect("running main future");

    let sum = tx_vec_counts.values().sum::<u64>();
    info!("final tx_vec_idx counts:\n{:#?}\ntotal: {}", tx_vec_counts, sum);
    assert!(tx_vec_counts.contains_key(&MAX_SUCCESSFUL_IDX));
    let others = sum - tx_vec_counts[&MAX_SUCCESSFUL_IDX];
    info!("others: {}", others);
    let mut tx_vecs: Vec<_> = tx_vec_counts.keys().cloned().collect();
    tx_vecs.sort();
    if tx_vecs.len() == REQUIRED_IDXS.len() {
        // Vector 135 may not be attempted due to randomness and that's OK.
        assert_eq!(&tx_vecs[..], REQUIRED_IDXS);
    } else {
        assert_eq!(&tx_vecs[..], SUPPORTED_IDXS);
    }
    info!(
        "If the test fails due to QEMU slowness outside of the scope of WLAN (See https://fxbug.dev/42162128, \
         https://fxbug.dev/42108316). Try increasing |DATA_FRAME_INTERVAL_NANOS| above."
    );
    assert_eq!(snapshot.max.tx_vec_idx, MAX_SUCCESSFUL_IDX);
}
