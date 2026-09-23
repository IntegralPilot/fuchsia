// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::Mutex;
use ieee80211::Bssid;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use wlan_common::bss::BssDescription;
use {fidl_fuchsia_wlan_sme as fidl_sme, fuchsia_async as fasync};

/// Only connect failures that were RECENT_CONNECT_FAILURE_WINDOW from now would
/// contribute to score penalty.
const RECENT_CONNECT_FAILURE_WINDOW: zx::MonotonicDuration =
    zx::MonotonicDuration::from_seconds(60 * 5);

/// The amount to decrease the score by for each failed connection attempt.
const SCORE_PENALTY_FOR_RECENT_CONNECT_FAILURE: i16 = 5;
/// Excessive recent connect failures warrant a higher penalty.
const THRESHOLD_EXCESSIVE_RECENT_CONNECT_FAILURES: usize = 5;
const SCORE_PENALTY_FOR_EXCESSIVE_RECENT_CONNECT_FAILURES: i16 = 10;
/// This penalty is much higher than for a general failure because we are not likely to succeed
/// on a retry.
const SCORE_PENALTY_FOR_RECENT_CREDENTIAL_REJECTED: i16 = 30;

#[derive(Debug)]
struct RecentConnectFailure {
    timestamp: fasync::MonotonicInstant,
    is_credential_rejected: bool,
}

/// BssScorer implements a subset of the scoring algorithm in wlancfg.
#[derive(Debug)]
pub(crate) struct BssScorer {
    inner: Arc<Mutex<BssScorerInner>>,
}

impl BssScorer {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(BssScorerInner { recent_connect_failures: HashMap::new() })),
        }
    }

    pub fn report_connect_failure(&self, bssid: Bssid, connect_result: &fidl_sme::ConnectResult) {
        self.inner.lock().report_connect_failure(bssid, connect_result)
    }

    pub fn score_bss(&self, bss_description: &BssDescription) -> i16 {
        self.inner.lock().score_bss(bss_description)
    }
}

#[derive(Debug, Default)]
struct BssScorerInner {
    recent_connect_failures: HashMap<Bssid, VecDeque<RecentConnectFailure>>,
}

impl BssScorerInner {
    pub fn report_connect_failure(
        &mut self,
        bssid: Bssid,
        connect_result: &fidl_sme::ConnectResult,
    ) {
        let now = fasync::MonotonicInstant::now();
        let failure = RecentConnectFailure {
            timestamp: now,
            is_credential_rejected: connect_result.is_credential_rejected,
        };
        self.recent_connect_failures.entry(bssid).or_default().push_back(failure);
    }

    pub fn score_bss(&mut self, bss_description: &BssDescription) -> i16 {
        let penalty = self.compute_connect_failure_penalty(bss_description.bssid);
        (bss_description.rssi_dbm as i16).saturating_sub(penalty)
    }

