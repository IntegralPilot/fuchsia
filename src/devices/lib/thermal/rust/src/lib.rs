// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod linear_lookup_table;
pub use linear_lookup_table::{LinearLookupTable, LookupTableEntry};

// NtcChannel struct layout constants (C++ compatibility)
pub const NTC_CHANNEL_SIZE_BYTES: usize = 64;
pub const NTC_CHANNEL_NAME_LEN: usize = 50;
pub const NTC_CHANNEL_NAME_OFFSET_BYTES: usize = 12;

// NtcInfo struct layout constants (C++ compatibility)
pub const NTC_INFO_SIZE_BYTES: usize = 1652;
pub const NTC_INFO_PART_LEN: usize = 50;
pub const NTC_INFO_PROFILE_LEN: usize = 200;
pub const NTC_INFO_PROFILE_OFFSET_BYTES: usize = 52;
pub const NTC_TABLE_SIZE_BYTES: usize = 8;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Error {
    #[error(
        "Invalid channel metadata size: expected non-zero multiple of {}, got {}",
        expected,
        got
    )]
    InvalidChannelSize { expected: usize, got: usize },

    #[error("Invalid info metadata size: expected non-zero multiple of {}, got {}", expected, got)]
    InvalidInfoSize { expected: usize, got: usize },

    #[error("Invalid normalized sample: expected [0.0, 1.0), got {0}")]
    InvalidNormalizedSample(f32),

    #[error("Profile lookup table is empty")]
    EmptyProfile,

    #[error("Profile contains NaN values")]
    ProfileContainsNan,

    #[error("Profile contains duplicate keys")]
    ProfileContainsDuplicateKeys,

    #[error("Profile is not monotonic")]
    ProfileNotMonotonic,
}

// TODO(b/448631407): Replace with a FIDL metadata type.
#[derive(Clone, Debug, PartialEq)]
pub struct NtcTable {
    pub temperature_c: f32,
    pub resistance_ohm: u32,
}

// TODO(b/448631407): Replace with a FIDL metadata type.
#[derive(Clone, Debug, PartialEq)]
pub struct NtcChannel {
    pub adc_channel: u32,
    pub pullup_ohms: u32,
    pub profile_idx: u32,
    pub name: String,
}

