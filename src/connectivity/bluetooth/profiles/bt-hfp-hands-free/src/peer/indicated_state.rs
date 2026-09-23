// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_bluetooth_hfp as hfp;
use log::warn;
use std::collections::VecDeque;

#[derive(Clone, Copy, Default)]
struct NetworkInformation {
    service_available: Option<bool>,
    signal_strength: Option<hfp::SignalStrength>,
    roaming: Option<bool>,
}

impl From<NetworkInformation> for hfp::NetworkInformation {
    fn from(other: NetworkInformation) -> hfp::NetworkInformation {
        hfp::NetworkInformation {
            service_available: other.service_available,
            signal_strength: other.signal_strength,
            roaming: other.roaming,
            ..Default::default()
        }
    }
}

/// Keep track of state set by AG indicators
pub struct IndicatedState {
    network_information: NetworkInformation,
    network_information_dirty: bool,
    network_information_responders: VecDeque<hfp::PeerHandlerWatchNetworkInformationResponder>,
    // TODO(b/421998123) Report battery state.
    ag_battery_level: Option<i64>,
    initial_indicators_set: bool,
}

impl Default for IndicatedState {
    fn default() -> Self {
        Self {
            network_information: NetworkInformation::default(),
            network_information_dirty: false,
            network_information_responders: VecDeque::new(),
            ag_battery_level: None,
            initial_indicators_set: false,
        }
    }
}

impl IndicatedState {
    pub fn set_service_available(&mut self, service_available: bool) {
        self.network_information.service_available = Some(service_available);
        self.network_information_dirty = true;
        self.maybe_report_network_information()
    }

    pub fn set_signal_strength(&mut self, signal_strength: hfp::SignalStrength) {
        self.network_information.signal_strength = Some(signal_strength);
        self.network_information_dirty = true;
        self.maybe_report_network_information()
    }

    pub fn set_roaming(&mut self, roaming: bool) {
        self.network_information.roaming = Some(roaming);
        self.network_information_dirty = true;
        self.maybe_report_network_information()
    }

    fn maybe_report_network_information(&mut self) {
        if self.initial_indicators_set
            && self.network_information_dirty
            && self.network_information_responders.len() > 0
        {
            let responder =
                self.network_information_responders.pop_front().expect("Responder len > 0");
            let network_information: hfp::NetworkInformation = self.network_information.into();
            if let Err(e) = responder.send(&network_information) {
                warn!("Error sending network information: {:?}", e)
            }
            self.network_information_dirty = false;
        }
    }

    pub fn handle_watch_network_information(
        &mut self,
        responder: hfp::PeerHandlerWatchNetworkInformationResponder,
    ) {
        self.network_information_responders.push_back(responder);
        self.maybe_report_network_information();
    }

    pub fn set_ag_battery_level(&mut self, ag_battery_level: i64) {
        self.ag_battery_level = Some(ag_battery_level);
        self.maybe_report_battery_level();
    }

    // TODO(b/421998123) Report battery state.
    pub fn maybe_report_battery_level(&mut self) {}

    pub fn initial_indicators_set(&mut self) {
        self.initial_indicators_set = true;
        self.maybe_report_network_information();
        self.maybe_report_battery_level();
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy_and_stream;
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use std::task::Poll;

    fn get_network_information(
        exec: &mut fasync::TestExecutor,
        indicated_state: &mut IndicatedState,
    ) -> Option<hfp::NetworkInformation> {
        let (peer_handler_proxy, mut peer_handler_stream) =
            create_proxy_and_stream::<hfp::PeerHandlerMarker>();

        let mut network_info_fut = peer_handler_proxy.watch_network_information();
        let _ = exec.run_until_stalled(&mut network_info_fut);

        let network_info_request_fut = peer_handler_stream.next();
        let network_info_request = exec.run_singlethreaded(network_info_request_fut);
        let network_info_responder = match network_info_request {
            Some(Ok(hfp::PeerHandlerRequest::WatchNetworkInformation { responder })) => responder,
            other => panic!("Failed to get network info request: {:?}", other),
        };

        indicated_state.handle_watch_network_information(network_info_responder);

        let network_info = exec.run_until_stalled(&mut network_info_fut);

        let ret = match network_info {
            Poll::Pending => None,
            Poll::Ready(network_info_result) => {
                let network_info = network_info_result.expect("Error getting network info.");
                Some(network_info)
            }
        };

        return ret;
    }

    #[fuchsia::test]
    fn network_information() {
        let mut exec = fasync::TestExecutor::new();
        let mut indicated_state = IndicatedState::default();

        indicated_state.set_service_available(true);
        indicated_state.initial_indicators_set();

        let network_info = get_network_information(&mut exec, &mut indicated_state);

        assert_matches!(
            network_info,
            Some(hfp::NetworkInformation { service_available: Some(true), .. })
        );
    }

    #[fuchsia::test]
    fn network_information_before_indicators_set() {
        let mut exec = fasync::TestExecutor::new();

        let mut indicated_state = IndicatedState::default();

        indicated_state.set_signal_strength(hfp::SignalStrength::None);
        // No call to initial_indicators_set

        let network_info = get_network_information(&mut exec, &mut indicated_state);

        assert_matches!(network_info, None);
    }

    #[fuchsia::test]
    fn network_information_before_request() {
        let mut exec = fasync::TestExecutor::new();

        let mut indicated_state = IndicatedState::default();

        indicated_state.set_signal_strength(hfp::SignalStrength::None);
        indicated_state.initial_indicators_set();
        indicated_state.set_signal_strength(hfp::SignalStrength::VeryLow);
        indicated_state.set_signal_strength(hfp::SignalStrength::Low);

        let network_info = get_network_information(&mut exec, &mut indicated_state);
        // Only the last update comes through before the first call to the hanging get.
        assert_matches!(
            network_info,
            Some(hfp::NetworkInformation { signal_strength: Some(hfp::SignalStrength::Low), .. })
        );

        indicated_state.set_signal_strength(hfp::SignalStrength::Medium);
        indicated_state.set_signal_strength(hfp::SignalStrength::High);

        // Again, only the last value comes through.
        let network_info = get_network_information(&mut exec, &mut indicated_state);
        assert_matches!(
            network_info,
            Some(hfp::NetworkInformation { signal_strength: Some(hfp::SignalStrength::High), .. })
        );
    }
}
