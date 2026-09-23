// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fpower;
use log::{debug, info, warn};

/// Polisher goes through the following stages of battery info processing to obtain better battery
//  level, level status, time to full estimation and so on.
///
/// Data flow:
///                      Raw Data of Battery Level
///                                  |
///                                  V
///                   Initial Scaler: 3-100% => 0-100%
///                                  |
///                                  V
///                               Filters
///                                  |
///                                  |=====> Time to Full
///                                  |
///                                  V
///                        Spoofing and Splicing
///                                  |
///                                  V
///                           rate limiter
///                                  |
///                                  V
///                  Reported to upper level, displayed

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TimeEstimatorError {
    InvalidRange,
    MissingCurrent,
    NonPositiveCurrent,
    MissingConfig,
}

struct InitialScaler;

impl InitialScaler {
    fn scale_level_with_offset(level: f32, offset: f32) -> f32 {
        if level >= 100.0 {
            return 100.0;
        }
        if offset <= 0.0 || offset >= 100.0 {
            return level.ceil();
        }
        if level >= offset {
            let scaled = (level - offset) * 100.0 / (100.0 - offset);
            return scaled.ceil();
        } else {
            return 0.0;
        }
    }
}

// Size of lookup table from 0% to 99%.
const LOOKUP_TABLE_SIZE: usize = 100;

struct ChargeTimeEstimator {
    baseline_duration_lookup: [i32; LOOKUP_TABLE_SIZE],
    // If true, use actual capacity to calculate time to full.
    // If false, use design capacity to calculate time to full.
    use_actual_capacity: bool,
    actual_capacity_uah: Option<i32>,
    config: crate::BatteryManagerConfig,

    // Average current tracking
    current_tier: Option<usize>,
    tier_accumulated_current_ua_ms: i64,
    tier_accumulated_time_ms: i64,
    last_update_timestamp_ns: Option<i64>,
}

impl ChargeTimeEstimator {
    // TODO(https://fxbug.dev/442619993): evaluate the use of actual capacity instead.
    fn get_delta_cc_uah(&self) -> Result<i64, TimeEstimatorError> {
        if self.use_actual_capacity {
            if let Some(cap) = self.actual_capacity_uah {
                return Ok(cap as i64 / 100);
            }
        }
        let design_cap =
            self.config.design_capacity_uah.ok_or(TimeEstimatorError::MissingConfig)?;
        Ok(design_cap / 100)
    }

    fn get_reference_current_ua(
        &self,
        level_percent: f32,
        temperature_mc: Option<i32>,
    ) -> Result<i32, TimeEstimatorError> {
        let limits =
            self.config.ttf_charge_temp_limits.as_ref().ok_or(TimeEstimatorError::MissingConfig)?;
        let thresholds =
            self.config.ttf_tier_thresholds.as_ref().ok_or(TimeEstimatorError::MissingConfig)?;
        let limits_matrix =
            self.config.chg_cc_limits_ua.as_ref().ok_or(TimeEstimatorError::MissingConfig)?;

        let temp_mc = temperature_mc.unwrap_or(25_000);

        let row_idx = limits
            .iter()
            .skip(1)
            .position(|&limit| temp_mc < limit)
            .unwrap_or_else(|| limits_matrix.len().saturating_sub(1));

        let col_idx = thresholds
            .iter()
            .skip(1)
            .position(|&threshold| level_percent < threshold)
            .unwrap_or_else(|| thresholds.len().saturating_sub(1));

        if let Some(row) = limits_matrix.get(row_idx) {
            if let Some(&val) = row.get(col_idx) {
                return Ok(val);
            }
        }

        Err(TimeEstimatorError::MissingConfig)
    }

    #[cfg(test)]
    pub fn get_static_reference_current_ua(
        level_percent: f32,
        temperature_mc: Option<i32>,
    ) -> Result<i32, TimeEstimatorError> {
        Self::new(false).get_reference_current_ua(level_percent, temperature_mc)
    }

    fn new_with_config(
        use_actual_capacity: bool,
        config: crate::BatteryManagerConfig,
    ) -> ChargeTimeEstimator {
        let mut table = [0i32; LOOKUP_TABLE_SIZE];
        let mut percent_start = 0;
        if let Some(ref steps) = config.percent_charge_duration {
            for step in steps {
                let end = step.soc_threshold.min((LOOKUP_TABLE_SIZE - 1).try_into().unwrap());
                for percent in percent_start..=end {
                    table[percent as usize] = step.duration_sec;
                }

                percent_start = end + 1;
                if percent_start >= LOOKUP_TABLE_SIZE as u32 {
                    break;
                }
            }
        }

        ChargeTimeEstimator {
            baseline_duration_lookup: table,
            use_actual_capacity,
            actual_capacity_uah: None,
            config,
            current_tier: None,
            tier_accumulated_current_ua_ms: 0,
            tier_accumulated_time_ms: 0,
            last_update_timestamp_ns: None,
        }
    }

    #[cfg(test)]
    fn new(use_actual_capacity: bool) -> ChargeTimeEstimator {
        Self::new_with_config(use_actual_capacity, crate::BatteryManagerConfig::default_for_test())
    }

    fn set_actual_capacity(&mut self, actual_capacity_uah: Option<i32>) {
        self.actual_capacity_uah = actual_capacity_uah;
    }

    fn get_tier_index(&self, level_percent: f32) -> Result<usize, TimeEstimatorError> {
        let thresholds =
            self.config.ttf_tier_thresholds.as_ref().ok_or(TimeEstimatorError::MissingConfig)?;
        if thresholds.len() > 1 {
            Ok(thresholds[1..].partition_point(|&threshold| threshold <= level_percent))
        } else {
            Ok(0)
        }
    }

    fn update_average_current(
        &mut self,
        level: f32,
        actual_current_ua: Option<i32>,
        timestamp_ns: Option<i64>,
        charge_status: Option<fpower::ChargeStatus>,
    ) -> Option<i32> {
        let timestamp_ns = timestamp_ns?;
        let tier_idx = self.get_tier_index(level).ok()?;

        if self.current_tier != Some(tier_idx) {
            self.current_tier = Some(tier_idx);
            self.tier_accumulated_current_ua_ms = 0;
            self.tier_accumulated_time_ms = 0;
        }

        if charge_status == Some(fpower::ChargeStatus::Charging) {
            if let Some(last_ts) = self.last_update_timestamp_ns {
                let dt_ns = timestamp_ns - last_ts;
                if dt_ns > 0 {
                    if let Some(current) = actual_current_ua {
                        let dt_ms =
                            std::time::Duration::from_nanos(dt_ns as u64).as_millis() as i64;
                        self.tier_accumulated_time_ms += dt_ms;
                        let ms_current = current as i64 * dt_ms;
                        self.tier_accumulated_current_ua_ms += ms_current;
                    }
                }
            }
            self.last_update_timestamp_ns = Some(timestamp_ns);
        } else {
            self.last_update_timestamp_ns = None;
        }

        let sixty_secs_ms = std::time::Duration::from_secs(60).as_millis() as i64;
        if self.tier_accumulated_time_ms >= sixty_secs_ms {
            let time_ms = self.tier_accumulated_time_ms;
            if time_ms > 0 {
                let avg = (self.tier_accumulated_current_ua_ms / time_ms) as i32;
                return Some(avg);
            }
        }

        None
    }

    fn reset_average_current(&mut self) {
        self.current_tier = None;
        self.tier_accumulated_current_ua_ms = 0;
        self.tier_accumulated_time_ms = 0;
        self.last_update_timestamp_ns = None;
    }

    // Calculate the implied reference current (uA) for a given SOC level.
    fn get_implied_ref_current_ua(&self, level: u32) -> Result<i32, TimeEstimatorError> {
        let base_elap = self.get_level_duration(level)?;
        if base_elap == 0 {
            return Ok(0);
        }

        let delta_cc = self.get_delta_cc_uah()?;
        Ok(((delta_cc * 3600) / (base_elap as i64)) as i32)
    }

    /// Calculates the time to full for the range [from_soc, to_soc].
    fn time_to_full(
        &self,
        from_soc: f32,
        to_soc: f32,
        actual_current_ua: Option<i32>,
        temperature_mc: Option<i32>,
    ) -> Result<zx::BootDuration, TimeEstimatorError> {
        if to_soc > 100.0 || to_soc < from_soc {
            return Err(TimeEstimatorError::InvalidRange);
        }
        if to_soc == from_soc {
            return Ok(zx::Duration::from_seconds(0));
        }

        // If both from_soc and to_soc fall within the same integer percentage (e.g. 99.2 to 99.8)
        if from_soc.floor() == to_soc.floor() {
            let ratio = self.ttf_current_ratio(actual_current_ua, from_soc, temperature_mc)?;
            let elap = self.ttf_elap_estimate_step(from_soc.floor() as u32, ratio)?;
            let estimate_s = elap * (to_soc - from_soc);
            return Ok(zx::Duration::from_seconds(estimate_s as i64));
        }

        let mut estimate_s = 0.0_f32;

        // FIRST: fraction part of from_soc if any
        let from_soc_int = from_soc.floor() as u32;
        let from_soc_frac = from_soc.fract();
        let mut i = from_soc_int;
        if from_soc_frac > 0.0 {
            let ratio = self.ttf_current_ratio(actual_current_ua, from_soc, temperature_mc)?;
            let elap = self.ttf_elap_estimate_step(i, ratio)?;
            estimate_s += elap * (1.0 - from_soc_frac);
            i += 1;
        }

        // accumulate ttf_elap_estimate_step starting from i until end
        let last_int = to_soc.floor() as u32;
        while i < last_int {
            let ratio = self.ttf_current_ratio(actual_current_ua, i as f32, temperature_mc)?;
            let elap = self.ttf_elap_estimate_step(i, ratio)?;
            estimate_s += elap;
            i += 1;
        }

        // LAST: fraction of to_soc if any
        let to_soc_frac = to_soc.fract();
        if to_soc_frac > 0.0 {
            let ratio = self.ttf_current_ratio(actual_current_ua, to_soc, temperature_mc)?;
            let elap = self.ttf_elap_estimate_step(last_int, ratio)?;
            estimate_s += elap * to_soc_frac;
        }

        debug!("actual_current: {:?}, estimate_s: {:?}", actual_current_ua, estimate_s);
        Ok(zx::Duration::from_seconds(estimate_s as i64))
    }

