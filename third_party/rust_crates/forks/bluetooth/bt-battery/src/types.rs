// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::packet_encoding::{Decodable, Error as PacketError};
use bt_common::Uuid;

/// The UUID of the GATT battery service.
/// Defined in Assigned Numbers Section 3.4.2.
pub const BATTERY_SERVICE_UUID: Uuid = Uuid::from_u16(0x180f);

/// The UUID of the GATT Battery level characteristic.
/// Defined in Assigned Numbers Section 3.8.1.
pub const BATTERY_LEVEL_UUID: Uuid = Uuid::from_u16(0x2a19);

pub(crate) const READ_CHARACTERISTIC_BUFFER_SIZE: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct BatteryLevel(pub u8);

impl Decodable for BatteryLevel {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < 1 {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }

        let level_percent = buf[0].clamp(0, 100);
        (Ok(BatteryLevel(level_percent)), 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_battery_level_success() {
        let buf = [55];
        let (parsed, parsed_size) = BatteryLevel::decode(&buf);
        assert_eq!(parsed, Ok(BatteryLevel(55)));
        assert_eq!(parsed_size, 1);
    }

    #[test]
    fn decode_large_battery_level_clamped() {
        let buf = [125]; // Too large, expected to be a percentage value.
        let (parsed, parsed_size) = BatteryLevel::decode(&buf);
        assert_eq!(parsed, Ok(BatteryLevel(100)));
        assert_eq!(parsed_size, 1);
    }

    #[test]
    fn decode_large_buf_success() {
        let large_buf = [19, 0]; // Only expect a single u8 for the level.
        let (parsed, parsed_size) = BatteryLevel::decode(&large_buf);
        assert_eq!(parsed, Ok(BatteryLevel(19))); // Only the first byte should be read.
        assert_eq!(parsed_size, 1);
    }

    #[test]
    fn decode_invalid_battery_level_buf_is_error() {
        let buf = [];
        let (result, parsed_size) = BatteryLevel::decode(&buf);
        assert_eq!(result, Err(PacketError::UnexpectedDataLength));
        assert_eq!(parsed_size, 0);
    }
}
