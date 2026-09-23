// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeviceMobility {
    #[default]
    Mobile,
    Stationary,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TelemetryConfig {
    pub enable_connect_disconnect: bool,
    pub enable_iface_logger: bool,
    pub enable_power_logger: bool,
    pub enable_recovery_logger: bool,
    pub enable_scan_logger: bool,
    pub enable_pno_scan_logger: bool,
    pub enable_sme_timeout_logger: bool,
    pub enable_toggle_logger: bool,
    pub enable_tx_power_scenario_logger: bool,
    pub enable_client_iface_counters_logger: bool,
    pub device_mobility: DeviceMobility,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CobaltAllowlist {
    All,
    Only(HashSet<u32>),
}

impl CobaltAllowlist {
    pub fn contains(&self, metric_id: u32) -> bool {
        match self {
            CobaltAllowlist::All => true,
            CobaltAllowlist::Only(set) => set.contains(&metric_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_telemetry_config_default() {
        let config = TelemetryConfig::default();
        assert_eq!(config.device_mobility, DeviceMobility::Mobile);
    }
}
