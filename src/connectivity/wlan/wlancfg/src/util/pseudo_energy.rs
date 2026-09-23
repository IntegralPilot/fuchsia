// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::types as client_types;
use anyhow::{format_err, Error};
use log::error;

/// Update a weighted average with a new measurement
fn calculate_ewma_update(current: f64, next: f64, weighting_factor: f64) -> f64 {
    let weight = 2.0 / (1.0 + weighting_factor);
    weight * next + (1.0 - weight) * current
}

/// Struct for maintaining a dB or dBm exponentially weighted moving average. Differs from
/// SignalStrengthAverage, which is not exponentially weighted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EwmaPseudoDecibel {
    current: f64,
    weighting_factor: f64,
}

impl EwmaPseudoDecibel {
    pub fn new(n: usize, initial_signal: impl Into<f64>) -> Self {
        Self { current: initial_signal.into(), weighting_factor: n as f64 }
    }

    /// Returns the current EWMA value
    pub fn get(&self) -> f64 {
        self.current
    }

    pub fn update_average(&mut self, next: impl Into<f64>) {
        self.current = calculate_ewma_update(self.current, next.into(), self.weighting_factor);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EwmaSignalData {
    pub ewma_rssi: EwmaPseudoDecibel,
    pub ewma_snr: EwmaPseudoDecibel,
}

impl EwmaSignalData {
    pub fn new(
        initial_rssi: impl Into<f64>,
        initial_snr: impl Into<f64>,
        ewma_weight: usize,
    ) -> Self {
        Self {
            ewma_rssi: EwmaPseudoDecibel::new(ewma_weight, initial_rssi),
            ewma_snr: EwmaPseudoDecibel::new(ewma_weight, initial_snr),
        }
    }

    #[allow(clippy::ptr_arg, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn new_from_list(
        signals: &Vec<client_types::Signal>,
        ewma_weight: usize,
    ) -> Result<Self, anyhow::Error> {
        let first_signal =
            signals.first().ok_or_else(|| format_err!("At least one signal must be provided"))?;

        let mut ewma = Self::new(first_signal.rssi_dbm, first_signal.snr_db, ewma_weight);
        for signal in signals.iter().skip(1) {
            ewma.update_with_new_measurement(signal.rssi_dbm, signal.snr_db);
        }
        Ok(ewma)
    }

    pub fn update_with_new_measurement(&mut self, rssi: impl Into<f64>, snr: impl Into<f64>) {
        self.ewma_rssi.update_average(rssi);
        self.ewma_snr.update_average(snr);
    }
}

/// Calculates the rate of change across a vector of dB measurements by determining
/// the slope of the line of best fit using least squares regression. Return is technically
/// dB(f64)/t where t is the unit of time used in the vector. Returns error if integer overflows.
///
/// Note: This is the linear velocity (not the logarithmic velocity), but it is a useful
/// abstraction for monitoring real-world signal changes.
///
/// Intended to be used for RSSI Values, ranging from -128 to -1.
fn calculate_raw_velocity(samples: Vec<f64>) -> Result<f64, Error> {
    let n = i32::try_from(samples.len())?;
    if n < 2 {
        return Err(format_err!("At least two data points required to calculate velocity"));
    }
    // Using i32 for the calculations, to allow more room for preventing overflows
    let mut sum_x: i32 = 0;
    let mut sum_y: i32 = 0;
    let mut sum_xy: i32 = 0;
    let mut sum_x2: i32 = 0;

    // Least squares regression summations, returning an error if there are any overflows
    for (i, y) in samples.iter().enumerate() {
        let x = i32::try_from(i).map_err(|_| format_err!("failed to convert index to i32"))?;
        sum_x = sum_x.checked_add(x).ok_or_else(|| format_err!("overflow of X summation"))?;
        sum_y =
            sum_y.checked_add(*y as i32).ok_or_else(|| format_err!("overflow of Y summation"))?;
        sum_xy = sum_xy
            .checked_add(x.checked_mul(*y as i32).ok_or_else(|| format_err!("overflow of X * Y"))?)
            .ok_or_else(|| format_err!("overflow of XY summation"))?;
        sum_x2 = sum_x2
            .checked_add(x.checked_mul(x).ok_or_else(|| format_err!("overflow of X**2"))?)
            .ok_or_else(|| format_err!("overflow of X2 summation"))?;
    }

    // Calculate velocity from summations, returning an error if there are any overflows. Note that
    // in practice, the try_from should never fail, since the input values are bound from 0 to -128.
    let velocity = (n.checked_mul(sum_xy).ok_or_else(|| format_err!("overflow in n * sum_xy"))?
        - sum_x.checked_mul(sum_y).ok_or_else(|| format_err!("overflow in sum_x * sum_y"))?)
        / (n.checked_mul(sum_x2).ok_or_else(|| format_err!("overflow in n * sum_x2"))?
            - sum_x.checked_mul(sum_x).ok_or_else(|| format_err!("overflow in sum_x**2"))?);
    Ok(velocity.into())
}

// Struct for tracking the exponentially weighted moving average (EWMA) signal measurements.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RssiVelocity {
    curr_velocity: f64,
    prev_rssi: f64,
}
impl RssiVelocity {
    pub fn new(initial_rssi: impl Into<f64>) -> Self {
        Self { curr_velocity: 0.0, prev_rssi: initial_rssi.into() }
    }

