// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use crate::MlmeImpl;
use crate::block_ack::{BlockAckState, Closed};
use crate::client::lost_bss::LostBssCounter;
use crate::client::state::*;
use crate::client::{
    BoundClient, Client, ClientMlme, ParsedConnectRequest, TimedEvent, TimedEventClass,
};
use crate::device::{FakeDevice, FakeDeviceConfig, FakeDeviceState};
use crate::test_utils::{MockWlanRxInfo, fake_wlan_channel};
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_softmac as fidl_softmac;
use fuchsia_sync::Mutex;
use ieee80211::{Bssid, MacAddr, MacAddrBytes, Ssid};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use wlan_common::fake_bss_description;
use wlan_common::stats::SignalStrengthAverage;
use wlan_common::test_utils::fake_capabilities::fake_client_capabilities;
use wlan_common::timer::{self, EventId, Timer, create_timer};
use wlan_statemachine::*;

pub fn drain_timeouts(
    time_stream: &mut timer::EventStream<TimedEvent>,
) -> HashMap<TimedEventClass, Vec<(TimedEvent, EventId)>> {
    let mut timeouts = HashMap::new();
    loop {
        match time_stream.try_next() {
            Ok(Some((_, timed_event, _))) => {
                timeouts
                    .entry(timed_event.event.class())
                    .or_insert(vec![])
                    .push((timed_event.event, timed_event.id));
            }
            _ => return timeouts,
        };
    }
}

pub static BSSID: LazyLock<Bssid> = LazyLock::new(|| [6u8; 6].into());
pub static IFACE_MAC: LazyLock<MacAddr> = LazyLock::new(|| [7u8; 6].into());
pub const RSNE: &[u8] = &[
    0x30, 0x14, //  ID and len
    1, 0, //  version
    0x00, 0x0f, 0xac, 0x04, //  group data cipher suite
    0x01, 0x00, //  pairwise cipher suite count
    0x00, 0x0f, 0xac, 0x04, //  pairwise cipher suite list
    0x01, 0x00, //  akm suite count
    0x00, 0x0f, 0xac, 0x02, //  akm suite list
    0xa8, 0x04, //  rsn capabilities
];
pub const SCAN_CHANNEL: fidl_ieee80211::ChannelNumber =
    fidl_ieee80211::ChannelNumber { band: fidl_ieee80211::WlanBand::TwoGhz, number: 6 };
// Note: not necessarily valid beacon frame.
#[rustfmt::skip]
pub const BEACON_FRAME: &'static [u8] = &[
    // Mgmt header
    0b10000000, 0, // Frame Control
    0, 0, // Duration
    255, 255, 255, 255, 255, 255, // addr1
    6, 6, 6, 6, 6, 6, // addr2
    6, 6, 6, 6, 6, 6, // addr3
    0, 0, // Sequence Control
    // Beacon header:
    0, 0, 0, 0, 0, 0, 0, 0, // Timestamp
    10, 0, // Beacon interval
    33, 0, // Capabilities
    // IEs:
    0, 4, 0x73, 0x73, 0x69, 0x64, // SSID - "ssid"
    1, 8, 1, 2, 3, 4, 5, 6, 7, 8, // Supported rates
    3, 1, 11, // DSSS parameter set - channel 11
    5, 4, 0, 0, 0, 0, // TIM
];

pub struct MockObjects {
    pub fake_device: FakeDevice,
    pub fake_device_state: Arc<Mutex<FakeDeviceState>>,
    pub timer: Option<Timer<TimedEvent>>,
    pub time_stream: timer::EventStream<TimedEvent>,
}

impl MockObjects {
    // TODO(https://fxbug.dev/327499461): This function is async to ensure MLME functions will
    // run in an async context and not call `wlan_common::timer::Timer::now` without an
    // executor.
    pub async fn new() -> Self {
        let (timer, time_stream) = create_timer();
        let (fake_device, fake_device_state) = FakeDevice::new_with_config(
            FakeDeviceConfig::default()
                .with_mock_mac_role(fidl_common::WlanMacRole::Client)
                .with_mock_sta_addr((*IFACE_MAC).to_array()),
        )
        .await;
        Self { fake_device, fake_device_state, timer: Some(timer), time_stream }
    }

