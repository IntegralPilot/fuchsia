// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::roaming::lib::*;
use crate::client::roaming::roam_monitor::{RoamMonitorApi, RoamTriggerDataOutcome};
use anyhow::format_err;
use log::error;

pub struct DefaultRoamMonitor {}

impl Default for DefaultRoamMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultRoamMonitor {
    pub fn new() -> Self {
        Self {}
    }
}

use async_trait::async_trait;
#[async_trait(?Send)]
impl RoamMonitorApi for DefaultRoamMonitor {
    async fn handle_roam_trigger_data(
        &mut self,
        _data: RoamTriggerData,
    ) -> Result<RoamTriggerDataOutcome, anyhow::Error> {
        // Default response to noop. Metrics for default devices can be added here.
        Ok(RoamTriggerDataOutcome::Noop)
    }
    fn should_send_roam_request(&self, request: PolicyRoamRequest) -> Result<bool, anyhow::Error> {
        Err(format_err!(
            "Default roam monitor unexpectedly received a roam candidate: {}, roam reasons: {:?}",
            request.candidate.to_string_without_pii(),
            request.reasons
        ))
    }
    fn notify_of_roam_attempt(&mut self) {
        error!("Default roam monitor unexpectedly receieved notification of roam attempt");
    }
}
#[cfg(test)]
mod test {
    use super::*;
    use crate::util::testing::generate_random_scanned_candidate;
    use assert_matches::assert_matches;
    use fidl_fuchsia_wlan_internal as fidl_internal;

    #[fuchsia::test]
    async fn test_handle_roam_trigger_data_always_returns_noop() {
        let mut monitor = DefaultRoamMonitor::new();

        // Send each type of trigger data and verify the default monitor always returns noop.
        assert_matches!(
            monitor
                .handle_roam_trigger_data(RoamTriggerData::SignalReportInd(
                    fidl_internal::SignalReportIndication {
                        rssi_dbm: -100,
                        snr_db: 0,
                        tx_rate_500kbps: 0
                    },
                ))
                .await,
            Ok(RoamTriggerDataOutcome::Noop)
        );
    }

    #[fuchsia::test]
    async fn test_should_send_roam_request_returns_error() {
        let monitor = DefaultRoamMonitor::new();

        // Send a candidate and verify an error is returned
        let candidate = generate_random_scanned_candidate();
        assert_matches!(
            monitor.should_send_roam_request(PolicyRoamRequest { candidate, reasons: vec![] }),
            Err(_)
        );
    }
}
