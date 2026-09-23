// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, bail, format_err};
use async_trait::async_trait;
use futures::{Future, FutureExt as _, TryFutureExt as _};
use serde::{Deserialize, Serialize};
use sorted_vec_map::SortedVecMap;
use std::ops::{Add, Div, Mul, Sub};

/// Abstracts over grouping of red, green, blue and clear color channel data.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Rgbc<T> {
    pub(crate) red: T,
    pub(crate) green: T,
    pub(crate) blue: T,
    pub(crate) clear: T,
}

impl<T> Rgbc<T> {
    /// Maps the supplied function to each color channel.
    ///
    /// # Example
    /// ```
    /// let rgbc = Rgbc { red: 1, green: 2, blue: 3, clear: 4 };
    /// let rgbc = rgbc.map(|c| c + 1);
    /// assert_eq!(rgbc, Rgbc { red: 2, green: 3, blue: 4, clear: 5 });
    /// ```
    pub(crate) fn map<U>(self, func: impl Fn(T) -> U) -> Rgbc<U> {
        let Self { red, green, blue, clear } = self;
        Rgbc { red: func(red), green: func(green), blue: func(blue), clear: func(clear) }
    }

    /// Maps the supplied function to each color channel, but returns the first error that occurs.
    pub(crate) fn map_async<U, F>(
        self,
        func: impl Fn(T) -> F,
    ) -> impl Future<Output = Result<Rgbc<U>, Error>>
    where
        F: Future<Output = Result<U, Error>>,
    {
        let Self { red, green, blue, clear } = self;
        let red = func(red).map(|result| result.context("map red"));
        let green = func(green).map(|result| result.context("map green"));
        let blue = func(blue).map(|result| result.context("map blue"));
        let clear = func(clear).map(|result| result.context("map clear"));
        let fut = futures::future::try_join4(red, green, blue, clear);
        fut.map_ok(|(red, green, blue, clear)| Rgbc { red, green, blue, clear })
    }

    /// Maps the supplied function to the matching pair of color channels of the inputs.
    ///
    /// # Example
    /// ```
    /// let left = Rgbc { red: 1, green: 2, blue: 3, clear: 4};
    /// let right = Rgbc { red: 5, green: 6, blue: 7, clear: 8};
    /// let rgbc = Rgbc::multi_map(left, right, |l, r| l + r);
    /// assert_eq!(rgbc, Rgbc { red: 6, green: 8, blue: 10, clear: 12 });
    /// ```
    pub(crate) fn multi_map<U>(rgbc1: Self, rgbc2: Self, func: impl Fn(T, T) -> U) -> Rgbc<U> {
        let Self { red: red1, green: green1, blue: blue1, clear: clear1 } = rgbc1;
        let Self { red: red2, green: green2, blue: blue2, clear: clear2 } = rgbc2;
        Rgbc {
            red: func(red1, red2),
            green: func(green1, green2),
            blue: func(blue1, blue2),
            clear: func(clear1, clear2),
        }
    }

    /// Applies a fold operation across each color channel as if the Rgbc struct was a vec with
    /// order: red, green, blue, clear.
    ///
    /// # Example
    /// ```
    /// let rgbc = Rgbc { red: 1, green: 2, blue: 3, clear: 4};
    /// let value = rgbc.fold(0, |acc, v| acc + v);
    /// assert_eq!(value, 10);
    /// ```
    pub(crate) fn fold<U>(self, acc: U, func: impl Fn(U, T) -> U) -> U {
        let Self { red, green, blue, clear } = self;
        [red, green, blue, clear].into_iter().fold(acc, func)
    }

    /// Helper function that ensures all fields of both [Rgbc] structs match according to the supplied
    /// predicate.
    #[cfg(test)]
    pub(crate) fn match_all(left: Self, right: Self, predicate: impl Fn(T, T) -> bool) -> bool {
        let Rgbc { red, green, blue, clear } = Self::multi_map(left, right, predicate);
        red && green && blue && clear
    }
}

