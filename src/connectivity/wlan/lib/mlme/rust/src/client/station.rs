// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::bound::BoundClient;
use crate::client::state::States;
use crate::client::{ChannelState, Context, Scanner};
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use ieee80211::{Bssid, MacAddr, Ssid};
use wlan_common::bss::BssDescription;
use wlan_common::capabilities::ClientCapabilities;
use wlan_common::mac;
use wlan_common::time::TimeUnit;
use wlan_common::timer::EventHandle;
use wlan_trace as wtrace;
use zerocopy::SplitByteSlice;

pub struct Client {
    pub state: Option<States>,
    pub connect_req: ParsedConnectRequest,
    pub iface_mac: MacAddr,
    pub client_capabilities: ClientCapabilities,
    pub connect_timeout: Option<EventHandle>,
}

impl Client {
    pub fn new(
        connect_req: ParsedConnectRequest,
        iface_mac: MacAddr,
        client_capabilities: ClientCapabilities,
    ) -> Self {
        Self {
            state: Some(States::new_initial()),
            connect_req,
            iface_mac,
            client_capabilities,
            connect_timeout: None,
        }
    }

    pub fn ssid(&self) -> &Ssid {
        &self.connect_req.selected_bss.ssid
    }

    pub fn bssid(&self) -> Bssid {
        self.connect_req.selected_bss.bssid
    }

    pub fn beacon_period(&self) -> zx::MonotonicDuration {
        zx::MonotonicDuration::from(TimeUnit(self.connect_req.selected_bss.beacon_period))
    }

    pub fn eapol_required(&self) -> bool {
        self.connect_req.selected_bss.rsne().is_some()
        // TODO(https://fxbug.dev/42139266): Add detection of WPA1 in softmac for testing
        // purposes only. In particular, connect-to-wpa1-network relies
        // on this half of the OR statement.
            || self.connect_req.selected_bss.find_wpa_ie().is_some()
    }

    pub fn bind<'a, D>(
        &'a mut self,
        ctx: &'a mut Context<D>,
        scanner: &'a mut Scanner,
        channel_state: &'a mut ChannelState,
    ) -> BoundClient<'a, D> {
        BoundClient { sta: self, ctx, scanner, channel_state }
    }

    /// Only management and data frames should be processed. Furthermore, the source address should
    /// be the BSSID the client associated to and the receiver address should either be non-unicast
    /// or the client's MAC address.
    pub fn should_handle_frame<B: SplitByteSlice>(&self, mac_frame: &mac::MacFrame<B>) -> bool {
        wtrace::duration!("Client::should_handle_frame");

        // Technically, |transmitter_addr| and |receiver_addr| would be more accurate but using src
        // src and dst to be consistent with |data_dst_addr()|.
        let (src_addr, dst_addr) = match mac_frame {
            mac::MacFrame::Mgmt(mac::MgmtFrame { mgmt_hdr, .. }) => {
                (Some(mgmt_hdr.addr3), mgmt_hdr.addr1)
            }
            mac::MacFrame::Data(mac::DataFrame { fixed_fields, .. }) => {
                (mac::data_bssid(&fixed_fields), mac::data_dst_addr(&fixed_fields))
            }
            // Control frames are not supported. Drop them.
            _ => return false,
        };
        src_addr.is_some_and(|src_addr| src_addr == self.bssid().into())
            && (!dst_addr.is_unicast() || dst_addr == self.iface_mac)
    }
}

pub struct ParsedConnectRequest {
    pub selected_bss: BssDescription,
    pub connect_failure_timeout: u32,
    pub auth_type: fidl_mlme::AuthenticationTypes,
    pub security_ie: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::test_utils::*;
    use wlan_common::fake_fidl_bss_description;

    #[fuchsia::test(allow_stalls = false)]
    async fn rsn_ie_implies_sta_eapol_required() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        assert!(me.get_bound_client().is_none(), "MLME should not contain client, yet");
        me.on_sme_connect(fidl_mlme::ConnectRequest {
            selected_bss: fake_fidl_bss_description!(Wpa2, ssid: Ssid::try_from("foo").unwrap()),
            connect_failure_timeout: 100,
            auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
            sae_password: vec![],
            wep_key: None,
            security_ie: vec![],
            owe_public_key: None,
        })
        .await
        .expect("valid ConnectRequest should be handled successfully");
        let client = me.get_bound_client().expect("client sta should have been created by now.");
        assert!(client.sta.eapol_required());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn wpa1_implies_sta_eapol_required() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        assert!(me.get_bound_client().is_none(), "MLME should not contain client, yet");
        me.on_sme_connect(fidl_mlme::ConnectRequest {
            selected_bss: fake_fidl_bss_description!(Wpa1, ssid: Ssid::try_from("foo").unwrap()),
            connect_failure_timeout: 100,
            auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
            sae_password: vec![],
            wep_key: None,
            security_ie: vec![],
            owe_public_key: None,
        })
        .await
        .expect("valid ConnectRequest should be handled successfully");
        let client = me.get_bound_client().expect("client sta should have been created by now.");
        assert!(client.sta.eapol_required());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn no_wpa_or_rsn_ie_implies_sta_eapol_not_required() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        assert!(me.get_bound_client().is_none(), "MLME should not contain client, yet");
        me.on_sme_connect(fidl_mlme::ConnectRequest {
            selected_bss: fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap()),
            connect_failure_timeout: 100,
            auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
            sae_password: vec![],
            wep_key: None,
            security_ie: vec![],
            owe_public_key: None,
        })
        .await
        .expect("valid ConnectRequest should be handled successfully");
        let client = me.get_bound_client().expect("client sta should have been created by now.");
        assert!(!client.sta.eapol_required());
    }
}
