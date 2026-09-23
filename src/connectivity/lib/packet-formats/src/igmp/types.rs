// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! IGMP parsing and serialization helper types.

use core::time::Duration;

use super::IgmpMaxRespCode;
use crate::gmp::{ExactConversionError, LinExpConversion, OverflowError};

/// Thin wrapper around `u8` that provides maximum response time parsing
/// for IGMP v2.
///
/// Provides conversions to and from `Duration` for parsing and
/// and serializing in the correct format, following that the underlying `u8`
/// is the maximum response time in tenths of seconds.
#[derive(Debug, PartialEq)]
pub struct IgmpResponseTimeV2(u8);

impl IgmpMaxRespCode for IgmpResponseTimeV2 {
    fn as_code(&self) -> u8 {
        self.0
    }

    fn from_code(code: u8) -> Self {
        Self(code)
    }
}

impl TryFrom<Duration> for IgmpResponseTimeV2 {
    type Error = OverflowError;

    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        let tenths = value.as_millis() / 100;
        Ok(Self(tenths.try_into().map_err(|_| OverflowError)?))
    }
}

impl From<IgmpResponseTimeV2> for Duration {
    fn from(value: IgmpResponseTimeV2) -> Duration {
        let v: u64 = value.0.into();
        Self::from_millis(v * 100)
    }
}

/// Thin wrapper around u8 that provides maximum response time parsing
/// for IGMP v3.
///
/// Provides conversions to and from `Duration` for parsing and
/// and serializing in the correct format.
#[derive(Debug, PartialEq, Copy, Clone)]
pub struct IgmpResponseTimeV3(u8);

impl IgmpMaxRespCode for IgmpResponseTimeV3 {
    fn as_code(&self) -> u8 {
        self.0
    }

    fn from_code(code: u8) -> Self {
        Self(code)
    }
}

impl LinExpConversion<Duration> for IgmpResponseTimeV3 {
    const NUM_MANT_BITS: u8 = 4;
    const NUM_EXP_BITS: u8 = 3;

    fn lossy_try_from(value: Duration) -> Result<Self, OverflowError> {
        let tenths: u32 = (value.as_millis() / 100).try_into().map_err(|_| OverflowError)?;
        let code = Self::lossy_try_from_expanded(tenths)?.try_into().map_err(|_| OverflowError)?;
        Ok(Self(code))
    }
}

impl IgmpResponseTimeV3 {
    /// Creates a new `IgmpResponseTimeV3` allowing lossy conversion from
    /// `value`.
    pub fn new_lossy(value: Duration) -> Result<Self, OverflowError> {
        Self::lossy_try_from(value)
    }

    /// Creates a new `IgmpResponseTimeV3` rejecting lossy conversion from
    /// `value`.
    pub fn new_exact(value: Duration) -> Result<Self, ExactConversionError> {
        Self::exact_try_from(value)
    }
}

impl From<IgmpResponseTimeV3> for Duration {
    fn from(IgmpResponseTimeV3(value): IgmpResponseTimeV3) -> Duration {
        // ResponseTime v3 is represented in tenths of seconds and coded
        // with specific floating point schema.
        let tenths: u64 = IgmpResponseTimeV3::to_expanded(value.into()).into();
        Duration::from_millis(tenths * 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn parse_and_serialize_code_v2() {
        for code in 0..=255 {
            let response = IgmpResponseTimeV2::from_code(code);
            assert_eq!(response.as_code(), code);
            let dur = Duration::from(response);
            let back = IgmpResponseTimeV2::try_from(dur).unwrap();
            assert_eq!(dur.as_millis(), u128::from(code) * 100);
            assert_eq!(code, back.as_code());
        }

        // test that anything larger than max u8 tenths of seconds will cause
        // try_from to fail:
        assert_eq!(
            IgmpResponseTimeV2::try_from(Duration::from_millis((u64::from(u8::MAX) + 1) * 100,)),
            Err(OverflowError)
        );
    }

    #[test]
    pub fn parse_and_serialize_code_v3() {
        let r = Duration::from(IgmpResponseTimeV3::from_code(0x80 | 0x01));
        assert_eq!(r.as_millis(), 13600);
        let t = IgmpResponseTimeV3::new_lossy(Duration::from_millis((128 + 8) * 100)).unwrap();
        assert_eq!(t.as_code(), (0x80 | 0x01));
        for code in 0..=255 {
            let response = IgmpResponseTimeV3::from_code(code);
            assert_eq!(response.as_code(), code);
            let dur = Duration::from(response);
            let back = IgmpResponseTimeV3::new_lossy(dur).unwrap();
            assert_eq!(code, back.as_code());
        }

        // test that anything larger than max u8 tenths of seconds will cause
        // try_from to fail:
        assert_eq!(
            IgmpResponseTimeV3::new_lossy(Duration::from_millis(
                (IgmpResponseTimeV3::MAX_VALUE as u64 + 1) * 100,
            )),
            Err(OverflowError)
        );
    }
}