impl<T> Sub for Rgbc<T>
where
    T: Sub<Output = T> + Copy,
{
    type Output = Rgbc<T::Output>;

    fn sub(self, rhs: Self) -> Self::Output {
        Rgbc::multi_map(self, rhs, |left, right| left - right)
    }
}

impl<T> Add for Rgbc<T>
where
    T: Add<Output = T> + Copy,
{
    type Output = Rgbc<T::Output>;

    fn add(self, rhs: Self) -> Self::Output {
        Rgbc::multi_map(self, rhs, |left, right| left + right)
    }
}

impl<T> Mul for Rgbc<T>
where
    T: Mul<Output = T> + Copy,
{
    type Output = Rgbc<T::Output>;

    fn mul(self, rhs: Self) -> Self::Output {
        Rgbc::multi_map(self, rhs, |left, right| left * right)
    }
}

impl<T> Div for Rgbc<T>
where
    T: Div<Output = T> + Copy,
{
    type Output = Rgbc<T::Output>;

    fn div(self, rhs: Self) -> Self::Output {
        Rgbc::multi_map(self, rhs, |left, right| left / right)
    }
}
#[derive(Deserialize, Debug)]
pub struct Configuration {
    pub calibration: Option<CalibrationConfiguration>,
    pub sensor: SensorConfiguration,
}

/// Configuration file format.
///
/// Each string in the rgbc structs should be a file path to a file with the following format:
/// ```no_rust
/// version sample_count
///
/// linear_fit_slope linear_fit_intercept
///
/// lux measurement
/// # repeated multiple times
/// ```
/// Only `linear_fit_slope` and `linear_fit_intercept` are used.
#[derive(Deserialize, Debug)]
pub struct CalibrationConfiguration {
    /// A list of [LedConfig]s.
    pub(crate) leds: Vec<LedConfig>,
    /// Calibration data collected with all LEDs and backlight off.
    pub(crate) off: Rgbc<String>,
    /// Calibration data collection with all LEDs and backlight at maximum brightness.
    pub(crate) all_on: Rgbc<String>,
    /// The mean calibration parameters of the fleet of devices matching this product's
    /// configuration.
    pub(crate) golden_calibration_params: Rgbc<Parameters>,
}

/// Light Sensor configuration
#[derive(Deserialize, Debug)]
pub struct SensorConfiguration {
    /// Vendor id of the product, which is used to validate `input_report_path`.
    pub(crate) vendor_id: u32,
    /// Product id of the product, which is used to validate `input_report_path`.
    pub(crate) product_id: u32,
    /// Coefficients which are multiplied by sensor rgbc results to get lux
    /// units.
    pub(crate) rgbc_to_lux_coefficients: Rgbc<f32>,
    /// Scaling factors which are multiplied by sensor output to get device
    /// readings in uW/cm^2 SI units (https://en.wikipedia.org/wiki/International_System_of_Units).
    pub(crate) si_scaling_factors: Rgbc<f32>,
    /// Range of adjustment settings for low through high sensitivity readings
    /// from the light sensor.
    pub(crate) settings: Vec<AdjustmentSetting>,
}

/// Configuration for a single LED.
///
/// Each string in the rgbc struct follows the format specified in [Configuration].
#[derive(Deserialize, Debug)]
pub struct LedConfig {
    /// The name of this LED. It should be a value that matches the names returned in the
    /// fuchsia.settings.Light FIDL API.
    name: String,
    /// Calibration data collected with only this LED on.
    rgbc: Rgbc<String>,
}

/// Linear fit parameters for a particular sensor channel. They describe the linear response that a
/// sensor has to a particular color channel.
#[derive(Copy, Clone, Serialize, Deserialize, Debug)]
pub struct Parameters {
    pub(crate) slope: f32,
    pub(crate) intercept: f32,
}

type LedMap = SortedVecMap<String, Rgbc<Parameters>>;