    // Predict the time in seconds needed to charge by 1% according to the lookup table.
    fn get_level_duration(&self, level: u32) -> Result<i32, TimeEstimatorError> {
        let level = level as usize;
        if level >= LOOKUP_TABLE_SIZE {
            return Err(TimeEstimatorError::InvalidRange);
        }
        if self.config.percent_charge_duration.is_none() {
            return Err(TimeEstimatorError::MissingConfig);
        }
        Ok(self.baseline_duration_lookup[level])
    }

    /// Calculates the power ratio used to scale time-to-full estimations.
    ///
    /// # Errors
    /// Returns `MissingCurrent` if `actual_current_ua` is `None`.
    /// Returns `NonPositiveCurrent` if `actual_current_ua <= 0`.
    fn ttf_current_ratio(
        &self,
        actual_current_ua: Option<i32>,
        level_percent: f32,
        temperature_mc: Option<i32>,
    ) -> Result<f32, TimeEstimatorError> {
        let actual_current = actual_current_ua.ok_or(TimeEstimatorError::MissingCurrent)?;

        if actual_current <= 0 {
            return Err(TimeEstimatorError::NonPositiveCurrent);
        }

        let level_int = level_percent.floor() as u32;

        // Calculate ref_cc_ua (Implied Reference Current)
        let ref_cc_ua = self.get_implied_ref_current_ua(level_int)?;
        if ref_cc_ua <= 0 {
            return Ok(1.0);
        }

        // Get cc_max_ua (Maximum Allowed Current)
        let cc_max_ua = self.get_reference_current_ua(level_percent, temperature_mc)?;

        // Determine equiv_icl_ua (Effective Expected Current)
        // TODO(https://fxbug.dev/442619993): Consider fast charging which could be higher than cc_max_ua.
        let equiv_icl_ua = actual_current.min(cc_max_ua);
        if equiv_icl_ua <= 0 {
            return Err(TimeEstimatorError::NonPositiveCurrent);
        }

        // Calculate ratio
        if equiv_icl_ua < ref_cc_ua { Ok(ref_cc_ua as f32 / equiv_icl_ua as f32) } else { Ok(1.0) }
    }

    /// Calculates the elapsed time to charge a single 1% SOC step.
    fn ttf_elap_estimate_step(&self, level: u32, ratio: f32) -> Result<f32, TimeEstimatorError> {
        let base_elap = self.get_level_duration(level)? as f32;
        Ok(base_elap * ratio)
    }
}

// Determine the LevelStatus
struct LevelChecker;

impl LevelChecker {
    // Used to determine the level_status, after scale_level
    const THRESHOLD_LEVEL_OK: f32 = 80.0;
    const THRESHOLD_LEVEL_WARNING: f32 = 40.0;
    const THRESHOLD_LEVEL_LOW: f32 = 0.0;

