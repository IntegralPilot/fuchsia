// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fuchsia_inspect as inspect;
use log::{error, info};

/// Step configuration defining charging duration per 1% SOC up to a given threshold.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct PercentChargeDurationStep {
    /// Duration in seconds needed to gain 1% battery charge in this interval.
    pub duration_sec: i32,
    /// Upper bound State of Charge (SOC, 0-100%) for this charging duration step.
    pub soc_threshold: u32,
}

/// Structured battery manager configuration loaded from JSON5 config files.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct BatteryManagerConfig {
    /// Offset percentage (0.0 to 100.0) below which the raw battery level is scaled
    /// to 0% to trigger shutdown before hardware power-off.
    #[serde(default)]
    pub shutdown_offset_percent: f32,

    /// Nominal design capacity of the battery in microampere-hours (uAh).
    #[serde(default)]
    pub design_capacity_uah: Option<i64>,

    /// State of Charge (SOC, 0-100%) thresholds defining Time-To-Full (TTF) charging tiers
    /// (e.g. `[0.0, 51.0, 76.0]`).
    #[serde(default)]
    pub ttf_tier_thresholds: Option<Vec<f32>>,

    /// Temperature boundaries in millicelsius (m°C) defining Time-To-Full (TTF) thermal tiers
    /// (e.g. `[0, 10000, 20000, 42000, 46000, 48000, 55000]`).
    #[serde(default)]
    pub ttf_charge_temp_limits: Option<Vec<i32>>,

    /// Ideal baseline charging duration (seconds per 1% SOC step) up to specified SOC thresholds
    /// used for Time-To-Full (TTF) baseline estimation.
    #[serde(default)]
    pub percent_charge_duration: Option<Vec<PercentChargeDurationStep>>,

    /// 2D matrix of maximum Constant Current (CC) charging limits in microamperes (uA),
    /// indexed by temperature tier (rows) and voltage/SOC tier (columns).
    #[serde(default)]
    pub chg_cc_limits_ua: Option<Vec<Vec<i32>>>,

    /// State of Charge (SOC) offset distance subtracted from the real SOC threshold to
    /// splice the discharge curve when unplugged while full (SSOC = Smoothed/Spoofed SOC).
    #[serde(default)]
    pub ssoc_delta: Option<f32>,

    /// Rate Limiter (RL) maximum allowed SOC change (in percentage points) over the time
    /// window defined by `rl_max_time_s`.
    #[serde(default)]
    pub rl_max_delta_soc: Option<f32>,

    /// Rate Limiter (RL) time window in seconds used in conjunction with `rl_max_delta_soc`
    /// to enforce the maximum rate of SOC change.
    #[serde(default)]
    pub rl_max_time_s: Option<f32>,
}

impl BatteryManagerConfig {
    pub(crate) fn record_inspect(&self, node: &inspect::Node) {
        if let Ok(json) = serde_json5::to_string(self) {
            node.record_string("config", json);
        }
    }

    #[cfg(test)]
    pub(crate) fn default_for_test() -> Self {
        BatteryManagerConfig {
            shutdown_offset_percent: 3.0,
            design_capacity_uah: Some(420_000),
            ttf_tier_thresholds: Some(vec![0.0, 84.0, 90.0]),
            ttf_charge_temp_limits: Some(vec![0, 10_000, 20_000, 42_000, 46_000]),
            percent_charge_duration: Some(vec![
                PercentChargeDurationStep { duration_sec: 32, soc_threshold: 78 },
                PercentChargeDurationStep { duration_sec: 56, soc_threshold: 86 },
                PercentChargeDurationStep { duration_sec: 84, soc_threshold: 96 },
                PercentChargeDurationStep { duration_sec: 92, soc_threshold: 100 },
            ]),
            chg_cc_limits_ua: Some(vec![
                vec![200_000, 100_000, 100_000],
                vec![275_000, 100_000, 100_000],
                vec![500_000, 500_000, 200_000],
                vec![400_000, 400_000, 200_000],
            ]),
            ssoc_delta: Some(2.0),
            rl_max_delta_soc: Some(2.0),
            rl_max_time_s: Some(15.0),
        }
    }
}

pub(crate) fn read_battery_manager_config(path: &str) -> Result<BatteryManagerConfig, Error> {
    info!("Loading battery manager config from {path}");
    let contents = std::fs::read_to_string(path).map_err(|e| {
        let err = anyhow::format_err!(
            "Failed to read battery manager config at '{path}': {e}. \
            Please verify the configuration file path and permissions."
        );
        error!("{err}");
        err
    })?;
    parse_battery_manager_config(&contents, path)
}