    fn compute_connect_failure_penalty(&mut self, bssid: Bssid) -> i16 {
        let mut penalty: i16 = 0;
        if let Some(recent_connect_failures) = self.recent_connect_failures.get_mut(&bssid) {
            let now = fasync::MonotonicInstant::now();
            // Remove connect failures that are no longer recent
            while let Some(failure) = recent_connect_failures.front() {
                if failure.timestamp >= now - RECENT_CONNECT_FAILURE_WINDOW {
                    break;
                }
                let _failure = recent_connect_failures.pop_front();
            }

            let mut non_credential_failure_count: usize = 0;
            for failure in recent_connect_failures {
                if failure.is_credential_rejected {
                    penalty = penalty.saturating_add(SCORE_PENALTY_FOR_RECENT_CREDENTIAL_REJECTED)
                } else {
                    non_credential_failure_count += 1;
                    if non_credential_failure_count <= THRESHOLD_EXCESSIVE_RECENT_CONNECT_FAILURES {
                        penalty = penalty.saturating_add(SCORE_PENALTY_FOR_RECENT_CONNECT_FAILURE);
                    } else {
                        penalty = penalty
                            .saturating_add(SCORE_PENALTY_FOR_EXCESSIVE_RECENT_CONNECT_FAILURES);
                    }
                }
            }
        }
        penalty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
    use wlan_common::fake_bss_description;

    const BSSID: [u8; 6] = [1, 3, 3, 7, 4, 2];

    fn setup_test() -> (fasync::TestExecutor, BssScorer) {
        let exec = fasync::TestExecutor::new_with_fake_time();
        let bss_scorer = BssScorer::new();
        (exec, bss_scorer)
    }

    fn connect_failure() -> fidl_sme::ConnectResult {
        fidl_sme::ConnectResult {
            code: fidl_ieee80211::StatusCode::RejectedSequenceTimeout,
            is_credential_rejected: false,
            is_reconnect: false,
        }
    }

    fn fake_bss(rssi_dbm: i8) -> BssDescription {
        fake_bss_description!(Open, bssid: BSSID, rssi_dbm: rssi_dbm)
    }

    #[test]
    fn test_score_without_penalty() {
        let (_exec, bss_scorer) = setup_test();
        let score = bss_scorer.score_bss(&fake_bss(-50));
        assert_eq!(score, -50);
    }

    #[test]
    fn test_score_with_one_connect_failure() {
        let (_exec, bss_scorer) = setup_test();
        bss_scorer.report_connect_failure(Bssid::from(BSSID), &connect_failure());
        let score = bss_scorer.score_bss(&fake_bss(-50));
        assert_eq!(score, -55);
    }

    #[test]
    fn test_score_with_one_recent_credential_rejected_connect_failure() {
        let (_exec, bss_scorer) = setup_test();
        bss_scorer.report_connect_failure(
            Bssid::from(BSSID),
            &fidl_sme::ConnectResult { is_credential_rejected: true, ..connect_failure() },
        );
        let score = bss_scorer.score_bss(&fake_bss(-50));
        assert_eq!(score, -80);
    }

    #[test]
    fn test_score_with_excessive_connect_failures() {
        let (_exec, bss_scorer) = setup_test();
        for _i in 0..8 {
            bss_scorer.report_connect_failure(Bssid::from(BSSID), &connect_failure());
        }
        let score = bss_scorer.score_bss(&fake_bss(-50));
        assert_eq!(score, -105);
    }

    #[test]
    fn test_connect_failure_eviction() {
        let (exec, bss_scorer) = setup_test();
        // Two connect failures at the 0th second mark.
        bss_scorer.report_connect_failure(Bssid::from(BSSID), &connect_failure());
        bss_scorer.report_connect_failure(Bssid::from(BSSID), &connect_failure());
        exec.set_fake_time(fasync::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(
            300,
        )));
        assert_eq!(bss_scorer.score_bss(&fake_bss(-50)), -60);
        bss_scorer.report_connect_failure(Bssid::from(BSSID), &connect_failure());
        // At 300th second mark, three connect failures are considered as recent.
        assert_eq!(bss_scorer.score_bss(&fake_bss(-50)), -65);

        exec.set_fake_time(fasync::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(1)));
        // At 301th second mark, the two connect failures from the 0th second mark are
        // evicted, leaving one recent connect failure.
        assert_eq!(bss_scorer.score_bss(&fake_bss(-50)), -55);
    }

    #[test]
    fn test_connect_failure_penalty_overflow() {
        let (_exec, bss_scorer) = setup_test();
        for _i in 0..1100 {
            bss_scorer.report_connect_failure(
                Bssid::from(BSSID),
                &fidl_sme::ConnectResult { is_credential_rejected: true, ..connect_failure() },
            );
        }
        assert_eq!(bss_scorer.score_bss(&fake_bss(-50)), i16::MIN);
    }
}
