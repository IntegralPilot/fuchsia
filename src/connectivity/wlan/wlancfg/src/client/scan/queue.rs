// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::ScanReason;
use crate::client::types::{self, Ssid};
use crate::telemetry::TelemetryEvent::{ScanQueueStatistics, ScanRequestFulfillmentTime};
use crate::telemetry::TelemetrySender;
use fidl_fuchsia_wlan_sme as fidl_sme;
use futures::channel::oneshot;
use log::warn;
use std::sync::LazyLock;

static WILDCARD_SSID: LazyLock<Ssid> = LazyLock::new(|| Ssid::from_bytes_unchecked("".into()));

struct QueuedRequest {
    reason: ScanReason,
    ssids: Vec<Ssid>,
    channels: Vec<types::WlanChan>,
    responder: oneshot::Sender<Result<Vec<types::ScanResult>, types::ScanError>>,
    received_at: zx::MonotonicInstant,
}

impl QueuedRequest {
    /// Returns true if the request's SSIDs can be fulfilled by the given SME request
    fn ssids_match(&self, sme_request: &fidl_sme::ScanRequest) -> bool {
        // An undirected (i.e. passive or wildcard active) scan can fulfill the request if
        // the request has no SSIDs specified, or specifies only the wildcard SSID
        let undirected_scan_fulfills_request =
            self.ssids.is_empty() || self.ssids.iter().all(|s| *s == *WILDCARD_SSID);

        if undirected_scan_fulfills_request {
            // Can be fulfilled by a passive scan or a wildcard active scan
            match sme_request {
                fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest { .. }) => true,
                fidl_sme::ScanRequest::Active(active_req) => {
                    // Empty SSID list is equivalent to only having the wildcard SSID
                    active_req.ssids.is_empty()
                        || active_req.ssids.contains(&WILDCARD_SSID.to_vec())
                }
            }
        } else {
            // Can only be fulfilled by an active scan
            match sme_request {
                fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest { .. }) => false,
                fidl_sme::ScanRequest::Active(active_req) => {
                    // Every SSID in the queued request must be in the SME request
                    self.ssids.iter().all(|ssid| active_req.ssids.contains(&ssid.to_vec()))
                }
            }
        }
    }

    /// Returns true if the request's channels can be fulfilled by the given SME request
    fn channels_match(&self, sme_request: &fidl_sme::ScanRequest) -> bool {
        match sme_request {
            fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest { .. }) => true,
            fidl_sme::ScanRequest::Active(active_req) => {
                self.channels.iter().all(|chan| active_req.channels.contains(&chan.primary))
            }
        }
    }

    /// Returns true if the request can be fulfilled by the given SME request
    pub fn can_be_fulfilled_by(&self, sme_request: &fidl_sme::ScanRequest) -> bool {
        self.ssids_match(sme_request) && self.channels_match(sme_request)
    }
}

pub struct RequestQueue {
    queue: Vec<QueuedRequest>,
    telemetry_sender: TelemetrySender,
}

impl RequestQueue {
    pub fn new(telemetry_sender: TelemetrySender) -> Self {
        Self { queue: vec![], telemetry_sender }
    }

    /// Adds a new request to the queue
    pub fn add_request(
        &mut self,
        reason: ScanReason,
        ssids: Vec<Ssid>,
        channels: Vec<types::WlanChan>,
        responder: oneshot::Sender<Result<Vec<types::ScanResult>, types::ScanError>>,
        current_time: zx::MonotonicInstant,
    ) {
        let req = QueuedRequest { reason, ssids, channels, responder, received_at: current_time };
        self.queue.push(req)
    }

    /// Selects the highest priority request in the queue
    fn select_highest_priority_request(&self) -> Option<(Vec<Ssid>, Vec<types::WlanChan>)> {
        // For now, the "highest priority" is the first request
        self.queue.first().map(|req| (req.ssids.clone(), req.channels.clone()))
    }