    pub async fn make_mlme(&mut self) -> ClientMlme<FakeDevice> {
        let mut mlme = ClientMlme::new(
            Default::default(),
            self.fake_device.clone(),
            self.timer.take().unwrap(),
        )
        .await
        .expect("Failed to create client MLME.");
        let channel = fake_wlan_channel();
        let (bandwidth, secondary80_num) = channel.bandwidth.to_fidl();
        let vht_secondary_80_channel =
            fidl_ieee80211::ChannelNumber { band: channel.band, number: secondary80_num };
        mlme.set_main_channel(channel.into(), bandwidth, vht_secondary_80_channel)
            .await
            .expect("unable to set main channel");
        mlme
    }
}

pub fn scan_req() -> fidl_mlme::ScanRequest {
    fidl_mlme::ScanRequest {
        txn_id: 1337,
        scan_type: fidl_mlme::ScanTypes::Passive,
        channel_list: vec![SCAN_CHANNEL],
        ssid_list: vec![Ssid::try_from("ssid").unwrap().into()],
        probe_delay: 0,
        min_channel_time: 100,
        max_channel_time: 300,
    }
}

pub fn make_client_station() -> Client {
    let connect_req = ParsedConnectRequest {
        selected_bss: fake_bss_description!(Open, bssid: BSSID.to_array()),
        connect_failure_timeout: 100,
        auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
        security_ie: vec![],
    };
    Client::new(connect_req, *IFACE_MAC, fake_client_capabilities())
}

pub fn make_client_station_protected() -> Client {
    let connect_req = ParsedConnectRequest {
        selected_bss: fake_bss_description!(Wpa2, bssid: BSSID.to_array()),
        connect_failure_timeout: 100,
        auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
        security_ie: RSNE.to_vec(),
    };
    Client::new(connect_req, *IFACE_MAC, fake_client_capabilities())
}

impl ClientMlme<FakeDevice> {
    pub fn make_client_station(&mut self) {
        self.sta.replace(make_client_station());
    }

    pub fn make_client_station_protected(&mut self) {
        self.sta.replace(make_client_station_protected());
    }

    pub fn get_bound_client(&mut self) -> Option<BoundClient<'_, FakeDevice>> {
        match self.sta.as_mut() {
            None => None,
            Some(sta) => Some(sta.bind(&mut self.ctx, &mut self.scanner, &mut self.channel_state)),
        }
    }
}

impl BoundClient<'_, FakeDevice> {
    pub fn move_to_associated_state(&mut self) {
        let status_check_timeout =
            schedule_association_status_timeout(self.sta.beacon_period(), &mut self.ctx.timer);
        let state = States::from(wlan_statemachine::testing::new_state(Associated(Association {
            aid: 42,
            assoc_resp_ies: vec![],
            controlled_port_open: true,
            ap_ht_op: None,
            ap_vht_op: None,
            qos: Qos::Disabled,
            lost_bss_counter: LostBssCounter::start(
                self.sta.beacon_period(),
                DEFAULT_AUTO_DEAUTH_TIMEOUT_BEACON_COUNT,
            ),
            status_check_timeout,
            signal_strength_average: SignalStrengthAverage::new(),
            block_ack_state: StateMachine::new(BlockAckState::from(State::new(Closed))),
        })));
        self.sta.state.replace(state);
    }

    pub async fn close_controlled_port(&mut self) {
        self.handle_mlme_request(wlan_sme::MlmeRequest::SetCtrlPort(
            fidl_mlme::SetControlledPortRequest {
                peer_sta_address: BSSID.to_array(),
                state: fidl_mlme::ControlledPortState::Closed,
            },
        ))
        .await;
    }
}

pub fn mock_rx_info<'a>(client: &BoundClient<'a, FakeDevice>) -> fidl_softmac::WlanRxInfo {
    let channel = client.channel_state.get_primary().unwrap();
    MockWlanRxInfo::with_channel(channel).into()
}
