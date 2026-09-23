// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::connection_selection::scoring_functions;
use crate::client::types;
use crate::telemetry::{TelemetryEvent, TelemetrySender};
use fuchsia_inspect_contrib::inspect_log;
use fuchsia_inspect_contrib::log::InspectList;
use fuchsia_inspect_contrib::nodes::BoundedListNode as InspectBoundedListNode;
use futures::lock::Mutex;
use log::{error, info};
use std::cmp::Reverse;
use std::sync::Arc;

/// BSS selection. Selects the best from a list of candidates that are available for
/// connection.
pub async fn select_bss(
    allowed_candidate_list: Vec<types::ScannedCandidate>,
    reason: types::ConnectReason,
    inspect_node: Arc<Mutex<InspectBoundedListNode>>,
    telemetry_sender: TelemetrySender,
) -> Option<types::ScannedCandidate> {
    if allowed_candidate_list.is_empty() {
        info!("No BSSs available to select from.");
    } else {
        info!("Selecting from {} BSSs found for allowed networks", allowed_candidate_list.len());
    }

    let mut inspect_node = inspect_node.lock().await;

    let mut scored_candidates = allowed_candidate_list
        .iter()
        .inspect(|&candidate| {
            info!("{}", candidate.to_string_without_pii());
        })
        .filter(|&candidate| {
            // Note: this check is redundant, because all ScannedCandidates have an authenticator,
            // and only BSSs with compatibility can have an authenticator. It would be unexpected
            // to ever hit this branch, unless logic changes elsewhere.
            if !candidate.bss.is_compatible() {
                error!("BSS is unexpectedly incompatible: {}", candidate.to_string_without_pii());
                false
            } else {
                true
            }
        })
        .map(|candidate| {
            (candidate.clone(), scoring_functions::score_bss_scanned_candidate(candidate.clone()))
        })
        .collect::<Vec<(types::ScannedCandidate, i16)>>();

    scored_candidates.sort_by_key(|(_, score)| Reverse(*score));
    let selected_candidate = scored_candidates.first();

    // Log the candidates into Inspect
    inspect_log!(
        inspect_node,
        candidates: InspectList(&allowed_candidate_list),
        selected?: selected_candidate.map(|(candidate, _)| candidate)
    );

    telemetry_sender.send(TelemetryEvent::BssSelectionResult {
        reason,
        scored_candidates: scored_candidates.clone(),
        selected_candidate: selected_candidate.cloned(),
    });

    if let Some((candidate, _)) = selected_candidate {
        info!("Selected BSS:");
        info!("{}", candidate.to_string_without_pii());
        Some(candidate.clone())
    } else {
        None
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::config_management::{ConnectFailure, FailureReason};
    use crate::util::testing::{
        generate_channel, generate_random_bss_with_compatibility, generate_random_connect_reason,
        generate_random_scanned_candidate,
    };
    use assert_matches::assert_matches;
    use diagnostics_assertions::{
        AnyBoolProperty, AnyNumericProperty, AnyProperty, AnyStringProperty, assert_data_tree,
    };
    use fuchsia_async as fasync;
    use fuchsia_inspect as inspect;
    use futures::channel::mpsc;
    use ieee80211_testutils::{BSSID_REGEX, SSID_REGEX};
    use wlan_common::random_fidl_bss_description;
    use wlan_common::scan::Incompatible;

    struct TestValues {
        inspector: inspect::Inspector,
        inspect_node: Arc<Mutex<InspectBoundedListNode>>,
        telemetry_sender: TelemetrySender,
        telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
    }

    fn test_setup() -> TestValues {
        let inspector = inspect::Inspector::default();
        let inspect_node =
            InspectBoundedListNode::new(inspector.root().create_child("bss_select_test"), 10);
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);

        TestValues {
            inspector,
            inspect_node: Arc::new(Mutex::new(inspect_node)),
            telemetry_sender: TelemetrySender::new(telemetry_sender),
            telemetry_receiver,
        }
    }

    fn generate_candidate_for_scoring(
        rssi: i8,
        snr_db: i8,
        channel: types::WlanChan,
    ) -> types::ScannedCandidate {
        let bss = types::Bss {
            signal: types::Signal { rssi_dbm: rssi, snr_db },
            channel,
            bss_description: fidl_fuchsia_wlan_ieee80211::BssDescription {
                rssi_dbm: rssi,
                snr_db,
                primary: channel.into(),
                ..random_fidl_bss_description!()
            }
            .into(),
            ..generate_random_bss_with_compatibility()
        };
        types::ScannedCandidate { bss, ..generate_random_scanned_candidate() }
    }

    fn connect_failure_with_bssid(bssid: types::Bssid) -> ConnectFailure {
        ConnectFailure {
            reason: FailureReason::GeneralFailure,
            time: fasync::MonotonicInstant::INFINITE,
            bssid,
        }
    }

    #[allow(clippy::vec_init_then_push, reason = "mass allow for https://fxbug.dev/381896734")]
    #[fuchsia::test]
    fn select_bss_sorts_by_score() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let mut candidates = vec![];

        candidates.push(generate_candidate_for_scoring(
            -35,
            30,
            generate_channel(36, fidl_fuchsia_wlan_ieee80211::WlanBand::FiveGhz),
        ));
        candidates.push(generate_candidate_for_scoring(
            -30,
            30,
            generate_channel(1, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz),
        ));

        // there's a network on 5G, it should get a boost and be selected
        let reason = generate_random_connect_reason();
        assert_eq!(
            exec.run_singlethreaded(select_bss(
                candidates.clone(),
                reason,
                test_values.inspect_node.clone(),
                test_values.telemetry_sender.clone()
            )),
            Some(candidates[0].clone())
        );

        // make the 5GHz network into a 2.4GHz network
        let mut modified_network = candidates[0].clone();
        let modified_bss = types::Bss {
            channel: generate_channel(6, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz),
            ..modified_network.bss.clone()
        };
        modified_network.bss = modified_bss;
        candidates[0] = modified_network;

        // all networks are 2.4GHz, strongest RSSI network returned
        assert_eq!(
            exec.run_singlethreaded(select_bss(
                candidates.clone(),
                reason,
                test_values.inspect_node.clone(),
                test_values.telemetry_sender.clone()
            )),
            Some(candidates[1].clone())
        );
    }

    #[allow(clippy::vec_init_then_push, reason = "mass allow for https://fxbug.dev/381896734")]
    #[fuchsia::test]
    fn select_bss_sorts_by_failure_count() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let mut candidates = vec![];

        candidates.push(generate_candidate_for_scoring(
            -30,
            30,
            generate_channel(1, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz),
        ));
        candidates.push(generate_candidate_for_scoring(
            -35,
            30,
            generate_channel(1, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz),
        ));

        // stronger network returned
        assert_eq!(
            exec.run_singlethreaded(select_bss(
                candidates.clone(),
                generate_random_connect_reason(),
                test_values.inspect_node.clone(),
                test_values.telemetry_sender.clone()
            )),
            Some(candidates[0].clone()),
        );

        // mark the stronger network as having some failures
        let num_failures = 4;
        candidates[0].saved_network_info.recent_failures =
            vec![connect_failure_with_bssid(candidates[0].bss.bssid); num_failures];

        // weaker network (with no failures) returned
        assert_eq!(
            exec.run_singlethreaded(select_bss(
                candidates.clone(),
                generate_random_connect_reason(),
                test_values.inspect_node.clone(),
                test_values.telemetry_sender.clone()
            )),
            Some(candidates[1].clone())
        );

        // give them both the same number of failures
        candidates[1].saved_network_info.recent_failures =
            vec![connect_failure_with_bssid(candidates[1].bss.bssid); num_failures];

        // stronger network returned
        assert_eq!(
            exec.run_singlethreaded(select_bss(
                candidates.clone(),
                generate_random_connect_reason(),
                test_values.inspect_node.clone(),
                test_values.telemetry_sender.clone()
            )),
            Some(candidates[0].clone())
        );
    }

    #[allow(clippy::vec_init_then_push, reason = "mass allow for https://fxbug.dev/381896734")]
    #[fuchsia::test]
    fn select_bss_ignore_incompatible() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let mut candidates = vec![];

        // Add two BSSs, both compatible to start.
        candidates.push(generate_candidate_for_scoring(
            -14,
            30,
            generate_channel(1, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz),
        ));
        candidates.push(generate_candidate_for_scoring(
            -90,
            30,
            generate_channel(1, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz),
        ));

        // The stronger BSS is selected initially.
        assert_eq!(
            exec.run_singlethreaded(select_bss(
                candidates.clone(),
                generate_random_connect_reason(),
                test_values.inspect_node.clone(),
                test_values.telemetry_sender.clone()
            )),
            Some(candidates[0].clone())
        );

        // Make the stronger BSS incompatible.
        candidates[0].bss.compatibility = Incompatible::unknown();

        // The weaker, but still compatible, BSS is selected.
        assert_eq!(
            exec.run_singlethreaded(select_bss(
                candidates.clone(),
                generate_random_connect_reason(),
                test_values.inspect_node.clone(),
                test_values.telemetry_sender.clone()
            )),
            Some(candidates[1].clone())
        );

        // TODO(https://fxbug.dev/42071595): After `select_bss` filters out incompatible BSSs, this None
        // compatibility should change to a Some, to test that logic.
        // Make both BSSs incompatible.
        candidates[1].bss.compatibility = Incompatible::unknown();

        // No BSS is selected.
        assert_eq!(
            exec.run_singlethreaded(select_bss(
                candidates.clone(),
                generate_random_connect_reason(),
                test_values.inspect_node.clone(),
                test_values.telemetry_sender.clone()
            )),
            None
        );
    }

    #[allow(clippy::vec_init_then_push, reason = "mass allow for https://fxbug.dev/381896734")]
    #[fuchsia::test]
    fn select_bss_logs_to_inspect() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let mut candidates = vec![];

        candidates.push(generate_candidate_for_scoring(
            -50,
            30,
            generate_channel(1, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz),
        ));
        candidates.push(generate_candidate_for_scoring(
            -60,
            30,
            generate_channel(3, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz),
        ));
        candidates.push(generate_candidate_for_scoring(
            -30,
            30,
            generate_channel(6, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz),
        ));

        // stronger network returned
        assert_eq!(
            exec.run_singlethreaded(select_bss(
                candidates.clone(),
                generate_random_connect_reason(),
                test_values.inspect_node.clone(),
                test_values.telemetry_sender.clone()
            )),
            Some(candidates[2].clone())
        );

        assert_data_tree!(@executor exec, test_values.inspector, root: {
            bss_select_test: {
                "0": {
                    "@time": AnyNumericProperty,
                    "candidates": {
                        "0": contains {
                            score: AnyNumericProperty,
                            bssid: &*BSSID_REGEX,
                            ssid: &*SSID_REGEX,
                            rssi: AnyNumericProperty,
                            security_type_saved: AnyStringProperty,
                            security_type_scanned: AnyStringProperty,
                            channel: AnyStringProperty,
                            compatible: AnyBoolProperty,
                            incompatibility: AnyStringProperty,
                            recent_failure_count: AnyNumericProperty,
                            saved_network_has_ever_connected: AnyBoolProperty,
                        },
                        "1": contains {
                            score: AnyProperty,
                        },
                        "2": contains {
                            score: AnyProperty,
                        },
                    },
                    "selected": {
                        ssid: candidates[2].network.ssid.to_string(),
                        bssid: candidates[2].bss.bssid.to_string(),
                        rssi: i64::from(candidates[2].bss.signal.rssi_dbm),
                        score: i64::from(scoring_functions::score_bss_scanned_candidate(candidates[2].clone())),
                        security_type_saved: candidates[2].saved_security_type_to_string(),
                        security_type_scanned: format!("{}", wlan_common::bss::Protection::from(candidates[2].security_type_detailed)),
                        channel: AnyStringProperty,
                        compatible: candidates[2].bss.is_compatible(),
                        incompatibility: AnyStringProperty,
                        recent_failure_count: candidates[2].recent_failure_count(),
                        saved_network_has_ever_connected: candidates[2].saved_network_info.has_ever_connected,
                    },
                }
            },
        });
    }

    #[fuchsia::test]
    fn select_bss_empty_list_logs_to_inspect() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        assert_eq!(
            exec.run_singlethreaded(select_bss(
                vec![],
                generate_random_connect_reason(),
                test_values.inspect_node.clone(),
                test_values.telemetry_sender.clone()
            )),
            None
        );

        // Verify that an empty list of candidates is still logged to inspect.
        assert_data_tree!(@executor exec, test_values.inspector, root: {
            bss_select_test: {
                "0": {
                    "@time": AnyProperty,
                    "candidates": {},
                }
            },
        });
    }

    #[fuchsia::test]
    fn select_bss_logs_cobalt_metrics() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();

        let reason_code = generate_random_connect_reason();
        let candidates =
            vec![generate_random_scanned_candidate(), generate_random_scanned_candidate()];
        assert!(
            exec.run_singlethreaded(select_bss(
                candidates.clone(),
                reason_code,
                test_values.inspect_node.clone(),
                test_values.telemetry_sender.clone()
            ))
            .is_some()
        );

        assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::BssSelectionResult {
                reason,
                scored_candidates,
                selected_candidate: _,
            } => {
                assert_eq!(reason, reason_code);
                let mut prior_score = i16::MAX;
                for (candidate, score) in scored_candidates {
                    assert!(candidates.contains(&candidate));
                    assert!(prior_score >= score);
                    prior_score = score;
                }
            })
        });
    }
}