#[async_trait(?Send)]
pub trait FileLoader {
    async fn load_file(&self, file_path: &str) -> Result<String, Error>;
}

#[derive(Clone, Debug, Serialize)]
/// Calibration data that is used for calibrating light sensor readings.
pub struct Calibration {
    /// Map of LED names to the sensor [Parameters] when only the corresponding
    /// LED was on.
    leds: LedMap,
    /// The sensor [Parameters] when all LEDs were off.
    off: Rgbc<Parameters>,
    /// The sensor [Parameters] when all LEDs were on.
    all_on: Rgbc<Parameters>,
    /// The calibrated slope for the light sensor.
    calibrated_slope: Rgbc<f32>,
}

impl Calibration {
    pub async fn new(
        configuration: CalibrationConfiguration,
        file_loader: &impl FileLoader,
    ) -> Result<Self, Error> {
        let mut leds = SortedVecMap::new();
        for led_config in configuration.leds {
            let name = led_config.name;
            let config = match led_config
                .rgbc
                .map_async(
                    |file_path| async move { Self::parse_file(&file_path, file_loader).await },
                )
                .await
            {
                Ok(config) => config,
                Err(e) => {
                    log::error!("Failed to map {name:?}'s rgbc field: {e:?}");
                    log::error!("Will not account for {name:?} in calibration");
                    continue;
                }
            };
            let _ = leds.insert(name.clone(), config);
        }

        let off = configuration
            .off
            .map_async(|file_path| async move { Self::parse_file(&file_path, file_loader).await })
            .await
            .context("Failed to map off rgbc")?;
        let all_on = configuration
            .all_on
            .map_async(|file_path| async move { Self::parse_file(&file_path, file_loader).await })
            .await
            .context("Failed to map all_on rgbc")?;
        let calibrated_slope =
            configuration.golden_calibration_params.map(|c| c.slope) / off.map(|c| c.slope);
        Ok(Self { leds, off, all_on, calibrated_slope })
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(
        leds: LedMap,
        off: Rgbc<Parameters>,
        all_on: Rgbc<Parameters>,
        calibrated_slope: Rgbc<f32>,
    ) -> Self {
        Self { leds, off, all_on, calibrated_slope }
    }

    async fn parse_file(path: &str, file_loader: &impl FileLoader) -> Result<Parameters, Error> {
        let cal_contents = file_loader
            .load_file(path)
            .await
            .with_context(|| format_err!("Could not load {path:?} for parsing"))?;

        // Skip the first 2 words in the file (version and sample count, which are not used).
        let mut words = cal_contents.trim().split_ascii_whitespace().skip(2);
        let slope: f32 = words
            .next()
            .ok_or_else(|| format_err!("Missing slope"))?
            .parse()
            .context("Failed to parse slope")?;

        if !slope.is_finite() {
            bail!("Slope must not be NaN or Infinity");
        }

        let intercept: f32 = words
            .next()
            .ok_or_else(|| format_err!("Missing intercept"))?
            .parse()
            .context("Failed to parse intercept")?;

        if !intercept.is_finite() {
            bail!("Intercept must not be NaN or Infinity");
        }

        Ok(Parameters { slope, intercept })
    }

    pub(crate) fn leds(&self) -> &LedMap {
        &self.leds
    }

    pub(crate) fn off(&self) -> Rgbc<Parameters> {
        self.off
    }

    pub(crate) fn all_on(&self) -> Rgbc<Parameters> {
        self.all_on
    }

    pub(crate) fn calibrated_slope(&self) -> Rgbc<f32> {
        self.calibrated_slope
    }
}

/// Settings used to configure a light sensor.
#[derive(Copy, Clone, Deserialize, Debug)]
pub(crate) struct AdjustmentSetting {
    /// Rgbc integration time.
    pub(crate) atime: u32,
    /// Rgbc gain control.
    pub(crate) gain: u32,
}

#[cfg(test)]
mod types_tests;