    fn determine_level_status(
        level: f32,
        charger_status: Option<fpower::ChargeStatus>,
    ) -> fpower::LevelStatus {
        if level > Self::THRESHOLD_LEVEL_OK {
            return fpower::LevelStatus::Ok;
        } else if level > Self::THRESHOLD_LEVEL_WARNING {
            return fpower::LevelStatus::Warning;
        } else if level > Self::THRESHOLD_LEVEL_LOW {
            return fpower::LevelStatus::Low;
        } else if charger_status == Some(fpower::ChargeStatus::NotCharging)
            || charger_status == Some(fpower::ChargeStatus::Discharging)
        {
            return fpower::LevelStatus::Critical;
        } else if charger_status == Some(fpower::ChargeStatus::Unknown) || charger_status == None {
            return fpower::LevelStatus::Unknown;
        } else {
            return fpower::LevelStatus::Low;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CurvePoint {
    real: f32,
    ui: f32,
}

impl CurvePoint {
    fn interpolate(p1: &Self, p2: &Self, x: f32) -> f32 {
        let dx = p2.real - p1.real;
        if dx == 0.0 {
            return p1.ui;
        }

        let slope = (p2.ui - p1.ui) / dx;
        p1.ui + slope * (x - p1.real)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlugTransition {
    PluggedIn,
    Unplugged,
    None,
}

struct CurveMapper {
    current_curve: [CurvePoint; CurveMapper::UICURVE_MAX],
}

impl CurveMapper {
    const UICURVE_MAX: usize = 3;

    // SSOC_TRUE: the point below which all "spoofing" is disabled.
    // SSOC_SPOOF: default threshold where real SOC starts mapping towards 100% UI.
    const SSOC_TRUE: f32 = 15.0;
    const SSOC_SPOOF: f32 = 95.0;
    const SSOC_FULL: f32 = 100.0;
    // This is now the default value of ssoc_delta if not provided by the config.
    const SSOC_DELTA_DEFAULT: f32 = 3.0;

    const CHG_CURVE_DEFAULT: [CurvePoint; Self::UICURVE_MAX] = [
        CurvePoint { real: Self::SSOC_TRUE, ui: Self::SSOC_TRUE },
        CurvePoint { real: Self::SSOC_SPOOF, ui: Self::SSOC_SPOOF },
        CurvePoint { real: Self::SSOC_FULL, ui: Self::SSOC_FULL },
    ];

    const DISCHARGE_CURVE_DEFAULT: [CurvePoint; Self::UICURVE_MAX] = [
        CurvePoint { real: Self::SSOC_TRUE, ui: Self::SSOC_TRUE },
        CurvePoint { real: Self::SSOC_SPOOF, ui: Self::SSOC_FULL },
        CurvePoint { real: Self::SSOC_FULL, ui: Self::SSOC_FULL },
    ];

    pub fn new() -> CurveMapper {
        CurveMapper {
            current_curve: Self::CHG_CURVE_DEFAULT, // Start with CHG curve as default
        }
    }

    /// Maps the real battery level (scaled real SoC) to the UI level using the current curve.
    ///
    /// # Arguments
    /// * `real` - The real battery level (scaled real SoC) to map.
    fn ssoc_uicurve_map(&self, real: f32) -> f32 {
        // Destructure the array into the three points we know exist
        let [p_left, p_mid, p_right] = self.current_curve;

        if real < p_left.real {
            real
        } else if real < p_mid.real {
            CurvePoint::interpolate(&p_left, &p_mid, real)
        } else if real < p_right.real {
            CurvePoint::interpolate(&p_mid, &p_right, real)
        } else {
            p_right.ui
        }
    }

    /// Sets the midpoint of the curve (index 1).
    ///
    /// This is used to hold the UI at 100% or taper off on disconnect.
    ///
    /// # Arguments
    /// * `curve` - The curve to modify.
    /// * `real` - The real battery level for the midpoint.
    /// * `ui` - The UI battery level for the midpoint.
    fn set_midpoint(curve: &mut [CurvePoint; Self::UICURVE_MAX], real: f32, ui: f32) {
        if real < curve[0].real || real > curve[2].real {
            return;
        }
        curve[1].real = real;
        curve[1].ui = ui;
    }

    /// Updates the current curve based on connection state changes.
    ///
    /// # Arguments
    /// * `transition` - The plug transition state since last update.
    /// * `scaled_real_soc` - The current scaled real SoC.
    /// * `current_ui_soc` - The current UI SoC.
    pub fn update_curve_state(
        &mut self,
        transition: PlugTransition,
        scaled_real_soc: f32,
        current_ui_soc: f32,
        ssoc_delta_config: Option<f32>,
    ) {
        let ssoc_delta = ssoc_delta_config.unwrap_or(Self::SSOC_DELTA_DEFAULT);
        let mut curve_changed = false;
        let mut new_curve = self.current_curve;

        match transition {
            PlugTransition::Unplugged => {
                new_curve = Self::DISCHARGE_CURVE_DEFAULT;
                curve_changed = true;

                let (new_midpoint, ui) = if current_ui_soc >= Self::SSOC_FULL {
                    let new_midpoint =
                        (scaled_real_soc.max(Self::SSOC_SPOOF) - ssoc_delta).max(0.0);
                    info!(
                        "CurveMapper: Splicing discharge curve at real={:.2} due to disconnect while FULL",
                        new_midpoint
                    );
                    (new_midpoint, Self::SSOC_FULL)
                } else {
                    info!("CurveMapper: Switching to default discharge curve on disconnect");
                    (scaled_real_soc, current_ui_soc)
                };

                Self::set_midpoint(&mut new_curve, new_midpoint, ui);
            }
            PlugTransition::PluggedIn => {
                info!("CurveMapper: Detected Connect");
                new_curve = Self::CHG_CURVE_DEFAULT;
                Self::set_midpoint(&mut new_curve, scaled_real_soc, current_ui_soc);
                curve_changed = true;
            }
            PlugTransition::None => {}
        }

        if curve_changed {
            self.current_curve = new_curve;
            info!("CurveMapper: New curve: {:?}", self.current_curve);
        }
    }
}

type TimeStampNs = zx::sys::zx_time_t;
type TimeDeltaSecs = f32;

struct RateLimiter {
    max_rate: f32,
    rl_ssoc_target: f32,
    rl_ssoc_last_update: TimeStampNs,
    rl_current_level: f32,
    is_initialized: bool,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new_with_config(&crate::BatteryManagerConfig::default())
    }
}

impl RateLimiter {
    const NANO_SECOND_TO_SECONDS: f32 = 0.000000001;

    fn new_with_config(config: &crate::BatteryManagerConfig) -> RateLimiter {
        if let (Some(max_delta), Some(max_time)) = (config.rl_max_delta_soc, config.rl_max_time_s) {
            if max_time > 0.0 {
                return RateLimiter::new(max_delta / max_time);
            }
        }
        RateLimiter::new(f32::MAX)
    }

    fn new(rate: f32) -> RateLimiter {
        RateLimiter {
            max_rate: rate,
            rl_ssoc_target: 0.0,
            rl_ssoc_last_update: 0,
            rl_current_level: 0.0,
            is_initialized: false,
        }
    }

    fn apply_rate_limit(
        &mut self,
        current_target: f32,
        is_charging: bool,
        current_timestamp_ns: TimeStampNs,
    ) -> f32 {
        // POWER-ON
        if !self.is_initialized {
            self.rl_ssoc_target = current_target;
            self.rl_current_level = current_target;
            self.rl_ssoc_last_update = current_timestamp_ns;
            self.is_initialized = true;
            return self.rl_current_level;
        }

        let now = current_timestamp_ns;
        let delta_time_s: TimeDeltaSecs =
            (now - self.rl_ssoc_last_update) as f32 * Self::NANO_SECOND_TO_SECONDS;
        if delta_time_s <= 0.0 {
            // If no time passed, return current level and don't update last time
            return self.rl_current_level;
        }
        let max_delta = self.max_rate * delta_time_s;

        // limit according to charging status
        let new_target =
            if is_charging { current_target } else { current_target.min(self.rl_ssoc_target) };
        self.rl_ssoc_target = new_target.clamp(0.0, 100.0);

        // Calculate step to target
        let limiting;
        let mut step = self.rl_ssoc_target - self.rl_current_level;
        if step.abs() > max_delta {
            step = if step > 0.0 { max_delta } else { -max_delta };
            limiting = true;
        } else {
            limiting = false;
        }

        let new_level = self.rl_current_level + step;
        self.rl_ssoc_last_update = now;
        self.rl_current_level = new_level.clamp(0.0, 100.0);

        if limiting {
            info!(
                "RateLimit: Target={:.2}, MaxDelta={:.4}, Step={:.4}, NewLevel={:.2} Timestamp={:?}",
                self.rl_ssoc_target, max_delta, step, self.rl_current_level, now,
            );
        }

        self.rl_current_level
    }

    fn reset(&mut self) {
        self.is_initialized = false;
    }
}

pub(crate) struct Polisher {
    config: crate::BatteryManagerConfig,
    curve_mapper: CurveMapper,
    last_rate_limited_level: Option<f32>,
    last_post_curve: Option<f32>,
    estimator: ChargeTimeEstimator,
    rate_limiter: RateLimiter,
    last_is_plugged_in: Option<bool>,
    last_original_level: Option<f32>,
}

impl Polisher {
    #[cfg(test)]
    pub fn new() -> Polisher {
        Self::new_with_battery_manager_config(crate::BatteryManagerConfig::default_for_test())
    }

    pub fn new_with_battery_manager_config(config: crate::BatteryManagerConfig) -> Polisher {
        let rate_limiter = RateLimiter::new_with_config(&config);
        let estimator = ChargeTimeEstimator::new_with_config(
            /*use_actual_capacity*/ false,
            config.clone(),
        );
        Polisher {
            config,
            curve_mapper: CurveMapper::new(),
            last_rate_limited_level: None,
            last_post_curve: None,
            estimator,
            rate_limiter,
            last_is_plugged_in: None,
            last_original_level: None,
        }
    }

    fn scale_battery_level(&self, info: &mut fpower::BatteryInfo) {
        if let Some(level) = info.level_percent {
            info.level_percent = Some(InitialScaler::scale_level_with_offset(
                level as f32,
                self.config.shutdown_offset_percent,
            ));
        }
    }

    fn set_level_status(&self, level: Option<f32>, info: &mut fpower::BatteryInfo) {
        if let Some(l) = level {
            info.level_status = Some(LevelChecker::determine_level_status(l, info.charge_status));
        }
    }

    /// Calculates the estimated time to full charge.
    ///
    /// # Arguments
    /// * `scaled_real_soc` - The current scaled real SoC.
    /// * `rate_limited_soc` - The current rate-limited UI level (RL).
    /// * `info` - The BatteryInfo to update with the TTF result.
    fn calculate_time_to_full(
        &mut self,
        scaled_real_soc: Option<f32>,
        rate_limited_soc: Option<f32>,
        info: &mut fpower::BatteryInfo,
    ) {
        let Some(real_level) = scaled_real_soc else {
            warn!("Missing real level for TTF");
            info.time_remaining = Some(fpower::TimeRemaining::Indeterminate(0));
            return;
        };
        let Some(current_ui_level) = rate_limited_soc else {
            warn!("Missing UI level for TTF");
            info.time_remaining = Some(fpower::TimeRemaining::Indeterminate(0));
            return;
        };

        // Short-circuit if not plugged in (Time To Full is only calculated when plugged in)
        if !Self::is_plugged_in(info) {
            info.time_remaining = Some(fpower::TimeRemaining::Indeterminate(0));
            return;
        }

        // --- TTF "Full" Check ---
        // If the *UI* level is 100%, TTF is 0.
        if current_ui_level >= CurveMapper::SSOC_FULL {
            info.time_remaining = Some(fpower::TimeRemaining::FullCharge(0));
            return;
        }

        let actual_current = info.average_charging_current_ua.or(info.present_charging_current_ua);

        let avg_current = self.estimator.update_average_current(
            real_level,
            actual_current,
            info.timestamp,
            info.charge_status,
        );

        if info.charge_status != Some(fpower::ChargeStatus::Charging) {
            if info.charge_status == Some(fpower::ChargeStatus::Full) {
                info.time_remaining = Some(fpower::TimeRemaining::FullCharge(0));
            } else {
                info.time_remaining = Some(fpower::TimeRemaining::Indeterminate(0));
            }
            return;
        }

        debug!(
            "tier_accumulated_current_ua_ms: {:?}, average_current: {:?}",
            self.estimator.tier_accumulated_current_ua_ms, avg_current
        );

        let current_to_use = avg_current.or(actual_current);

        self.estimator.set_actual_capacity(info.full_capacity_uah);

        // --- Core TTF Estimation ---
        // The estimator.time_to_full function uses the REAL level (scaled_real_soc)
        let time_to_full_estimate = match self.estimator.time_to_full(
            real_level,
            100.0,
            current_to_use,
            info.temperature_mc,
        ) {
            Ok(duration) => duration.into_nanos(),
            Err(e) => {
                match e {
                    TimeEstimatorError::MissingConfig => {
                        debug!("Unable to estimate time to full: {:?}", e);
                    }
                    _ => {
                        warn!("Failed to estimate time to full: {:?}", e);
                    }
                }
                info.time_remaining = Some(fpower::TimeRemaining::Indeterminate(0));
                return;
            }
        };
        info.time_remaining = Some(fpower::TimeRemaining::FullCharge(time_to_full_estimate));
    }

    pub(crate) fn is_plugged_in(info: &fpower::BatteryInfo) -> bool {
        !matches!(
            info.charge_source,
            Some(fpower::ChargeSource::None) | Some(fpower::ChargeSource::Unknown) | None
        )
    }

    fn rate_limit_level(&mut self, info: &mut fpower::BatteryInfo) {
        let Some(level) = info.level_percent else {
            warn!("Missing level for rate limiter");
            return;
        };
        let Some(timestamp_ns) = info.timestamp else {
            warn!("Missing timestamp for rate limiter");
            return;
        };
        let is_charging_or_full = info.charge_status == Some(fpower::ChargeStatus::Charging)
            || info.charge_status == Some(fpower::ChargeStatus::Full);

        // The curve-mapped level becomes the *target* for the rate limiter.
        let rate_limited_soc =
            self.rate_limiter.apply_rate_limit(level, is_charging_or_full, timestamp_ns);

        info.level_percent = Some(rate_limited_soc);
    }

    /// Applies battery level spoofing by updating the curve and mapping the level.
    /// Returns the mapped level (post-curve).
    fn apply_spoofing(
        &mut self,
        info: &fpower::BatteryInfo,
        scaled_real_soc: Option<f32>,
    ) -> Option<f32> {
        let is_plugged_in = Self::is_plugged_in(info);

        // Initialization on first run
        if self.last_is_plugged_in.is_none() {
            self.curve_mapper.current_curve = CurveMapper::CHG_CURVE_DEFAULT;
            if let Some(level) = scaled_real_soc {
                CurveMapper::set_midpoint(
                    &mut self.curve_mapper.current_curve,
                    level,
                    level, // Anchor UI to Real at boot
                );
            }
        }

        let was_plugged_in = self.last_is_plugged_in.unwrap_or(is_plugged_in);

        // Get the previous rate-limited level (RL) for curve state updates
        let prev_rate_limited_level =
            self.last_rate_limited_level.unwrap_or_else(|| scaled_real_soc.unwrap_or(0.0));

        if let Some(level) = scaled_real_soc {
            // Update Curve State (based on current scaled_real_soc and previous RL)
            let transition = match (was_plugged_in, is_plugged_in) {
                (true, false) => PlugTransition::Unplugged,
                (false, true) => PlugTransition::PluggedIn,
                _ => PlugTransition::None,
            };

            self.curve_mapper.update_curve_state(
                transition,
                level,
                prev_rate_limited_level,
                self.config.ssoc_delta,
            );

            // Handle Full state spoofing
            if info.charge_status == Some(fpower::ChargeStatus::Full) {
                info!(
                    "CurveMapper: Splicing curve to FULL at real={:.2} due to FULL status",
                    level
                );
                CurveMapper::set_midpoint(
                    &mut self.curve_mapper.current_curve,
                    level,
                    CurveMapper::SSOC_FULL,
                );
            }
        }

        // Return the curve-mapped level (UIC equivalent)
        scaled_real_soc.map(|level| self.curve_mapper.ssoc_uicurve_map(level))
    }

    pub fn polish_info(&mut self, info: fpower::BatteryInfo) -> fpower::BatteryInfo {
        let original_level = info.level_percent;
        let mut info = info;

        // 1. Scaled Real SoC
        self.scale_battery_level(&mut info);
        let scaled_real_soc = info.level_percent;

        // 2. Apply Spoofing & Curve Mapping (returns UIC)
        let post_curve = self.apply_spoofing(&info, scaled_real_soc);
        info.level_percent = post_curve;

        // 3. Rate Limiting (RL equivalent)
        self.rate_limit_level(&mut info);
        let rate_limited_soc = info.level_percent; // RL

        // 4. Time to Full Calculation (Uses scaled_real_soc for core, RL for "is full")
        self.calculate_time_to_full(scaled_real_soc, rate_limited_soc, &mut info);

        // 5. Set Level Status (Uses rate_limited_soc / RL)
        self.set_level_status(rate_limited_soc, &mut info);

        // Logging
        if self.last_original_level != original_level
            || self.last_post_curve != post_curve
            || self.last_rate_limited_level != rate_limited_soc
        {
            info!(
                "Levels - original: {:?}, scaled: {:?}, post curve mapping: {:?}, rate limited: {:?}",
                original_level, scaled_real_soc, post_curve, rate_limited_soc
            );
            self.last_original_level = original_level;
            self.last_rate_limited_level = rate_limited_soc; // Store current RL for next cycle
            self.last_post_curve = post_curve; // Store UIC here!
        }
        self.last_is_plugged_in = Some(Self::is_plugged_in(&info));
        info
    }

    pub fn reset_rate_limiter(&mut self) {
        self.rate_limiter.reset();
    }

    pub fn reset_average_current(&mut self) {
        self.estimator.reset_average_current();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[fuchsia::test]
    fn test_scale_level() {
        assert_eq!(InitialScaler::scale_level_with_offset(3.0, 3.0), 0.0);
        assert_eq!(InitialScaler::scale_level_with_offset(13.0, 3.0), 11.0);
        assert_eq!(InitialScaler::scale_level_with_offset(51.0, 3.0), 50.0);
        assert_eq!(InitialScaler::scale_level_with_offset(51.5, 3.0), 50.0);
        assert_eq!(InitialScaler::scale_level_with_offset(99.0, 3.0), 99.0);
        assert_eq!(InitialScaler::scale_level_with_offset(100.0, 3.0), 100.0);
        assert_eq!(InitialScaler::scale_level_with_offset(101.0, 3.0), 100.0);
    }

    #[fuchsia::test]
    fn test_scale_level_with_offset_bounds() {
        // Offset <= 0.0 or >= 100.0 should perform no scaling but apply ceil()
        assert_eq!(InitialScaler::scale_level_with_offset(50.0, 0.0), 50.0);
        assert_eq!(InitialScaler::scale_level_with_offset(50.5, 0.0), 51.0);
        assert_eq!(InitialScaler::scale_level_with_offset(50.0, -5.0), 50.0);
        assert_eq!(InitialScaler::scale_level_with_offset(50.5, -5.0), 51.0);
        assert_eq!(InitialScaler::scale_level_with_offset(50.0, 100.0), 50.0);
        assert_eq!(InitialScaler::scale_level_with_offset(50.5, 100.0), 51.0);
        assert_eq!(InitialScaler::scale_level_with_offset(50.0, 105.0), 50.0);
        assert_eq!(InitialScaler::scale_level_with_offset(50.5, 105.0), 51.0);

        // Level >= 100.0 should be capped at 100.0 even when offset <= 0.0
        assert_eq!(InitialScaler::scale_level_with_offset(101.0, 0.0), 100.0);

        // Valid offset (e.g. 3.0) scales normally
        assert_eq!(InitialScaler::scale_level_with_offset(51.0, 3.0), 50.0);
    }

    #[fuchsia::test]
    fn test_determine_level_status() {
        assert_eq!(
            LevelChecker::determine_level_status(0.0, Some(fpower::ChargeStatus::NotCharging)),
            fpower::LevelStatus::Critical
        );
        assert_eq!(
            LevelChecker::determine_level_status(0.0, Some(fpower::ChargeStatus::Discharging)),
            fpower::LevelStatus::Critical
        );
        assert_eq!(
            LevelChecker::determine_level_status(0.0, Some(fpower::ChargeStatus::Unknown)),
            fpower::LevelStatus::Unknown
        );
        assert_eq!(LevelChecker::determine_level_status(0.0, None), fpower::LevelStatus::Unknown);
        assert_eq!(
            LevelChecker::determine_level_status(LevelChecker::THRESHOLD_LEVEL_OK + 1.0, None),
            fpower::LevelStatus::Ok
        );
        assert_eq!(
            LevelChecker::determine_level_status(LevelChecker::THRESHOLD_LEVEL_WARNING + 1.0, None),
            fpower::LevelStatus::Warning
        );
        assert_eq!(
            LevelChecker::determine_level_status(LevelChecker::THRESHOLD_LEVEL_LOW + 1.0, None),
            fpower::LevelStatus::Low
        );
        assert_eq!(
            LevelChecker::determine_level_status(100.0, Some(fpower::ChargeStatus::Charging)),
            fpower::LevelStatus::Ok
        );
    }

    #[fuchsia::test]
    fn test_scale_battery_level() {
        let polisher = Polisher::new_with_battery_manager_config(crate::BatteryManagerConfig {
            shutdown_offset_percent: 3.0,
            ..Default::default()
        });

        // Test when level_percent = shutdown offset (3.0)
        let mut info = fpower::BatteryInfo { level_percent: Some(3.0), ..Default::default() };
        polisher.scale_battery_level(&mut info);
        assert_eq!(info.level_percent, Some(0.0));

        // Test when level_percent = 100%
        info.level_percent = Some(100.0);
        polisher.scale_battery_level(&mut info);
        assert_eq!(info.level_percent, Some(100.0));
    }

    // Helper to create a default BatteryInfo for tests
    fn new_info(level: f32, status: fpower::ChargeStatus) -> fpower::BatteryInfo {
        fpower::BatteryInfo {
            level_percent: Some(level),
            charge_status: Some(status),
            charge_source: match status {
                fpower::ChargeStatus::Charging | fpower::ChargeStatus::Full => {
                    Some(fpower::ChargeSource::Usb)
                }
                _ => Some(fpower::ChargeSource::None),
            },
            ..Default::default()
        }
    }

    #[fuchsia::test]
    fn test_normal_charging_is_one_to_one() {
        let mut polisher = Polisher::new();
        // Input a normal charging level
        let info = polisher.polish_info(new_info(83.0, fpower::ChargeStatus::Charging));

        let expected_level = InitialScaler::scale_level_with_offset(83.0, 0.0);
        assert_matches!(info.level_percent, Some(p) if (p - expected_level).abs() < f32::EPSILON);

        // The curve should have its midpoint set at the first reading (83.0)
        let expected_curve = [
            CurvePoint { real: 15.0, ui: 15.0 },
            CurvePoint { real: 83.0, ui: 83.0 },
            CurvePoint { real: 100.0, ui: 100.0 },
        ];
        assert_eq!(polisher.curve_mapper.current_curve, expected_curve);
    }

    #[fuchsia::test]
    fn test_initialization_discharging_is_one_to_one() {
        let mut polisher = Polisher::new();
        // Input a discharging level at boot
        let info = polisher.polish_info(new_info(77.0, fpower::ChargeStatus::Discharging));

        let expected_level = InitialScaler::scale_level_with_offset(77.0, 0.0);
        assert_matches!(info.level_percent, Some(p) if (p - expected_level).abs() < f32::EPSILON);

        // The curve should have its midpoint set at the first reading (77.0)
        let expected_curve = [
            CurvePoint { real: 15.0, ui: 15.0 },
            CurvePoint { real: 77.0, ui: 77.0 },
            CurvePoint { real: 100.0, ui: 100.0 },
        ];
        assert_eq!(polisher.curve_mapper.current_curve, expected_curve);
    }

    #[fuchsia::test]
    fn test_unplug_unmodified_while_charging_is_smooth() {
        let mut polisher = Polisher::new();
        // Establish that we are in a charging state.
        let _ = polisher.polish_info(new_info(95.0, fpower::ChargeStatus::Charging));
        assert_eq!(polisher.curve_mapper.current_curve, CurveMapper::CHG_CURVE_DEFAULT);

        // Unplug at 96%.
        let _ = polisher.polish_info(new_info(96.0, fpower::ChargeStatus::Charging));
        let info = polisher.polish_info(new_info(96.0, fpower::ChargeStatus::Discharging));
        let expected_level = InitialScaler::scale_level_with_offset(96.0, 0.0);
        assert_matches!(info.level_percent, Some(p) if (p - expected_level).abs() < f32::EPSILON);

        // Verify that the curve midpoint was set at the current point on disconnect
        assert_eq!(polisher.curve_mapper.current_curve[1].real, 96.0);
        assert_eq!(polisher.curve_mapper.current_curve[1].ui, 96.0);
    }

    const NANOS_PER_SEC: i64 = 1_000_000_000;

    #[fuchsia::test]
    fn test_discharge_curve_spoofing_hold() {
        let mut mapper = CurveMapper::new();
        mapper.current_curve = CurveMapper::DISCHARGE_CURVE_DEFAULT;

        // 97 real should be > 95 so it interpolates between (95, 100) and (100, 100), yielding 100.
        let mapped = mapper.ssoc_uicurve_map(97.0);
        assert_eq!(mapped, 100.0);

        // 90 real interpolates between (15, 15) and (95, 100).
        let mapped_90 = mapper.ssoc_uicurve_map(90.0);
        assert_eq!(mapped_90, 15.0 + 75.0 * (85.0 / 80.0));
    }

    #[fuchsia::test]
    fn test_comprehensive_cycle_without_zig_zag() {
        let mut polisher = Polisher::new();
        const T_SEC: i64 = 1000;
        let mut timestamp: zx::sys::zx_time_t = 0;

        // --- SETUP: Start with 10%  ---
        let mut info = new_info(10.0, fpower::ChargeStatus::Charging);
        info.timestamp = Some(timestamp);
        let polished = polisher.polish_info(info);
        assert!(polished.level_percent.is_some());
        let mut previous_level = polished.level_percent.unwrap();

        // --- CHARGE TO 90% ---
        for i in 11..=90 {
            timestamp += T_SEC * NANOS_PER_SEC;
            let mut info = new_info(i as f32, fpower::ChargeStatus::Charging);
            info.timestamp = Some(timestamp);
            let polished = polisher.polish_info(info);

            info!("Looping charging with input {}", i);
            assert!(polished.level_percent.is_some());
            let current_level = polished.level_percent.unwrap();
            assert!(
                current_level >= previous_level,
                "level_percent should increase or remain flat during charging"
            );
            previous_level = current_level;
        }

        // --- DISCHARGING TO 50% ---
        for i in (50..89).rev() {
            timestamp += T_SEC * NANOS_PER_SEC;
            let mut info = new_info(i as f32, fpower::ChargeStatus::Discharging);
            info.timestamp = Some(timestamp);
            let polished = polisher.polish_info(info);

            info!("Looping discharging with input {}", i);
            assert!(polished.level_percent.is_some());
            let current_level = polished.level_percent.unwrap();
            assert!(
                current_level <= previous_level,
                "level_percent should decrease or remain flat during discharging"
            );
            previous_level = current_level;
        }

        // --- CHARGE TO 90% ---
        for i in 51..=100 {
            timestamp += T_SEC * NANOS_PER_SEC;
            let mut info = new_info(i as f32, fpower::ChargeStatus::Charging);
            info.timestamp = Some(timestamp);
            let polished = polisher.polish_info(info);

            info!("Looping charging with input {}", i);
            assert!(polished.level_percent.is_some());
            let current_level = polished.level_percent.unwrap();
            assert!(
                current_level >= previous_level,
                "level_percent should increase or remain flat during charging"
            );
            previous_level = current_level;
        }

        // --- DISCHARGING TO 70% but FULL ---
        for i in (70..100).rev() {
            timestamp += T_SEC * NANOS_PER_SEC;
            let mut info = new_info(i as f32, fpower::ChargeStatus::Full);
            info.timestamp = Some(timestamp);
            let polished = polisher.polish_info(info);

            info!("Looping discharging with input {}", i);
            assert!(polished.level_percent.is_some());
            let current_level = polished.level_percent.unwrap();
            assert!(
                current_level <= previous_level,
                "level_percent should not increase when draining while full"
            );
            previous_level = current_level;
        }
    }

    #[fuchsia::test]
    fn test_get_level_duration_lookup() {
        let estimator = ChargeTimeEstimator::new(false);
        // 1. Below the lowest threshold (78)
        assert_eq!(estimator.get_level_duration(70).unwrap(), 32, "Level 70 should return 32.");
        assert_eq!(estimator.get_level_duration(78).unwrap(), 32, "Level 78 should return 32.");

        // 2. Between the first two thresholds (78 < L <= 86)
        assert_eq!(estimator.get_level_duration(79).unwrap(), 56, "Level 79 should return 56.");
        assert_eq!(estimator.get_level_duration(85).unwrap(), 56, "Level 85 should return 56.");
        assert_eq!(estimator.get_level_duration(86).unwrap(), 56, "Level 86 should return 56.");

        // 3. Between 86 and 96
        assert_eq!(estimator.get_level_duration(95).unwrap(), 84, "Level 95 should return 84.");
        assert_eq!(estimator.get_level_duration(96).unwrap(), 84, "Level 96 should return 84.");

        // 4. Near full (96 < L <= 100)
        assert_eq!(estimator.get_level_duration(97).unwrap(), 92, "Level 97 should return 92.");
        assert_eq!(estimator.get_level_duration(99).unwrap(), 92, "Level 99 should return 92.");
        assert_eq!(
            estimator.get_level_duration(100),
            Err(TimeEstimatorError::InvalidRange),
            "Level 100 should return Err."
        );

        // 5. Above table maximum (u32 input)
        assert_eq!(
            estimator.get_level_duration(101),
            Err(TimeEstimatorError::InvalidRange),
            "Level 101 should return Err."
        );
    }

    #[fuchsia::test]
    fn test_get_reference_current_ua() {
        // Temperature below 10C (row 0), SOC < 84 (col 0)
        assert_eq!(
            ChargeTimeEstimator::get_static_reference_current_ua(50.0, Some(5_000)),
            Ok(200_000)
        );

        // Temperature 10-20C (row 1), SOC < 84 (col 0)
        assert_eq!(
            ChargeTimeEstimator::get_static_reference_current_ua(50.0, Some(15_000)),
            Ok(275_000)
        );

        // Temperature 20-42C (row 2), SOC 84-90 (col 1)
        assert_eq!(
            ChargeTimeEstimator::get_static_reference_current_ua(85.0, Some(25_000)),
            Ok(500_000)
        );

        // Temperature 42-46C (row 3, SOC 84-90 (col 1))
        assert_eq!(
            ChargeTimeEstimator::get_static_reference_current_ua(89.0, Some(45_000)),
            Ok(400_000)
        );

        // Temperature > 46C (row 3, saturating fallback)
        assert_eq!(
            ChargeTimeEstimator::get_static_reference_current_ua(95.0, Some(50_000)),
            Ok(200_000)
        );

        // Default temperature (25_000)
        assert_eq!(ChargeTimeEstimator::get_static_reference_current_ua(50.0, None), Ok(500_000));
    }

    #[fuchsia::test]
    fn test_ttf_current_ratio() {
        let estimator = ChargeTimeEstimator::new(false);

        // Test Error conditions
        assert_eq!(
            estimator.ttf_current_ratio(None, 50.0, None),
            Err(TimeEstimatorError::MissingCurrent)
        );
        assert_eq!(
            estimator.ttf_current_ratio(Some(0), 50.0, None),
            Err(TimeEstimatorError::NonPositiveCurrent)
        );
        assert_eq!(
            estimator.ttf_current_ratio(Some(-100), 50.0, None),
            Err(TimeEstimatorError::NonPositiveCurrent)
        );

        // Test get_reference_current_ua and get_implied_ref_current_ua
        let reference_current = ChargeTimeEstimator::get_static_reference_current_ua(50.0, None);
        assert_eq!(reference_current, Ok(500_000));

        // At 50% SOC, implied ref is 472,500 uA.
        let implied_current = estimator.get_implied_ref_current_ua(50).unwrap();
        assert_eq!(implied_current, 472_500);

        // if charging with actual current >= limit and ref current, ratio is capped at 1.0
        let higher_actual_current = 1_000_000;
        assert!(higher_actual_current >= implied_current);
        assert_eq!(estimator.ttf_current_ratio(Some(higher_actual_current), 50.0, None), Ok(1.0));

        let higher_actual_current = 500_000;
        assert!(higher_actual_current >= implied_current);
        assert_eq!(estimator.ttf_current_ratio(Some(higher_actual_current), 50.0, None), Ok(1.0));

        // Actual current is small, giving a ratio > 1.0.
        // For 50% SOC, implied ref is 472,500 uA.
        // With 236,250 uA actual, it's capped at max limit. Ratio is capped at 1.0.
        assert_eq!(estimator.ttf_current_ratio(Some(236_250), 50.0, None), Ok(2.0));

        // Very cold temperature reduces cc_max_ua drastically.
        // At 0C and 50% SOC, cc_max_ua = 200,000 uA.
        // Actual current = 500,000 uA is capped at cc_max_ua = 200,000.
        // Ratio = 472,500 / 200,000 = 2.3625.
        assert_eq!(estimator.ttf_current_ratio(Some(500_000), 50.0, Some(0)), Ok(2.3625));
    }

    #[fuchsia::test]
    fn test_time_to_full() {
        let estimator = ChargeTimeEstimator::new(false);

        // Pre-calculated Bucket Sums (Seconds):
        // 79-86 (56s/level) = 8 * 56 = 448
        // 87-96 (84s/level) = 10 * 84 = 840
        // 97-100 (92s/level) = 4 * 92 = 368
        // 100 (0s/level) = 0
        // Total seconds from 78 to 100: 448 + 840 + 368 = 1656

        // --- CASE 1: Full (100.0) ---
        assert_eq!(estimator.time_to_full(100.0, 100.0, None, None).unwrap().into_seconds(), 0);

        // --- CASE 2: Near Full (99.0) ---
        // Sums: 99, 100 (2 levels) -> Call(99)=92s, Call(100)=0s. Total: 92s.
        // Use 500,000 uA as actual current to yield ratio = 1.0
        let expected_99 = 92;
        assert_eq!(
            estimator.time_to_full(99.0, 100.0, Some(500_000), None).unwrap().into_seconds(),
            expected_99
        );

        // --- CASE 3: Level 91.0 (Starts sum at 91) ---
        // Levels 91-96 (6 * 84s) + 97-99 (3 * 92s) + 100 (0s) = 504 + 276 = 780 seconds
        let expected_91 = 780;
        assert_eq!(
            estimator.time_to_full(91.0, 100.0, Some(500_000), None).unwrap().into_seconds(),
            expected_91
        );

        // --- CASE 4: Level 85.0 (Starts sum at 85)
        // Levels 85-86 (2 * 56s) + Levels 87-100 (10*84 + 3*92 + 0)
        // Sums: (2 * 56) + 840 + 276 = 112 + 1116 = 1228 seconds
        let expected_85 = 1228;
        assert_eq!(
            estimator
                .time_to_full(
                    85.0,
                    100.0,
                    // Supply a current that meets or exceeds cc_max_ua to yield ratio = 1.0
                    Some(500_000),
                    None,
                )
                .unwrap()
                .into_seconds(),
            expected_85,
            "At 85%, time remaining should be 1228 seconds."
        );

        // --- CASE 5: Level 50.0 (Starts sum at 50)
        // Levels 50-78 (29 * 32s) + Levels 79-100 (448 + 840 + 276)
        // Sums: (29 * 32) + 1564 = 928 + 1564 = 2492 seconds
        let expected_50 = 2492;
        assert_eq!(
            estimator.time_to_full(50.0, 100.0, Some(500_000), None).unwrap().into_seconds(),
            expected_50,
            "At 50%, time remaining should be 2492 seconds."
        );

        // --- CASE 6: Level 0.0% ---
        // Total seconds: (4184 - 92) = 4092 seconds.
        let expected_0 = 4092;
        assert_eq!(
            estimator.time_to_full(0.0, 100.0, Some(500_000), None).unwrap().into_seconds(),
            expected_0,
            "At 0%, time remaining should be 4092 seconds."
        );
    }

    #[fuchsia::test]
    fn test_charge_time_estimator_fractional() {
        let estimator = ChargeTimeEstimator::new(false);

        // 99.5% should be half of 99s level duration (92 / 2 = 46s)
        assert_eq!(
            estimator.time_to_full(99.5, 100.0, Some(500_000), None).unwrap().into_seconds(),
            46
        );

        // 98.2% = 0.8 * 92s (for 98 -> 99) + 92s (for 99 -> 100)
        // 0.8 * 92 = 73.6 (rounded to 73 in integer math: 92 * 80 / 100 = 73)
        // Test fractional calculation: 98.2% to 100.0%
        assert_eq!(
            estimator.time_to_full(98.2, 100.0, Some(500_000), None).unwrap().into_seconds(),
            165
        );

        // Test intra-level fractional calculation: 99.2% to 99.8%
        // elapsed for level 99 is 92. Since diff is 0.6, it should be 92 * 0.6 = 55.2 -> 55
        assert_eq!(
            estimator.time_to_full(99.2, 99.8, Some(500_000), None).unwrap().into_seconds(),
            55
        );

        // Test out of bounds SOC/last ratio
        assert_eq!(
            estimator.time_to_full(
                100.0,
                90.0, // Error: last < soc
                Some(500_000),
                None
            ),
            Err(TimeEstimatorError::InvalidRange)
        );

        // Test missing actual_current (None) yields MissingCurrent properly
        assert_eq!(
            estimator.time_to_full(
                50.0, 100.0, None, // Missing actual battery current
                None
            ),
            Err(TimeEstimatorError::MissingCurrent)
        );
    }

    #[fuchsia::test]
    fn test_charge_time_estimator_ratio() {
        let estimator = ChargeTimeEstimator::new(false);
        let ref_current =
            ChargeTimeEstimator::get_static_reference_current_ua(100.0, None).unwrap();

        // Base case with ref current: should match None
        assert_eq!(
            estimator.time_to_full(99.0, 100.0, Some(ref_current), None).unwrap().into_seconds(),
            92
        );

        // Half current -> double time: 92 * (100 / 0.5) / 100 = 184
        let implied_99 = estimator.get_implied_ref_current_ua(99).unwrap();
        assert_eq!(
            estimator.time_to_full(99.0, 100.0, Some(implied_99 / 2), None).unwrap().into_seconds(),
            184
        );

        // Negative current -> returns Err instead of falling back to base case
        assert_eq!(
            estimator.time_to_full(99.0, 100.0, Some(-100), None),
            Err(TimeEstimatorError::NonPositiveCurrent)
        );

        // Zero current -> returns Err instead of falling back to base case
        assert_eq!(
            estimator.time_to_full(99.0, 100.0, Some(0), None),
            Err(TimeEstimatorError::NonPositiveCurrent)
        );

        // Very high current -> capped at max cc_max meaning ratio clamped at 1.0. Time remains 92s.
        assert_eq!(
            estimator.time_to_full(99.0, 100.0, Some(implied_99 * 2), None).unwrap().into_seconds(),
            92
        );
    }

    #[fuchsia::test]
    fn test_calculate_time_to_full() {
        let mut polisher = Polisher::new();

        // Test None
        let mut info = fpower::BatteryInfo {
            charge_status: Some(fpower::ChargeStatus::Charging),
            charge_source: Some(fpower::ChargeSource::Usb),
            ..Default::default()
        };
        polisher.calculate_time_to_full(info.level_percent, info.level_percent, &mut info);
        assert_eq!(info.time_remaining, Some(fpower::TimeRemaining::Indeterminate(0)),);

        // Test glitched negative current
        info = new_info(50.0, fpower::ChargeStatus::Charging);
        info.average_charging_current_ua = Some(-1);
        polisher.calculate_time_to_full(info.level_percent, info.level_percent, &mut info);
        assert_eq!(info.time_remaining, Some(fpower::TimeRemaining::Indeterminate(0)));

        // Test 50%
        let expected_50_nanos = 2492 * NANOS_PER_SEC;
        info = new_info(50.0, fpower::ChargeStatus::Charging);
        info.average_charging_current_ua =
            Some(ChargeTimeEstimator::get_static_reference_current_ua(50.0, None).unwrap());
        polisher.calculate_time_to_full(info.level_percent, info.level_percent, &mut info);
        assert_eq!(
            info.time_remaining,
            Some(fpower::TimeRemaining::FullCharge(expected_50_nanos)),
            "At 50%, time remaining should be 2492 seconds."
        );

        // Test 100%
        info = new_info(100.0, fpower::ChargeStatus::Charging);
        info.average_charging_current_ua = None;
        polisher.calculate_time_to_full(info.level_percent, info.level_percent, &mut info);
        assert_eq!(
            info.time_remaining,
            Some(fpower::TimeRemaining::FullCharge(0)),
            "At 100%, time remaining should be 0 seconds."
        );
    }

    #[fuchsia::test]
    fn test_calculate_time_to_full_with_current_limits() {
        let mut polisher = Polisher::new();

        // Level 50%, temperature 25C (25000 mc)
        // From get_reference_current_ua:
        // temp_mc = 25000 -> row_idx = 2 (20-42C)
        // level = 50.0 -> col_idx = 0 (< 84%)
        // CHG_CC_LIMITS_UA[2][0] = 500_000 uA
        let ref_current_ua = 500_000;

        // Actual current is half of ref current (ratio = 50)
        let actual_current_ua = ref_current_ua / 2;

        let mut info = new_info(50.0, fpower::ChargeStatus::Charging);
        info.temperature_mc = Some(25_000);
        info.average_charging_current_ua = Some(actual_current_ua);

        polisher.calculate_time_to_full(info.level_percent, info.level_percent, &mut info);

        // At 50% with an actual current of 250,000uA:
        // - 50-78% (32 seconds): implied_cc=472.5mA, ratio=1.89. Total: 29 * 32s * 1.89 = 1753.92s
        // - 79-86% (56 seconds): implied_cc=270mA, ratio=1.08. Total: 8 * 56s * 1.08 = 483.84s
        // - 87-96% (84 seconds): implied_cc=180mA, ratio=1.0 (clamped). Total: 10 * 84s = 840s
        // - 97-99% (92 seconds): implied_cc=164mA, ratio=1.0 (clamped). Total: 3 * 92s = 276s
        // Total expected time: 1753.92 + 483.84 + 840 + 276 = 3353.76s = 3353s (result of as i64)
        let expected_seconds = 3353;
        let expected_nanos = expected_seconds * NANOS_PER_SEC;

        assert_eq!(
            info.time_remaining,
            Some(fpower::TimeRemaining::FullCharge(expected_nanos)),
            "At 50% with an actual current of 250mA, time remaining should be 3353s."
        );
    }

    #[fuchsia::test]
    fn test_uicurve_map_and_set_midpoint() {
        let mut curve = CurveMapper::CHG_CURVE_DEFAULT;
        CurveMapper::set_midpoint(&mut curve, 98.0, 100.0);

        assert_eq!(curve[1].real, 98.0);
        assert_eq!(curve[1].ui, 100.0);
    }

    #[fuchsia::test]
    fn test_interpolate() {
        let p1 = CurvePoint { real: 0.0, ui: 0.0 };
        let p2 = CurvePoint { real: 100.0, ui: 100.0 };

        // Midpoint
        assert_eq!(CurvePoint::interpolate(&p1, &p2, 50.0), 50.0);

        // Exact match with points
        assert_eq!(CurvePoint::interpolate(&p1, &p2, 0.0), 0.0);
        assert_eq!(CurvePoint::interpolate(&p1, &p2, 100.0), 100.0);

        // Different slope
        let p3 = CurvePoint { real: 10.0, ui: 0.0 };
        let p4 = CurvePoint { real: 90.0, ui: 100.0 };
        // slope = (100 - 0) / (90 - 10) = 100 / 80 = 1.25
        // for x = 30: 0 + 1.25 * (30 - 10) = 1.25 * 20 = 25.0
        assert_eq!(CurvePoint::interpolate(&p3, &p4, 30.0), 25.0);

        // dx == 0 case
        let p5 = CurvePoint { real: 50.0, ui: 50.0 };
        let p6 = CurvePoint { real: 50.0, ui: 60.0 };
        assert_eq!(CurvePoint::interpolate(&p5, &p6, 50.0), 50.0);
    }

    #[fuchsia::test]
    fn test_update_curve_state() {
        let mut mapper = CurveMapper::new();

        // Initially CHG curve
        assert_eq!(mapper.current_curve, CurveMapper::CHG_CURVE_DEFAULT);

        // No state change when transition is None
        mapper.update_curve_state(PlugTransition::None, 50.0, 50.0, None);
        assert_eq!(mapper.current_curve, CurveMapper::CHG_CURVE_DEFAULT);

        // Connect (PluggedIn)
        // It should set the midpoint of the curve at real=50.0, ui=50.0
        mapper.update_curve_state(PlugTransition::PluggedIn, 50.0, 50.0, None);
        assert_eq!(mapper.current_curve[1].real, 50.0);
        assert_eq!(mapper.current_curve[1].ui, 50.0);

        // Disconnect when not full (Unplugged)
        mapper.update_curve_state(PlugTransition::Unplugged, 60.0, 60.0, None);
        assert_eq!(mapper.current_curve[1].real, 60.0);
        assert_eq!(mapper.current_curve[1].ui, 60.0);

        // Disconnect when full (Unplugged)
        // scaled_real_soc = 95.0, current_ui_soc = 100.0
        // new_midpoint = (95.0.max(95.0) - 2.0) = 93.0
        mapper.update_curve_state(PlugTransition::Unplugged, 95.0, 100.0, Some(2.0));
        assert_eq!(mapper.current_curve[1].real, 93.0);
        assert_eq!(mapper.current_curve[1].ui, 100.0);

        // Disconnect when full (Unplugged) with default ssoc_delta (3.0)
        // scaled_real_soc = 95.0, current_ui_soc = 100.0
        // new_midpoint = (95.0.max(95.0) - 3.0) = 92.0
        mapper.update_curve_state(PlugTransition::Unplugged, 95.0, 100.0, None);
        assert_eq!(mapper.current_curve[1].real, 92.0);
        assert_eq!(mapper.current_curve[1].ui, 100.0);
    }

    #[fuchsia::test]
    fn test_set_midpoint_guard() {
        let mut curve = CurveMapper::CHG_CURVE_DEFAULT;
        let original_mid_point = curve[1].real;

        // Force curve[2].real to 90.0
        curve[2].real = 90.0;

        // Attempt to set midpoint at real=95.0.
        // Reality check: 95.0 > 90.0 (curve[2].real).
        // The guard in set_midpoint should trigger and return early.
        CurveMapper::set_midpoint(&mut curve, 95.0, 95.0);

        // Verify that NO change occurred because the guard blocked it
        assert_eq!(curve[1].real, original_mid_point);
        assert_eq!(curve[2].real, 90.0);
    }

    #[fuchsia::test]
    fn test_set_midpoint_valid() {
        let mut curve = CurveMapper::CHG_CURVE_DEFAULT;

        // Set midpoint at a valid point (within 15.0 and 100.0)
        CurveMapper::set_midpoint(&mut curve, 77.0, 77.0);

        assert_eq!(curve[1].real, 77.0);
        assert_eq!(curve[1].ui, 77.0);
    }

    #[fuchsia::test]
    fn test_apply_spoofing() {
        let mut polisher = Polisher::new();

        let mut info = fpower::BatteryInfo {
            status: Some(fpower::BatteryStatus::Ok),
            charge_status: Some(fpower::ChargeStatus::Discharging), // off charger
            ..Default::default()
        };

        // First run, off-charger
        polisher.last_is_plugged_in = Some(false);
        polisher.last_rate_limited_level = Some(50.0);

        let level = polisher.apply_spoofing(&info, Some(50.0));
        assert_eq!(level, Some(50.0));

        // Now connect
        info.charge_status = Some(fpower::ChargeStatus::Charging);
        info.charge_source = Some(fpower::ChargeSource::Usb);
        // We simulated last_is_plugged_in = false and last_rate_limited_level = 50.0
        let level = polisher.apply_spoofing(&info, Some(60.0));
        assert_eq!(level, Some(50.0));

        // It should set midpoint at real=60.0, ui=prev_RL (50.0)
        assert_eq!(polisher.curve_mapper.current_curve[1].real, 60.0);
        assert_eq!(polisher.curve_mapper.current_curve[1].ui, 50.0);

        // Now test Full state spoofing
        info.charge_status = Some(fpower::ChargeStatus::Full);
        let level = polisher.apply_spoofing(&info, Some(95.0));
        assert_eq!(level, Some(100.0));
        // It should set midpoint to FULL (real=95, ui=100)
        assert_eq!(polisher.curve_mapper.current_curve[1].real, 95.0);
        assert_eq!(polisher.curve_mapper.current_curve[1].ui, 100.0);
    }

    fn seconds_to_nanoseconds(sec: TimeStampNs) -> TimeStampNs {
        sec * NANOS_PER_SEC
    }

    #[fuchsia::test]
    fn test_rate_limiter_advances() {
        // Pick a better rate: 2% / 16 seconds
        let mut rl = RateLimiter::new(0.125);

        let t0: TimeStampNs = seconds_to_nanoseconds(100);
        let initial_level: f32 = 50.0;
        rl.apply_rate_limit(initial_level, true, t0);

        // Advance time by 24 seconds with a huge jump to 100.
        let t1: TimeStampNs = seconds_to_nanoseconds(124);
        let target_level: f32 = 100.0;

        // Max change allowed: (2.0 / 16.0) * 24.0 = 3.0%
        let result = rl.apply_rate_limit(target_level, true, t1);

        // The level should move by the Max Allowed Delta: 50.0 + 3.0
        assert_eq!(result, 53.0, "Level should advance by 3.0% in 24 seconds.");
        assert_eq!(
            rl.rl_ssoc_last_update,
            seconds_to_nanoseconds(124),
            "Last update time should be 124."
        );
        assert_eq!(rl.rl_current_level, 53.0);

        // Advance time by 32 seconds with a small jump by 1%
        let t2: TimeStampNs = seconds_to_nanoseconds(156);
        let target_level: f32 = 54.0;

        let result = rl.apply_rate_limit(target_level, true, t2);

        assert_eq!(result, 54.0, "Level should advance by 1.0% in 32 seconds.");
        assert_eq!(
            rl.rl_ssoc_last_update,
            seconds_to_nanoseconds(156),
            "Last update time should be 156."
        );
        assert_eq!(rl.rl_current_level, 54.0);

        // Advance time by 16 seconds with a small drop by 1%
        let t3: TimeStampNs = seconds_to_nanoseconds(172);
        let target_level: f32 = 53.0;

        let result = rl.apply_rate_limit(target_level, false, t3);

        assert_eq!(result, 53.0, "Level should advance by -1.0% in 16 seconds.");
        assert_eq!(
            rl.rl_ssoc_last_update,
            seconds_to_nanoseconds(172),
            "Last update time should be 172."
        );
        assert_eq!(rl.rl_current_level, 53.0);

        // Advance time by 16 seconds with a large drop by 10%
        let t3: TimeStampNs = seconds_to_nanoseconds(188);
        let target_level: f32 = 43.0;

        let result = rl.apply_rate_limit(target_level, false, t3);

        assert_eq!(result, 51.0, "Level should advance by -2.0% in 16 seconds.");
        assert_eq!(
            rl.rl_ssoc_last_update,
            seconds_to_nanoseconds(188),
            "Last update time should be 188."
        );
        assert_eq!(rl.rl_current_level, 51.0);

        // Advance time by 16 seconds with a fluctuation.
        let t3: TimeStampNs = seconds_to_nanoseconds(204);
        let target_level: f32 = 50.0;

        let result = rl.apply_rate_limit(target_level, false, t3);

        assert_eq!(result, 49.0, "Level should advance by -2.0% in 16 seconds.");
        assert_eq!(
            rl.rl_ssoc_last_update,
            seconds_to_nanoseconds(204),
            "Last update time should be 172."
        );
        assert_eq!(rl.rl_current_level, 49.0);
    }

    #[fuchsia::test]
    fn test_rate_limiter_called_by_polisher() {
        let mut polisher = Polisher::new();

        let initial_level = 51.5;
        let mut incoming_info = new_info(initial_level, fpower::ChargeStatus::Charging);
        incoming_info.timestamp = Some(0);

        let info = polisher.polish_info(incoming_info);
        let initial_scaled_level = InitialScaler::scale_level_with_offset(
            initial_level,
            polisher.config.shutdown_offset_percent,
        );
        assert_matches!(info.level_percent, Some(p) if (p - initial_scaled_level).abs() < f32::EPSILON);

        let t0_s = 30;
        let t0: TimeStampNs = seconds_to_nanoseconds(t0_s);
        let new_level: f32 = 60.0;
        let mut incoming_info = new_info(new_level, fpower::ChargeStatus::Charging);
        incoming_info.timestamp = Some(t0);

        let info = polisher.polish_info(incoming_info);
        let expected_level2 = initial_scaled_level
            + t0_s as f32 * polisher.config.rl_max_delta_soc.unwrap()
                / polisher.config.rl_max_time_s.unwrap();
        assert_matches!(info.level_percent, Some(level) if expected_level2 == level);
    }

    #[fuchsia::test]
    fn test_polish_info_for_full_cycle() {
        let mut polisher = Polisher::new_with_battery_manager_config(crate::BatteryManagerConfig {
            shutdown_offset_percent: 3.0,
            ..crate::BatteryManagerConfig::default_for_test()
        });
        // Establish that we are charging.
        let info = polisher.polish_info(new_info(95.0, fpower::ChargeStatus::Charging));
        assert_eq!(polisher.curve_mapper.current_curve, CurveMapper::CHG_CURVE_DEFAULT);
        let expected_real = InitialScaler::scale_level_with_offset(95.0, 3.0);
        assert_matches!(info.level_percent, Some(p) if (p - expected_real).abs() < f32::EPSILON);

        // Reaching 100%, then unplug. Real is 96%, but since previous UI was 100% and
        // we disconnected, we set midpoint.
        let _ = polisher.polish_info(new_info(100.0, fpower::ChargeStatus::Full));
        let info = polisher.polish_info(new_info(96.0, fpower::ChargeStatus::Discharging));

        // Since disconnect happened from 100% UI, it sets the midpoint at real = 96.0 - 2.0 = 94.0.
        // Curve will become [(15, 15), (94, 100), (100, 100)].
        assert_eq!(polisher.curve_mapper.current_curve[1].real, 94.0);
        assert_eq!(polisher.curve_mapper.current_curve[1].ui, 100.0);

        // UI level should be interpolated between 94 (100%) and 100 (100%), so for real=96, UI=100.0
        assert_matches!(info.level_percent, Some(p) if (p - 100.0).abs() < f32::EPSILON);

        // Drop to 90% (real)
        let info = polisher.polish_info(new_info(90.0, fpower::ChargeStatus::Discharging));
        // It's below the midpoint 94, so interpolates between 15,15 and 94,100.
        let expected_level = polisher.curve_mapper.ssoc_uicurve_map(90.0);
        assert_matches!(info.level_percent, Some(p) if (p - expected_level).abs() < f32::EPSILON);

        // Back to Unmodified behavior?
        // Map function naturally maps real=12.0 to ui=12.0 since it's below curve[0].real (15.0).
        let info = polisher.polish_info(new_info(14.0, fpower::ChargeStatus::Discharging));
        assert_matches!(info.level_percent, Some(p) if (p - 12.0).abs() < f32::EPSILON);
    }

    #[fuchsia::test]
    fn test_polish_info() {
        let mut polisher = Polisher::new_with_battery_manager_config(crate::BatteryManagerConfig {
            shutdown_offset_percent: 3.0,
            ..crate::BatteryManagerConfig::default_for_test()
        });

        // Test when level_percent = shutdown offset (3.0)
        let mut info = fpower::BatteryInfo {
            level_percent: Some(3.0),
            charge_status: Some(fpower::ChargeStatus::Discharging),
            charge_source: Some(fpower::ChargeSource::Usb),
            ..Default::default()
        };
        info = polisher.polish_info(info);
        assert_eq!(info.level_percent, Some(0.0));
        assert_eq!(info.level_status, Some(fpower::LevelStatus::Critical));

        // Test a dead battery
        info.level_percent = Some(0.0);
        info.charge_status = Some(fpower::ChargeStatus::Charging);
        info = polisher.polish_info(info);
        assert_eq!(info.level_status, Some(fpower::LevelStatus::Low));

        // Test a battery that is charging
        info.level_percent = Some(10.0);
        info.charge_status = Some(fpower::ChargeStatus::Charging);
        info = polisher.polish_info(info);
        assert_eq!(info.level_status, Some(fpower::LevelStatus::Low));

        info.level_percent = Some(49.0);
        info.charge_status = Some(fpower::ChargeStatus::Charging);
        info = polisher.polish_info(info);
        assert_eq!(info.level_status, Some(fpower::LevelStatus::Warning));

        info.level_percent = Some(49.0);
        info.charge_status = Some(fpower::ChargeStatus::Discharging);
        info = polisher.polish_info(info);
        assert_eq!(info.level_status, Some(fpower::LevelStatus::Warning));

        info.level_percent = Some(83.0);
        info.charge_status = Some(fpower::ChargeStatus::Charging);
        info.average_charging_current_ua =
            Some(ChargeTimeEstimator::get_static_reference_current_ua(83.0, None).unwrap());
        info = polisher.polish_info(info);
        assert_eq!(info.level_status, Some(fpower::LevelStatus::Ok));
        // Expected nanoseconds (1432 seconds * 1,000,000,000 nanos/sec)
        const EXPECTED_NANOS: i64 = 1_340_000_000_000;
        assert_eq!(info.time_remaining, Some(fpower::TimeRemaining::FullCharge(EXPECTED_NANOS)));

        // Test when level_percent = 100%
        info.level_percent = Some(100.0);
        info = polisher.polish_info(info);
        assert_eq!(info.level_percent, Some(100.0));
        assert_eq!(info.level_status, Some(fpower::LevelStatus::Ok));
        assert_eq!(
            info.time_remaining,
            Some(fpower::TimeRemaining::FullCharge(0)),
            "When level is 100%, time_remaining must be set to FullCharge(0)."
        );
    }

    #[fuchsia::test]
    fn test_missing_config_returns_indeterminate_ttf() {
        let mut polisher =
            Polisher::new_with_battery_manager_config(crate::BatteryManagerConfig::default());
        let mut info = new_info(50.0, fpower::ChargeStatus::Charging);
        info.average_charging_current_ua = Some(500_000);
        polisher.calculate_time_to_full(info.level_percent, info.level_percent, &mut info);
        assert_eq!(info.time_remaining, Some(fpower::TimeRemaining::Indeterminate(0)));
    }

    #[fuchsia::test]
    fn test_actual_capacity() {
        let mut estimator = ChargeTimeEstimator::new(false);
        // Design capacity is 420,000 uAh. The base time for 99%->100% is 92s.
        let implied_99_design = estimator.get_implied_ref_current_ua(99).unwrap();

        // Time to full using design capacity (at exactly its own implied reference current)
        let duration_design = estimator
            .time_to_full(99.0, 100.0, Some(implied_99_design), None)
            .unwrap()
            .into_seconds();
        // Since current matches the implied reference, ratio is 1.0, so time is just base_elap (92s).
        assert_eq!(duration_design, 92);

        // Now set actual capacity to 450,000 uAh. Since the capacity is larger,
        // charging at the SAME current (implied_99_design) should take proportionally longer.
        // 450 / 420 * 92 = 98.57 -> 98 seconds.
        estimator.use_actual_capacity = true;
        estimator.set_actual_capacity(Some(450_000));

        let duration_actual = estimator
            .time_to_full(99.0, 100.0, Some(implied_99_design), None)
            .unwrap()
            .into_seconds();

        assert_eq!(duration_actual, 98);

        // Finally check fallback: if use_actual_capacity is true but actual_capacity_uah is None,
        // it should fall back to the design capacity implicitly and yield 92s again.
        estimator.set_actual_capacity(None);
        let duration_fallback = estimator
            .time_to_full(99.0, 100.0, Some(implied_99_design), None)
            .unwrap()
            .into_seconds();

        assert_eq!(duration_fallback, 92);
    }

    #[fuchsia::test]
    fn test_curve_mapper_direction_change_smoothness() {
        let mut polisher = Polisher::new();

        // Initial State: Full (triggers Spoofing)
        let info = polisher.polish_info(new_info(100.0, fpower::ChargeStatus::Full));
        assert_eq!(info.level_percent.unwrap(), 100.0);

        // Discharge down to 95% (Spoofing phase).
        let mut expected_ui = 100.0;
        for level in (95..100).rev() {
            let lvl = level as f32;
            let info = polisher.polish_info(new_info(lvl, fpower::ChargeStatus::Discharging));
            let curr_ui = info.level_percent.unwrap();
            assert!(curr_ui <= expected_ui);
            expected_ui = curr_ui;
        }

        // Drop to 94%
        let info_disch_res =
            polisher.polish_info(new_info(94.0, fpower::ChargeStatus::Discharging));

        // UI level should be higher than raw level during discharge from full
        let ui_level_discharging = info_disch_res.level_percent.unwrap();
        // Since we dropped 4% from 98%, UI interpolates between 98->100
        assert!(ui_level_discharging > 94.0, "UI level was {}", ui_level_discharging);

        // Switch back to Charging at 94%
        let info_charging_res =
            polisher.polish_info(new_info(94.0, fpower::ChargeStatus::Charging));

        let ui_level_charging = info_charging_res.level_percent.unwrap();

        // Now with splicing on direction change, it maintains continuity and doesn't snap to 94.0.
        // It should be equal to the level before direction change (ui_level_discharging).
        assert_eq!(ui_level_charging, ui_level_discharging);
    }

    #[fuchsia::test]
    fn test_average_current_smoothing_reduces_ttf_swings() {
        let mut polisher = Polisher::new();

        let mut info = fpower::BatteryInfo {
            level_percent: Some(50.0),
            charge_status: Some(fpower::ChargeStatus::Charging),
            charge_source: Some(fpower::ChargeSource::AcAdapter),
            average_charging_current_ua: Some(500_000),
            timestamp: Some(0),
            ..Default::default()
        };

        // At t = 0s
        polisher.polish_info(info.clone());

        // At t = 60s, stable current
        info.timestamp = Some(60 * NANOS_PER_SEC);
        polisher.polish_info(info.clone());

        // At t = 120s, stable current
        info.timestamp = Some(120 * NANOS_PER_SEC);
        let info3 = polisher.polish_info(info.clone());

        // At t = 180s, current suddenly drops to near zero
        info.timestamp = Some(180 * NANOS_PER_SEC);
        info.average_charging_current_ua = Some(100);
        let info4 = polisher.polish_info(info.clone());

        // Extract TTF estimates or fail the test if the enum is wrong
        let ttf3 = match info3.time_remaining {
            Some(fpower::TimeRemaining::FullCharge(t)) => t,
            _ => panic!("Expected FullCharge for info3, got {:?}", info3.time_remaining),
        };

        let ttf4 = match info4.time_remaining {
            Some(fpower::TimeRemaining::FullCharge(t)) => t,
            _ => panic!("Expected FullCharge for info4, got {:?}", info4.time_remaining),
        };

        assert!(ttf3 > 0, "TTF should be positive");
        assert!(ttf4 > 0, "TTF should be positive");

        // Without smoothing, dipping from 500,000uA to 100uA would make TTF spike by 5000x.
        // With 3 minutes of smoothing, the localized drop only increases the average slightly.
        // So the new TTF should be less than double the previous stable TTF.
        assert!(
            ttf4 < ttf3 * 2,
            "TTF spiked drastically despite smoothing! ttf3: {}, ttf4: {}",
            ttf3,
            ttf4
        );
    }

    #[fuchsia::test]
    fn test_state_transition_resumption_spike() {
        let mut polisher = Polisher::new();

        let mut info = fpower::BatteryInfo {
            level_percent: Some(50.0),
            charge_status: Some(fpower::ChargeStatus::NotCharging),
            charge_source: Some(fpower::ChargeSource::AcAdapter),
            average_charging_current_ua: Some(0),
            timestamp: Some(0),
            ..Default::default()
        };

        // At t = 0s, plugged in but NotCharging
        polisher.polish_info(info.clone());

        // Device sits idle for 1 hour (3600s)
        info.timestamp = Some(3600 * NANOS_PER_SEC);
        polisher.polish_info(info.clone());

        // After 1 hour, suddenly starts Charging
        // To test if the previous 3600s period of NotCharging is accumulated or dropped.
        info.timestamp = Some(3605 * NANOS_PER_SEC);
        info.charge_status = Some(fpower::ChargeStatus::Charging);
        info.average_charging_current_ua = Some(500_000);
        polisher.polish_info(info.clone());

        // Assert that the accumulator discarded the 3600s gap and only accumulated the time since
        // the transition. Since only 0 elapsed time has been legally accumulated (it just started),
        // the average time should be far below 60s, NO calculated average is available yet.
        assert!(
            polisher.estimator.tier_accumulated_time_ms < 60000, // 60s
            "Accumulator erroneously included the idle duration across the state transition!"
        );
    }
}