    #[allow(clippy::ptr_arg, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn new_from_list(rssi_samples: &Vec<f64>) -> Result<Self, anyhow::Error> {
        let last = rssi_samples.last().ok_or_else(|| format_err!("empty list"))?;
        match calculate_raw_velocity(rssi_samples.to_vec()) {
            Ok(velocity) => Ok(Self { curr_velocity: velocity, prev_rssi: *last }),
            Err(e) => Err(e),
        }
    }

    pub fn get(&mut self) -> f64 {
        self.curr_velocity
    }

    pub fn update(&mut self, rssi: impl Into<f64>) {
        let rssi: f64 = rssi.into();
        match calculate_raw_velocity(vec![self.prev_rssi, rssi]) {
            Ok(velocity) => self.curr_velocity = velocity,
            Err(e) => {
                error!("Failed to update velocity: {:?}", e);
            }
        }
        self.prev_rssi = rssi;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_util::{assert_gt, assert_lt};

    #[fuchsia::test]
    fn test_ewma_pseudo_decibel_simple_averaging_calculations() {
        let mut ewma_signal = EwmaPseudoDecibel::new(10, -50);
        assert_eq!(ewma_signal.get(), -50.0);

        // Validate average moves using exponential weighting
        ewma_signal.update_average(-60);
        assert_lt!(ewma_signal.get(), -50.0);
        assert_gt!(ewma_signal.get(), -60.0);

        // Validate average will eventually stabilize.
        for _ in 0..20 {
            ewma_signal.update_average(-60)
        }
        assert_eq!(ewma_signal.get().round(), -60.0);
    }

    #[fuchsia::test]
    fn test_ewma_pseudo_decibel_small_variation_averaging() {
        let mut ewma_signal = EwmaPseudoDecibel::new(5, -90);
        assert_eq!(ewma_signal.get(), -90.0);

        // Validate that a small change that does not change the i8 dBm average still changes the
        // internal f64 average.
        ewma_signal.update_average(-91);
        assert_eq!(ewma_signal.get().round(), -90.0);
        assert_lt!(ewma_signal.current, -90.0);

        // Validate that eventually the small changes are enough to change the i8 dbm average.
        for _ in 0..5 {
            ewma_signal.update_average(-91);
        }
        assert_lt!(ewma_signal.get(), -90.9);
    }

    #[fuchsia::test]
    fn test_ewma_signal_data_new_from_list() {
        let signals = vec![
            client_types::Signal { rssi_dbm: -40, snr_db: 30 },
            client_types::Signal { rssi_dbm: -60, snr_db: 15 },
        ];
        let ewma = EwmaSignalData::new_from_list(&signals, 10).unwrap();
        assert_lt!(ewma.ewma_rssi.get(), -40.0);
        assert_gt!(ewma.ewma_rssi.get(), -60.0);
        assert_lt!(ewma.ewma_snr.get(), 30.0);
        assert_gt!(ewma.ewma_snr.get(), 15.0);
    }

    #[fuchsia::test]
    fn test_ewma_signal_data_new_from_list_empty() {
        assert!(EwmaSignalData::new_from_list(&vec![], 10).is_err());
    }

    #[fuchsia::test]
    fn test_ewma_signal_data_update_with_new_measurements() {
        let mut signal_data = EwmaSignalData::new(-40, 30, 10);
        signal_data.update_with_new_measurement(-60, 15);
        assert_lt!(signal_data.ewma_rssi.get(), -40.0);
        assert_gt!(signal_data.ewma_rssi.get(), -60.0);
        assert_lt!(signal_data.ewma_snr.get(), 30.0);
        assert_gt!(signal_data.ewma_snr.get(), 15.0);
    }

    /// Vector argument must have length >=2.
    #[fuchsia::test]
    fn test_calculate_raw_velocity_insufficient_args() {
        assert!(calculate_raw_velocity(vec![]).is_err());
        assert!(calculate_raw_velocity(vec![-60.0]).is_err());
    }

    #[fuchsia::test]
    fn test_calculate_raw_velocity_negative() {
        assert_eq!(calculate_raw_velocity(vec![-60.0, -75.0]).expect("failed to calculate"), -15.0);
        assert_eq!(
            calculate_raw_velocity(vec![-40.0, -50.0, -58.0, -64.0]).expect("failed to calculate"),
            -8.0
        );
    }

    #[fuchsia::test]
    fn test_calculate_raw_velocity_positive() {
        assert_eq!(calculate_raw_velocity(vec![-48.0, -45.0]).expect("failed to calculate"), 3.0);
        assert_eq!(
            calculate_raw_velocity(vec![-70.0, -55.0, -45.0, -30.0]).expect("failed to calculate"),
            13.0
        );
    }

    #[fuchsia::test]
    fn test_calculate_raw_velocity_constant_zero() {
        assert_eq!(
            calculate_raw_velocity(vec![-25.0, -25.0, -25.0, -25.0, -25.0, -25.0])
                .expect("failed to calculate"),
            0.0
        );
    }

    #[fuchsia::test]
    fn test_calculate_raw_velocity_oscillating_zero() {
        assert_eq!(
            calculate_raw_velocity(vec![-35.0, -45.0, -35.0, -25.0, -35.0, -45.0, -35.0,])
                .expect("failed to calculate"),
            0.0
        );
    }

    #[fuchsia::test]
    fn test_calculate_raw_velocity_min_max() {
        assert_eq!(
            calculate_raw_velocity(vec![-1.0, -128.0]).expect("failed to calculate"),
            -127.0
        );
        assert_eq!(calculate_raw_velocity(vec![-128.0, -1.0]).expect("failed to calculate"), 127.0);
    }

    #[fuchsia::test]
    fn test_rssi_velocity_update() {
        let mut velocity = RssiVelocity::new(-40.0);
        velocity.update(-80.0);
        assert_lt!(velocity.get(), 0.0);

        let mut velocity = RssiVelocity::new(-40.0);
        velocity.update(-20.0);
        assert_gt!(velocity.get(), 0.0);
    }

    #[fuchsia::test]
    fn test_rssi_velocity_new_from_list_insufficent_args() {
        assert!(RssiVelocity::new_from_list(&vec![]).is_err());
        assert!(RssiVelocity::new_from_list(&vec![-40.0]).is_err());
    }

    #[fuchsia::test]
    fn test_rssi_velocity_new_from_list_negative() {
        assert_eq!(
            RssiVelocity::new_from_list(&vec![-60.0, -75.0]).expect("failed to calculate").get(),
            -15.0
        );
        assert_eq!(
            RssiVelocity::new_from_list(&vec![-40.0, -50.0, -58.0, -64.0])
                .expect("failed to calculate")
                .get(),
            -8.0
        );
    }

    #[fuchsia::test]
    fn test_rssi_velocity_new_from_list_positive() {
        assert_eq!(
            RssiVelocity::new_from_list(&vec![-48.0, -45.0]).expect("failed to calculate").get(),
            3.0
        );
        assert_eq!(
            RssiVelocity::new_from_list(&vec![-70.0, -55.0, -45.0, -30.0])
                .expect("failed to calculate")
                .get(),
            13.0
        );
    }

    #[fuchsia::test]
    fn test_rssi_velocity_new_from_list_constant_zero() {
        assert_eq!(
            RssiVelocity::new_from_list(&vec![-25.0, -25.0, -25.0, -25.0, -25.0, -25.0])
                .expect("failed to calculate")
                .get(),
            0.0
        );
    }

    #[fuchsia::test]
    fn test_rssi_velocity_new_from_list_oscillating_zero() {
        assert_eq!(
            RssiVelocity::new_from_list(&vec![-35.0, -45.0, -35.0, -25.0, -35.0, -45.0, -35.0,])
                .expect("failed to calculate")
                .get(),
            0.0
        );
    }

    #[fuchsia::test]
    fn test_rssi_velocity_new_from_list_min_max() {
        assert_eq!(
            RssiVelocity::new_from_list(&vec![-1.0, -128.0]).expect("failed to calculate").get(),
            -127.0
        );
        assert_eq!(
            RssiVelocity::new_from_list(&vec![-128.0, -1.0]).expect("failed to calculate").get(),
            127.0
        );
    }
}