impl NtcChannel {
    /// Parses a serialized array of `NtcChannel` C-struct bytes.
    pub fn deserialize(bytes: &[u8]) -> Result<Vec<Self>, Error> {
        if bytes.is_empty() || bytes.len() % NTC_CHANNEL_SIZE_BYTES != 0 {
            return Err(Error::InvalidChannelSize {
                expected: NTC_CHANNEL_SIZE_BYTES,
                got: bytes.len(),
            });
        }
        let mut channels = Vec::new();
        for chunk in bytes.chunks_exact(NTC_CHANNEL_SIZE_BYTES) {
            let adc_channel = u32::from_ne_bytes(chunk[0..4].try_into().unwrap());
            let pullup_ohms = u32::from_ne_bytes(chunk[4..8].try_into().unwrap());
            let profile_idx = u32::from_ne_bytes(chunk[8..12].try_into().unwrap());
            let name_bytes = &chunk[NTC_CHANNEL_NAME_OFFSET_BYTES
                ..NTC_CHANNEL_NAME_OFFSET_BYTES + NTC_CHANNEL_NAME_LEN];
            let len = name_bytes.iter().position(|&b| b == 0).unwrap_or(NTC_CHANNEL_NAME_LEN);
            let name = String::from_utf8_lossy(&name_bytes[..len]).into_owned();
            channels.push(NtcChannel { adc_channel, pullup_ohms, profile_idx, name });
        }
        Ok(channels)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NtcInfo {
    pub part: String,
    pub profile: Vec<NtcTable>,
}

impl NtcInfo {
    /// Parses a serialized array of `NtcInfo` C-struct bytes.
    pub fn deserialize(bytes: &[u8]) -> Result<Vec<Self>, Error> {
        if bytes.is_empty() || bytes.len() % NTC_INFO_SIZE_BYTES != 0 {
            return Err(Error::InvalidInfoSize { expected: NTC_INFO_SIZE_BYTES, got: bytes.len() });
        }
        let mut infos = Vec::new();
        for chunk in bytes.chunks_exact(NTC_INFO_SIZE_BYTES) {
            let part_bytes = &chunk[0..NTC_INFO_PART_LEN];
            let len = part_bytes.iter().position(|&b| b == 0).unwrap_or(NTC_INFO_PART_LEN);
            let part = String::from_utf8_lossy(&part_bytes[..len]).into_owned();

            let mut profile = Vec::new();
            for i in 0..NTC_INFO_PROFILE_LEN {
                let offset = NTC_INFO_PROFILE_OFFSET_BYTES + i * NTC_TABLE_SIZE_BYTES;
                let temp_bytes = &chunk[offset..offset + 4];
                let res_bytes = &chunk[offset + 4..offset + 8];
                let temperature_c = f32::from_ne_bytes(temp_bytes.try_into().unwrap());
                let resistance_ohm = u32::from_ne_bytes(res_bytes.try_into().unwrap());
                if resistance_ohm != 0 {
                    profile.push(NtcTable { temperature_c, resistance_ohm });
                }
            }
            infos.push(NtcInfo { part, profile });
        }
        Ok(infos)
    }
}

pub struct Ntc {
    lut: LinearLookupTable,
    pullup_ohms: u32,
}

impl Ntc {
    pub fn new(profile: Vec<NtcTable>, pullup_ohms: u32) -> Result<Self, Error> {
        let lut_entries = profile
            .into_iter()
            .map(|entry| LookupTableEntry {
                x: entry.temperature_c,
                y: entry.resistance_ohm as f32,
            })
            .collect();
        let lut = LinearLookupTable::new(lut_entries)?;
        Ok(Self { lut, pullup_ohms })
    }

    /// We use a normalized sample [0-1] to prevent having to worry about adc resolution
    /// in this library. This assumes the call site will normalize the value appropriately.
    /// Since the thermistor is in series with a pullup resistor, we must convert our sample
    /// value to a resistance then lookup in the profile table.
    pub fn get_temperature_celsius(&self, norm_sample: f32) -> Result<f32, Error> {
        // norm_sample should never be 1.0 because that would mean there is no pullup resistor. Also,
        // this ensures that division below is valid.
        if norm_sample < 0.0 || norm_sample >= 1.0 {
            return Err(Error::InvalidNormalizedSample(norm_sample));
        }
        let ratio = norm_sample / (1.0 - norm_sample);
        let resistance = ratio * self.pullup_ohms as f32;
        self.lut.lookup_x(resistance)
    }

    /// Returns the normalized sample. Convert from resistance.
    pub fn get_normalized_sample(&self, temperature_c: f32) -> Result<f32, Error> {
        let resistance = self.lut.lookup_y(temperature_c)?;
        let sample = resistance / (resistance + self.pullup_ohms as f32);
        Ok(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_PULLUP_OHMS: u32 = 47000;

    macro_rules! assert_float_eq {
        ($left:expr, $right:expr $(,)? ) => {
            let left = $left;
            let right = $right;
            assert!(
                (left - right).abs() < 1e-5,
                "assertion failed: `(left == right)`\n  left: `{:?}`,\n right: `{:?}`",
                left,
                right
            );
        };
    }

    fn get_fake_profile() -> Vec<NtcTable> {
        vec![
            NtcTable { temperature_c: -40.0, resistance_ohm: 4397119 },
            NtcTable { temperature_c: -35.0, resistance_ohm: 3088599 },
            NtcTable { temperature_c: -30.0, resistance_ohm: 2197225 },
            NtcTable { temperature_c: -25.0, resistance_ohm: 1581881 },
            NtcTable { temperature_c: -20.0, resistance_ohm: 1151037 },
            NtcTable { temperature_c: -15.0, resistance_ohm: 846579 },
            NtcTable { temperature_c: -10.0, resistance_ohm: 628988 },
            NtcTable { temperature_c: -5.0, resistance_ohm: 471632 },
            NtcTable { temperature_c: 0.0, resistance_ohm: 357012 },
            NtcTable { temperature_c: 5.0, resistance_ohm: 272500 },
            NtcTable { temperature_c: 10.0, resistance_ohm: 209710 },
            NtcTable { temperature_c: 15.0, resistance_ohm: 162651 },
            NtcTable { temperature_c: 20.0, resistance_ohm: 127080 },
            NtcTable { temperature_c: 25.0, resistance_ohm: 100000 },
            NtcTable { temperature_c: 30.0, resistance_ohm: 79222 },
            NtcTable { temperature_c: 35.0, resistance_ohm: 63167 },
            NtcTable { temperature_c: 40.0, resistance_ohm: 50677 },
            NtcTable { temperature_c: 45.0, resistance_ohm: 40904 },
            NtcTable { temperature_c: 50.0, resistance_ohm: 33195 },
            NtcTable { temperature_c: 55.0, resistance_ohm: 27091 },
            NtcTable { temperature_c: 60.0, resistance_ohm: 22224 },
            NtcTable { temperature_c: 65.0, resistance_ohm: 18323 },
            NtcTable { temperature_c: 70.0, resistance_ohm: 15184 },
            NtcTable { temperature_c: 75.0, resistance_ohm: 12635 },
            NtcTable { temperature_c: 80.0, resistance_ohm: 10566 },
            NtcTable { temperature_c: 85.0, resistance_ohm: 8873 },
            NtcTable { temperature_c: 90.0, resistance_ohm: 7481 },
            NtcTable { temperature_c: 95.0, resistance_ohm: 6337 },
            NtcTable { temperature_c: 100.0, resistance_ohm: 5384 },
            NtcTable { temperature_c: 105.0, resistance_ohm: 4594 },
            NtcTable { temperature_c: 110.0, resistance_ohm: 3934 },
            NtcTable { temperature_c: 115.0, resistance_ohm: 3380 },
            NtcTable { temperature_c: 120.0, resistance_ohm: 2916 },
            NtcTable { temperature_c: 125.0, resistance_ohm: 2522 },
        ]
    }

    #[test]
    fn test_get_temperature_celsius_empty_profile() {
        let result = Ntc::new(vec![], 47000);
        assert_eq!(result.err(), Some(Error::EmptyProfile));
    }

    #[test]
    fn test_get_temperature_celsius_invalid_low() {
        let ntc = Ntc::new(get_fake_profile(), FAKE_PULLUP_OHMS).unwrap();
        assert_eq!(
            ntc.get_temperature_celsius(-0.5).err(),
            Some(Error::InvalidNormalizedSample(-0.5))
        );
    }

    #[test]
    fn test_get_temperature_celsius_invalid_high() {
        let ntc = Ntc::new(get_fake_profile(), FAKE_PULLUP_OHMS).unwrap();
        assert_eq!(
            ntc.get_temperature_celsius(1.0).err(),
            Some(Error::InvalidNormalizedSample(1.0))
        );
    }

    #[test]
    fn test_get_temperature_celsius_low() {
        let ntc = Ntc::new(get_fake_profile(), FAKE_PULLUP_OHMS).unwrap();
        let temp = ntc.get_temperature_celsius(0.0).unwrap();
        assert_float_eq!(temp, 125.0);
    }

    #[test]
    fn test_get_temperature_celsius_high() {
        let ntc = Ntc::new(get_fake_profile(), FAKE_PULLUP_OHMS).unwrap();
        let temp = ntc.get_temperature_celsius(0.99).unwrap();
        assert_float_eq!(temp, -40.0);
    }

    #[test]
    fn test_get_temperature_celsius_middle() {
        let ntc = Ntc::new(get_fake_profile(), FAKE_PULLUP_OHMS).unwrap();
        let temp = ntc.get_temperature_celsius(0.88).unwrap();
        assert_float_eq!(temp, 0.7303901);
    }

    #[test]
    fn test_get_normalized_sample_low() {
        let ntc = Ntc::new(get_fake_profile(), FAKE_PULLUP_OHMS).unwrap();
        let sample = ntc.get_normalized_sample(-45.0).unwrap();
        assert_float_eq!(sample, 0.9894242);
    }

    #[test]
    fn test_get_normalized_sample_high() {
        let ntc = Ntc::new(get_fake_profile(), FAKE_PULLUP_OHMS).unwrap();
        let sample = ntc.get_normalized_sample(125.0).unwrap();
        assert_float_eq!(sample, 0.05092686);
    }

    #[test]
    fn test_get_normalized_sample_middle() {
        let ntc = Ntc::new(get_fake_profile(), FAKE_PULLUP_OHMS).unwrap();
        let sample = ntc.get_normalized_sample(2.5).unwrap();
        assert_float_eq!(sample, 0.8700782);
    }

    #[test]
    fn test_deserialize_ntc_channel_invalid_size() {
        let bytes = vec![0u8; 10]; // invalid size, not a multiple of 64
        let result = NtcChannel::deserialize(&bytes);
        assert_eq!(
            result.err(),
            Some(Error::InvalidChannelSize { expected: NTC_CHANNEL_SIZE_BYTES, got: 10 })
        );
        assert_eq!(
            format!("{}", Error::InvalidChannelSize { expected: NTC_CHANNEL_SIZE_BYTES, got: 10 }),
            "Invalid channel metadata size: expected non-zero multiple of 64, got 10"
        );
    }

    #[test]
    fn test_deserialize_ntc_info_invalid_size() {
        let bytes = vec![0u8; 10]; // invalid size, not a multiple of 1652
        let result = NtcInfo::deserialize(&bytes);
        assert_eq!(
            result.err(),
            Some(Error::InvalidInfoSize { expected: NTC_INFO_SIZE_BYTES, got: 10 })
        );
        assert_eq!(
            format!("{}", Error::InvalidInfoSize { expected: NTC_INFO_SIZE_BYTES, got: 10 }),
            "Invalid info metadata size: expected non-zero multiple of 1652, got 10"
        );
    }
}