    /// Generates an SME request to fulfill the `highest_priority_request`. Also examines the
    /// request queue to opportunistically fulfill multiple requests via the single SME request.
    fn generate_combined_sme_request(
        &self,
        highest_priority_request: (Vec<Ssid>, Vec<types::WlanChan>),
    ) -> fidl_sme::ScanRequest {
        let (ssids, channels) = highest_priority_request;
        // For now, only fulfill the highest priority request
        if ssids.is_empty() {
            fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest { channels: vec![] })
        } else {
            fidl_sme::ScanRequest::Active(fidl_sme::ActiveScanRequest {
                ssids: ssids.iter().map(|ssid| ssid.to_vec()).collect(),
                channels: channels.iter().map(|chan| chan.primary).collect(),
            })
        }
    }

    /// Returns an SME request to fulfill at least the next prioritized request in the queue, and
    /// potentially other requests from the queue if possible.
    pub fn get_next_sme_request(&self) -> Option<fidl_sme::ScanRequest> {
        self.select_highest_priority_request().map(|req| self.generate_combined_sme_request(req))
    }

    /// Sends scan results to any matching requesters in the queue
    pub fn handle_completed_sme_scan(
        &mut self,
        sme_request: fidl_sme::ScanRequest,
        result: Result<Vec<types::ScanResult>, types::ScanError>,
        current_time: zx::MonotonicInstant,
    ) {
        let initial_queue_len = self.queue.len();
        let mut unfulfilled_requests = vec![];
        // Ideally we'd use `drain_filter` here: https://github.com/rust-lang/rust/issues/43244,
        // but it hasn't yet landed in stable (and doesn't have an ETA to do so).
        for req in self.queue.drain(..) {
            if req.can_be_fulfilled_by(&sme_request) {
                // Send the results to this requester
                #[allow(
                    clippy::redundant_pattern_matching,
                    reason = "mass allow for https://fxbug.dev/381896734"
                )]
                if let Err(_) = req.responder.send(result.clone()) {
                    warn!(
                        "Failed to send scan results to requester, receiving end was dropped. Request reason: {:?}, ssid count: {}, channel count: {}",
                        req.reason,
                        req.ssids.len(),
                        req.channels.len()
                    );
                };
                // Record metrics about scan request fulfillment time
                let req_duration = current_time - req.received_at;
                self.telemetry_sender.send(ScanRequestFulfillmentTime {
                    duration: req_duration,
                    reason: req.reason,
                });
            } else {
                unfulfilled_requests.push(req)
            }
        }
        self.queue = unfulfilled_requests;

        // Record metrics about queue fulfillment
        let final_queue_len = self.queue.len();
        let fulfilled_count = initial_queue_len - final_queue_len;
        self.telemetry_sender.send(ScanQueueStatistics {
            fulfilled_requests: fulfilled_count,
            remaining_requests: final_queue_len,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::TelemetryEvent;
    use crate::util::testing::{generate_channel, generate_random_channel, generate_ssid};
    use assert_matches::assert_matches;
    use futures::channel::mpsc;
    use test_case::test_case;

    static WILDCARD_STR: LazyLock<String> =
        LazyLock::new(|| WILDCARD_SSID.to_string_not_redactable().into());

    fn active_sme_req(ssids: Vec<&str>, channels: Vec<u8>) -> fidl_sme::ScanRequest {
        fidl_sme::ScanRequest::Active(fidl_sme::ActiveScanRequest {
            ssids: ssids.iter().map(|s| s.as_bytes().to_vec()).collect(),
            channels,
        })
    }

    fn passive_sme_req() -> fidl_sme::ScanRequest {
        fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest { channels: vec![] })
    }

    fn setup_queue() -> (RequestQueue, mpsc::Receiver<TelemetryEvent>) {
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let queue = RequestQueue::new(telemetry_sender);
        (queue, telemetry_receiver)
    }

    #[fuchsia::test]
    fn add_request() {
        let (mut queue, _telemetry_receiver) = setup_queue();

        let time = zx::MonotonicInstant::get();
        queue.add_request(ScanReason::NetworkSelection, vec![], vec![], oneshot::channel().0, time);
        assert_eq!(queue.queue.len(), 1);
        assert_eq!(queue.queue[0].reason, ScanReason::NetworkSelection);
        assert_eq!(queue.queue[0].ssids, Vec::<Ssid>::new());
        assert_eq!(queue.queue[0].channels, Vec::<types::WlanChan>::new());
        assert_eq!(queue.queue[0].received_at, time);

        let time = zx::MonotonicInstant::get();
        let ssids = vec![generate_ssid("foo"), generate_ssid("bar")];
        let channels = vec![generate_channel(3, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz)];
        queue.add_request(
            ScanReason::BssSelectionAugmentation,
            ssids.clone(),
            channels.clone(),
            oneshot::channel().0,
            time,
        );
        assert_eq!(queue.queue.len(), 2);
        assert_eq!(queue.queue[1].reason, ScanReason::BssSelectionAugmentation);
        assert_eq!(queue.queue[1].ssids, ssids.clone());
        assert_eq!(queue.queue[1].channels, channels.clone());
        assert_eq!(queue.queue[1].received_at, time);
    }

    #[fuchsia::test]
    fn highest_priority_request_is_first_request_in_queue() {
        let (mut queue, _telemetry_receiver) = setup_queue();
        queue.queue = vec![
            QueuedRequest {
                reason: ScanReason::BssSelectionAugmentation,
                ssids: vec![generate_ssid("foo"), generate_ssid("bar")],
                channels: vec![generate_random_channel(), generate_random_channel()],
                responder: oneshot::channel().0,
                received_at: zx::MonotonicInstant::ZERO,
            },
            QueuedRequest {
                reason: ScanReason::NetworkSelection,
                ssids: vec![],
                channels: vec![generate_random_channel()],
                responder: oneshot::channel().0,
                received_at: zx::MonotonicInstant::ZERO,
            },
        ];

        let (selected_ssids, selected_channels) = queue.select_highest_priority_request().unwrap();
        assert_eq!(selected_ssids, queue.queue[0].ssids);
        assert_eq!(selected_channels, queue.queue[0].channels);
    }

    // As long as there's no SSID, expect a passive scan
    #[test_case(vec![], vec![], passive_sme_req())]
    #[test_case(vec![], vec![generate_channel(3, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz)], passive_sme_req())]
    // If there's an SSID, expect an active scan
    #[test_case(vec![generate_ssid("foo")], vec![],
                active_sme_req (vec!["foo"], vec![]))]
    #[test_case(vec![generate_ssid("foo")], vec![generate_channel(1, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz), generate_channel(2, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz)],
                active_sme_req (vec!["foo"], vec![1, 2]))]
    #[fuchsia::test(add_test_attr = false)]
    fn generate_combined_sme_request(
        ssids: Vec<Ssid>,
        channels: Vec<types::WlanChan>,
        expected_sme_request: fidl_sme::ScanRequest,
    ) {
        let (queue, _telemetry_receiver) = setup_queue();
        let req = queue.generate_combined_sme_request((ssids, channels));
        assert_eq!(req, expected_sme_request);
    }

    #[fuchsia::test]
    fn get_next_sme_request_trivial_test() {
        // A trivial test of get_next_sme_request(), since its functionality is fully tested
        // by tests for select_highest_priority_request() and generate_combined_sme_request()
        let (queue, _telemetry_receiver) = setup_queue();
        assert_eq!(queue.get_next_sme_request(), None);
    }

    // A passive scan
    #[test_case(true, passive_sme_req())]
    // An active scan with no SSIDs
    #[test_case(true, active_sme_req(vec![], vec![]))]
    // An active scan with the wildcard SSID (more than once is OK) and no other SSIDS
    #[test_case(true, active_sme_req(vec![&WILDCARD_STR, &WILDCARD_STR], vec![]))]
    // An active scan with the wildcard SSID and other SSIDS
    #[test_case(true, active_sme_req(vec![&WILDCARD_STR, "foo"], vec![]))]
    // Doesn't match: an active scan with other SSIDs, without the wildcard SSID
    #[test_case(false, active_sme_req(vec!["foo"], vec![]))]
    #[fuchsia::test(add_test_attr = false)]
    /// Tests requests which should match "undirected" SME scans
    fn ssids_match_undirected(matches: bool, sme_request: fidl_sme::ScanRequest) {
        // Both of these requests should match "undirected" scans, i.e. passive or wildcard active
        let req_without_ssids = QueuedRequest {
            reason: ScanReason::BssSelectionAugmentation,
            ssids: vec![],
            channels: vec![],
            responder: oneshot::channel().0,
            received_at: zx::MonotonicInstant::ZERO,
        };
        let req_with_only_wildcard_ssid = QueuedRequest {
            reason: ScanReason::BssSelectionAugmentation,
            ssids: vec![WILDCARD_SSID.clone()],
            channels: vec![],
            responder: oneshot::channel().0,
            received_at: zx::MonotonicInstant::ZERO,
        };

        assert_eq!(matches, req_without_ssids.ssids_match(&sme_request));
        assert_eq!(matches, req_with_only_wildcard_ssid.ssids_match(&sme_request));
    }

    // An active scan with the same SSIDs as the request
    #[test_case(true, active_sme_req(vec![&WILDCARD_STR, "foo"], vec![]))]
    // An active scan with the same SSIDs as the request and some additional ones
    #[test_case(true, active_sme_req(vec![&WILDCARD_STR, "foo", "bar"], vec![]))]
    // Doesn't match: A passive scan
    #[test_case(false, passive_sme_req())]
    // Doesn't match: An active scan with no SSIDs
    #[test_case(false, active_sme_req(vec![], vec![]))]
    // Doesn't match: An active scan with the wildcard SSID and no other SSIDS
    #[test_case(false, active_sme_req(vec![&WILDCARD_STR], vec![]))]
    // Doesn't match: An active scan with the wildcard SSID and other SSIDS
    #[test_case(false, active_sme_req(vec![&WILDCARD_STR, "bar"], vec![]))]
    #[fuchsia::test(add_test_attr = false)]
    /// Tests requests which should match "directed" SME scans
    fn ssids_match_directed(matches: bool, sme_request: fidl_sme::ScanRequest) {
        let req_with_ssids = QueuedRequest {
            reason: ScanReason::BssSelectionAugmentation,
            ssids: vec![generate_ssid("foo"), generate_ssid(&WILDCARD_STR)],
            channels: vec![],
            responder: oneshot::channel().0,
            received_at: zx::MonotonicInstant::ZERO,
        };

        assert_eq!(matches, req_with_ssids.ssids_match(&sme_request));
    }

    // Request has no channels, fulfilled by passive SME scan
    #[test_case(true, vec![], passive_sme_req())]
    // Request has channels, passive scan fulfills all channels
    #[test_case(true, vec![1], passive_sme_req())]
    // Request has no channels, fulfilled by active SME scan with no channels
    #[test_case(true, vec![], active_sme_req(vec!["bar"], vec![]))]
    // Request has channels, fulfilled by active SME scan with matching channels
    #[test_case(true, vec![1, 2], active_sme_req(vec![], vec![1, 2, 55]))]
    // Doesn't match: Request has channels, SME scan with non-matching channels
    #[test_case(false, vec![1, 2], active_sme_req(vec![], vec![1, 5, 55]))]
    #[fuchsia::test(add_test_attr = false)]
    fn channels_match(matches: bool, req_channels: Vec<u8>, sme_request: fidl_sme::ScanRequest) {
        let req = QueuedRequest {
            reason: ScanReason::BssSelectionAugmentation,
            ssids: vec![generate_ssid("foo"), generate_ssid(&WILDCARD_STR)],
            channels: req_channels
                .iter()
                .map(|c| generate_channel(*c, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz))
                .collect(),
            responder: oneshot::channel().0,
            received_at: zx::MonotonicInstant::ZERO,
        };

        assert_eq!(matches, req.channels_match(&sme_request));
    }

    #[fuchsia::test]
    fn handle_completed_sme_scan() {
        let initial_time = zx::MonotonicInstant::from_nanos(123);
        let sme_req = passive_sme_req();

        // Generate two requests, one which matches the passive sme_req and one which doesn't
        let (matching_sender, mut matching_receiver) = oneshot::channel();
        let matching_req = QueuedRequest {
            reason: ScanReason::BssSelectionAugmentation,
            ssids: vec![],
            channels: vec![],
            responder: matching_sender,
            received_at: initial_time,
        };
        let (non_matching_sender, mut non_matching_receiver) = oneshot::channel();
        let non_matching_req = QueuedRequest {
            reason: ScanReason::BssSelectionAugmentation,
            ssids: vec![generate_ssid("foo")],
            channels: vec![],
            responder: non_matching_sender,
            received_at: zx::MonotonicInstant::get(), // this value is unused, so set it "randomly"
        };

        // Set up the queue
        let (mut queue, mut telemetry_receiver) = setup_queue();
        queue.queue.push(matching_req);
        queue.queue.push(non_matching_req);

        // Advance mock time
        let scan_duration = zx::MonotonicDuration::from_seconds(6);

        let scan_results = Err(types::ScanError::GeneralError);
        queue.handle_completed_sme_scan(
            sme_req,
            scan_results.clone(),
            initial_time + scan_duration,
        );

        // Check the results were sent to the matching request
        assert_eq!(matching_receiver.try_recv(), Ok(Some(scan_results)));
        assert_eq!(non_matching_receiver.try_recv(), Ok(None));
        // The matching request was removed from the queue
        assert_eq!(queue.queue.len(), 1);
        // The request fulfillment time was recorded
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, ScanRequestFulfillmentTime {
                duration,
                reason: ScanReason::BssSelectionAugmentation
            } => assert_eq!(duration, scan_duration));
        });
        // The request fulfillment count was recorded
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, ScanQueueStatistics {
                fulfilled_requests: 1,
                remaining_requests: 1,
            });
        });
    }
}