fn parse_battery_manager_config(contents: &str, path: &str) -> Result<BatteryManagerConfig, Error> {
    let config: BatteryManagerConfig = serde_json5::from_str(contents).map_err(|e| {
        let err = anyhow::format_err!(
            "Failed to parse battery manager config at '{path}': {e}. \
            Ensure the configuration file contains valid JSON5 matching the \
            BatteryManagerConfig schema."
        );
        error!("{err}");
        err
    })?;

    if config.shutdown_offset_percent.is_nan()
        || config.shutdown_offset_percent < 0.0
        || config.shutdown_offset_percent >= 100.0
    {
        let err = anyhow::format_err!(
            "Invalid battery manager config at '{path}': shutdown_offset_percent ({}) \
            must be in range [0.0, 100.0).",
            config.shutdown_offset_percent
        );
        error!("{err}");
        return Err(err);
    }

    if let Some(cap) = config.design_capacity_uah {
        if cap <= 0 {
            let err = anyhow::format_err!(
                "Invalid battery manager config at '{path}': design_capacity_uah ({cap}) \
                must be positive.",
            );
            error!("{err}");
            return Err(err);
        }
    }

    if let Some(delta) = config.ssoc_delta {
        if delta < 0.0 || delta >= 100.0 {
            let err = anyhow::format_err!(
                "Invalid battery manager config at '{path}': ssoc_delta ({delta}) \
                must be in range [0.0, 100.0).",
            );
            error!("{err}");
            return Err(err);
        }
    }

    if let Some(max_delta) = config.rl_max_delta_soc {
        if max_delta <= 0.0 {
            let err = anyhow::format_err!(
                "Invalid battery manager config at '{path}': rl_max_delta_soc ({max_delta}) \
                must be positive.",
            );
            error!("{err}");
            return Err(err);
        }
    }

    if let Some(max_time) = config.rl_max_time_s {
        if max_time <= 0.0 {
            let err = anyhow::format_err!(
                "Invalid battery manager config at '{path}': rl_max_time_s ({max_time}) \
                must be positive.",
            );
            error!("{err}");
            return Err(err);
        }
    }

    let has_temp_limits = config.ttf_charge_temp_limits.is_some();
    let has_tier_thresholds = config.ttf_tier_thresholds.is_some();
    let has_cc_limits = config.chg_cc_limits_ua.is_some();

    if has_temp_limits || has_tier_thresholds || has_cc_limits {
        if !(has_temp_limits && has_tier_thresholds && has_cc_limits) {
            let err = anyhow::format_err!(
                "Invalid battery manager config at '{path}': \
                ttf_charge_temp_limits, ttf_tier_thresholds, and \
                chg_cc_limits_ua must all be provided together.",
            );
            error!("{err}");
            return Err(err);
        }

        let temp_limits = config.ttf_charge_temp_limits.as_ref().unwrap();
        let tier_thresholds = config.ttf_tier_thresholds.as_ref().unwrap();
        let cc_matrix = config.chg_cc_limits_ua.as_ref().unwrap();

        let expected_rows = temp_limits.len().saturating_sub(1);
        if cc_matrix.len() != expected_rows {
            let err = anyhow::format_err!(
                "Invalid battery manager config at '{path}': \
                chg_cc_limits_ua row count ({}) must match \
                ttf_charge_temp_limits count minus 1 ({}).",
                cc_matrix.len(),
                expected_rows
            );
            error!("{err}");
            return Err(err);
        }

        // TODO(https://fxbug.dev/540034557): When chg_cv_limits_uv is added, validate that
        // chg_cc_limits_ua column count exactly matches chg_cv_limits_uv.len().
        // For now, TTF estimation only requires num_cols >= tier_thresholds.len().
        let expected_min_cols = tier_thresholds.len();
        let num_cols = cc_matrix.first().map(|r| r.len()).unwrap_or(0);
        if num_cols < expected_min_cols {
            let err = anyhow::format_err!(
                "Invalid battery manager config at '{path}': \
                chg_cc_limits_ua column count ({num_cols}) must be at least \
                ttf_tier_thresholds count ({expected_min_cols}).",
            );
            error!("{err}");
            return Err(err);
        }

        for (i, row) in cc_matrix.iter().enumerate() {
            if row.len() != num_cols {
                let err = anyhow::format_err!(
                    "Invalid battery manager config at '{path}': \
                    chg_cc_limits_ua row {i} length ({}) must match \
                    row 0 length ({num_cols}).",
                    row.len()
                );
                error!("{err}");
                return Err(err);
            }
        }
    }

    if let Some(ref steps) = config.percent_charge_duration {
        if steps.is_empty() {
            let err = anyhow::format_err!(
                "Invalid battery manager config at '{path}': \
                percent_charge_duration cannot be empty.",
            );
            error!("{err}");
            return Err(err);
        }

        let mut prev_threshold = None;
        for (i, step) in steps.iter().enumerate() {
            if step.duration_sec < 0 {
                let err = anyhow::format_err!(
                    "Invalid battery manager config at '{path}': \
                    percent_charge_duration step {i} duration_sec ({}) \
                    must be non-negative.",
                    step.duration_sec
                );
                error!("{err}");
                return Err(err);
            }

            if let Some(prev) = prev_threshold {
                if step.soc_threshold <= prev {
                    let err = anyhow::format_err!(
                        "Invalid battery manager config at '{path}': \
                        percent_charge_duration step {i} soc_threshold ({}) \
                        must be strictly greater than previous threshold ({prev}).",
                        step.soc_threshold
                    );
                    error!("{err}");
                    return Err(err);
                }
            }
            prev_threshold = Some(step.soc_threshold);
        }

        if let Some(last_step) = steps.last() {
            if last_step.soc_threshold < 100 {
                let err = anyhow::format_err!(
                    "Invalid battery manager config at '{path}': \
                    percent_charge_duration last soc_threshold ({}) \
                    must be at least 100.",
                    last_step.soc_threshold
                );
                error!("{err}");
                return Err(err);
            }
        }
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_battery_manager_config_valid() {
        let json = r#"{
            "shutdown_offset_percent": 3.0
        }"#;
        let config = parse_battery_manager_config(json, "test_path").unwrap();
        assert_eq!(config.shutdown_offset_percent, 3.0);
    }

    #[test]
    fn test_parse_battery_manager_config_json5_features() {
        // Test JSON5 comments, trailing commas, and unquoted keys
        let json5 = r#"{
            // Comment line
            shutdown_offset_percent: 3.0,
            /* Multi-line
               comment */
            ttf_tier_thresholds: [0.0, 84.0, 90.0,],
            ttf_charge_temp_limits: [0, 10000, 20000, 42000, 46000,],
            chg_cc_limits_ua: [
                [200000, 100000, 100000,],
                [275000, 100000, 100000,],
                [500000, 500000, 200000,],
                [400000, 400000, 200000,],
            ],
        }"#;
        let config = parse_battery_manager_config(json5, "test_path").unwrap();
        assert_eq!(config.shutdown_offset_percent, 3.0);
        assert_eq!(config.ttf_tier_thresholds, Some(vec![0.0, 84.0, 90.0]));
        assert_eq!(config.ttf_charge_temp_limits, Some(vec![0, 10000, 20000, 42000, 46000]));
        assert_eq!(
            config.chg_cc_limits_ua,
            Some(vec![
                vec![200_000, 100_000, 100_000],
                vec![275_000, 100_000, 100_000],
                vec![500_000, 500_000, 200_000],
                vec![400_000, 400_000, 200_000],
            ])
        );
    }

    #[test]
    fn test_parse_battery_manager_config_default() {
        let json = r#"{}"#;
        let config = parse_battery_manager_config(json, "test_path").unwrap();
        assert_eq!(config.shutdown_offset_percent, 0.0);
    }

    #[test]
    fn test_parse_battery_manager_config_invalid_json() {
        let json = r#"{"shutdown_offset_percent": invalid_value}"#;
        let err = parse_battery_manager_config(json, "test_path").unwrap_err();
        assert!(err.to_string().contains("Failed to parse battery manager config"));
    }

    #[test]
    fn test_parse_battery_manager_config_out_of_range() {
        let json = r#"{"shutdown_offset_percent": -5.0}"#;
        let err = parse_battery_manager_config(json, "test_path").unwrap_err();
        assert!(err.to_string().contains("must be in range"));

        let json = r#"{"shutdown_offset_percent": 100.0}"#;
        let err = parse_battery_manager_config(json, "test_path").unwrap_err();
        assert!(err.to_string().contains("must be in range"));
    }

    #[test]
    fn test_parse_battery_manager_config_partial_cc_limits() {
        let json = r#"{
            "ttf_tier_thresholds": [0.0, 84.0, 90.0]
        }"#;
        let err = parse_battery_manager_config(json, "test_path").unwrap_err();
        assert!(err.to_string().contains("must all be provided together"));
    }

    #[test]
    fn test_parse_battery_manager_config_cc_limits_with_extra_columns() {
        // Valid matrix with more columns (4) than ttf_tier_thresholds (2)
        let json = r#"{
            "ttf_charge_temp_limits": [0, 10000, 20000],
            "ttf_tier_thresholds": [0.0, 84.0],
            "chg_cc_limits_ua": [
                [200000, 100000, 50000, 25000],
                [200000, 100000, 50000, 25000]
            ]
        }"#;
        let config = parse_battery_manager_config(json, "test_path").unwrap();
        let cc_limits = config.chg_cc_limits_ua.unwrap();
        assert_eq!(cc_limits.len(), 2);
        assert_eq!(cc_limits[0].len(), 4);
        assert_eq!(cc_limits[1].len(), 4);
    }

    #[test]
    fn test_parse_battery_manager_config_mismatched_cc_limits() {
        // Mismatched row count (temp_limits has 3 entries expecting 2 rows, but chg_cc_limits_ua
        // has 1 row)
        let json_rows = r#"{
            "ttf_charge_temp_limits": [0, 10000, 20000],
            "ttf_tier_thresholds": [0.0, 84.0],
            "chg_cc_limits_ua": [
                [200000, 100000]
            ]
        }"#;
        let err = parse_battery_manager_config(json_rows, "test_path").unwrap_err();
        assert!(err.to_string().contains("row count"));

        // Column count less than ttf_tier_thresholds (1 col < 2 tier thresholds)
        let json_fewer_cols = r#"{
            "ttf_charge_temp_limits": [0, 10000, 20000],
            "ttf_tier_thresholds": [0.0, 84.0],
            "chg_cc_limits_ua": [
                [200000],
                [200000]
            ]
        }"#;
        let err = parse_battery_manager_config(json_fewer_cols, "test_path").unwrap_err();
        assert!(err.to_string().contains("column count"));

        // Non-uniform row lengths (row 1 has 2 cols, row 0 has 3 cols)
        let json_ragged_cols = r#"{
            "ttf_charge_temp_limits": [0, 10000, 20000],
            "ttf_tier_thresholds": [0.0, 84.0],
            "chg_cc_limits_ua": [
                [200000, 100000, 50000],
                [200000, 100000]
            ]
        }"#;
        let err = parse_battery_manager_config(json_ragged_cols, "test_path").unwrap_err();
        assert!(err.to_string().contains("row 1 length"));
    }

    #[test]
    fn test_parse_battery_manager_config_design_capacity() {
        let json = r#"{"design_capacity_uah": 420000}"#;
        let config = parse_battery_manager_config(json, "test_path").unwrap();
        assert_eq!(config.design_capacity_uah, Some(420000));

        let json = r#"{"design_capacity_uah": -100}"#;
        let err = parse_battery_manager_config(json, "test_path").unwrap_err();
        assert!(err.to_string().contains("must be positive"));
    }

    #[test]
    fn test_parse_battery_manager_config_percent_charge_duration_valid() {
        let json = r#"{
            "percent_charge_duration": [
                {"duration_sec": 32, "soc_threshold": 78},
                {"duration_sec": 56, "soc_threshold": 86},
                {"duration_sec": 84, "soc_threshold": 96},
                {"duration_sec": 92, "soc_threshold": 100}
            ]
        }"#;
        let config = parse_battery_manager_config(json, "test_path").unwrap();
        assert_eq!(config.percent_charge_duration.unwrap().len(), 4);
    }

    #[test]
    fn test_parse_battery_manager_config_percent_charge_duration_invalid() {
        let json_empty = r#"{"percent_charge_duration": []}"#;
        let err = parse_battery_manager_config(json_empty, "test_path").unwrap_err();
        assert!(err.to_string().contains("cannot be empty"));

        let json_neg_dur = r#"{
            "percent_charge_duration": [
                {"duration_sec": -10, "soc_threshold": 100}
            ]
        }"#;
        let err = parse_battery_manager_config(json_neg_dur, "test_path").unwrap_err();
        assert!(err.to_string().contains("must be non-negative"));

        let json_not_increasing = r#"{
            "percent_charge_duration": [
                {"duration_sec": 30, "soc_threshold": 80},
                {"duration_sec": 40, "soc_threshold": 75},
                {"duration_sec": 50, "soc_threshold": 100}
            ]
        }"#;
        let err = parse_battery_manager_config(json_not_increasing, "test_path").unwrap_err();
        assert!(err.to_string().contains("strictly greater than"));

        let json_incomplete = r#"{
            "percent_charge_duration": [
                {"duration_sec": 30, "soc_threshold": 80},
                {"duration_sec": 40, "soc_threshold": 99}
            ]
        }"#;
        let err = parse_battery_manager_config(json_incomplete, "test_path").unwrap_err();
        assert!(err.to_string().contains("must be at least 100"));
    }

    #[fuchsia::test]
    async fn test_record_inspect() {
        use diagnostics_assertions::assert_data_tree;
        use fuchsia_inspect::Inspector;

        let inspector = Inspector::default();
        let config = BatteryManagerConfig::default_for_test();
        inspector.root().record_child("battery_manager_config", |node| {
            config.record_inspect(node);
        });

        let expected_json = serde_json5::to_string(&config).unwrap();
        assert_data_tree!(inspector, root: {
            battery_manager_config: {
                config: expected_json,
            }
        });
    }
}
