// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// BLE standard 16-bit, 32-bit, or 128-bit Type UUID representation.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uuid {
    value: [u8; 16],
}

impl Uuid {
    /// The size of a 16-bit UUID in bytes.
    const SIZE_16: usize = 2;

    /// The size of a 32-bit UUID in bytes.
    const SIZE_32: usize = 4;

    /// The size of a 128-bit UUID in bytes.
    const SIZE_128: usize = 16;

    /// The offset (in bytes) where a 16-bit or 32-bit UUID value is inserted into
    /// the 128-bit Base UUID (96 bits = 12 bytes offset).
    ///
    /// Both 16-bit and 32-bit SIG UUIDs are converted to 128-bit using the formula:
    ///   128_bit_value = (value * 2^96) + Bluetooth_Base_UUID
    ///
    /// (see Bluetooth Core Spec v6.0, Vol 3, Part B, Section 2.5.1)
    const BASE_OFFSET: usize = 12;

    /// The Bluetooth SIG Base UUID in little-endian byte format:
    /// "00000000-0000-1000-8000-00805F9B34FB"
    const BASE_UUID_LE: [u8; 16] = [
        0xFB, 0x34, 0x9B, 0x5F, 0x80, 0x00, 0x00, 0x80, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ];

    /// Creates a standard 16-bit UUID by expanding it to 128-bit.
    pub const fn from_u16(value: u16) -> Self {
        let mut bytes = Self::BASE_UUID_LE;
        // Convert the 16-bit value into little-endian bytes.
        let val_bytes = value.to_le_bytes();

        // Shift the 16-bit value left by 12 bytes (96 bits) and add it to the base.
        bytes[Self::BASE_OFFSET] = val_bytes[0];
        bytes[Self::BASE_OFFSET + 1] = val_bytes[1];
        Self { value: bytes }
    }

    /// Creates a standard 32-bit UUID by expanding it to 128-bit.
    pub const fn from_u32(value: u32) -> Self {
        let mut bytes = Self::BASE_UUID_LE;
        // Convert the 32-bit value into little-endian bytes.
        let val_bytes = value.to_le_bytes();

        // Shift the 32-bit value left by 12 bytes (96 bits) and add it to the base.
        bytes[Self::BASE_OFFSET] = val_bytes[0];
        bytes[Self::BASE_OFFSET + 1] = val_bytes[1];
        bytes[Self::BASE_OFFSET + 2] = val_bytes[2];
        bytes[Self::BASE_OFFSET + 3] = val_bytes[3];
        Self { value: bytes }
    }

    /// Creates a custom 128-bit UUID from little-endian bytes.
    pub const fn from_le_bytes(bytes: [u8; 16]) -> Self {
        Self { value: bytes }
    }

    /// Creates a custom 128-bit UUID from a u128 integer value.
    pub const fn from_u128(value: u128) -> Self {
        Self { value: value.to_le_bytes() }
    }

    /// Returns the full 128-bit little-endian byte representation of this UUID.
    pub const fn to_128_bytes(&self) -> [u8; 16] {
        self.value
    }

    /// Returns `true` if this UUID represents a standard 16-bit SIG UUID.
    pub fn is_u16(&self) -> bool {
        self.value[..Self::BASE_OFFSET] == Self::BASE_UUID_LE[..Self::BASE_OFFSET]
            && self.value[Self::BASE_OFFSET + Self::SIZE_16] == 0
            && self.value[Self::BASE_OFFSET + Self::SIZE_16 + 1] == 0
    }

    /// Returns the canonical byte representation of the UUID as a slice.
    ///
    /// Returns a 2-byte slice if this is a 16-bit SIG UUID, otherwise returns
    /// the full 16-byte representation.
    pub fn as_bytes(&self) -> &[u8] {
        if self.is_u16() {
            &self.value[Self::BASE_OFFSET..Self::BASE_OFFSET + Self::SIZE_16]
        } else {
            &self.value
        }
    }
}

/// Error type for UUID conversion failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UuidError {
    /// The UUID cannot be represented in the target format (e.g. not a SIG Base range or does not fit).
    InvalidConversion,
}

impl TryFrom<Uuid> for u16 {
    type Error = UuidError;

    fn try_from(uuid: Uuid) -> Result<Self, Self::Error> {
        if uuid.value[..Uuid::BASE_OFFSET] != Uuid::BASE_UUID_LE[..Uuid::BASE_OFFSET] {
            return Err(UuidError::InvalidConversion);
        }
        // Bytes 14 and 15 must be 0 (meaning the value fits in 16 bits and doesn't require 32 bits).
        if uuid.value[Uuid::BASE_OFFSET + Uuid::SIZE_16] != 0
            || uuid.value[Uuid::BASE_OFFSET + Uuid::SIZE_16 + 1] != 0
        {
            return Err(UuidError::InvalidConversion);
        }
        // Extract the 16-bit UUID value bytes from the 128-bit layout.
        let bytes: [u8; 2] =
            uuid.value[Uuid::BASE_OFFSET..Uuid::BASE_OFFSET + Uuid::SIZE_16].try_into().unwrap();
        Ok(u16::from_le_bytes(bytes))
    }
}

impl TryFrom<Uuid> for u32 {
    type Error = UuidError;

    fn try_from(uuid: Uuid) -> Result<Self, Self::Error> {
        if uuid.value[..Uuid::BASE_OFFSET] != Uuid::BASE_UUID_LE[..Uuid::BASE_OFFSET] {
            return Err(UuidError::InvalidConversion);
        }
        // Extract the 32-bit UUID value bytes from the 128-bit layout.
        let bytes: [u8; 4] =
            uuid.value[Uuid::BASE_OFFSET..Uuid::BASE_OFFSET + Uuid::SIZE_32].try_into().unwrap();
        Ok(u32::from_le_bytes(bytes))
    }
}

impl From<Uuid> for [u8; 16] {
    fn from(uuid: Uuid) -> Self {
        uuid.value
    }
}

impl TryFrom<Uuid> for [u8; 2] {
    type Error = UuidError;

    fn try_from(uuid: Uuid) -> Result<Self, Self::Error> {
        Ok(u16::try_from(uuid)?.to_le_bytes())
    }
}

impl TryFrom<Uuid> for [u8; 4] {
    type Error = UuidError;

    fn try_from(uuid: Uuid) -> Result<Self, Self::Error> {
        Ok(u32::try_from(uuid)?.to_le_bytes())
    }
}

impl TryFrom<&[u8]> for Uuid {
    type Error = UuidError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        if bytes.len() == Self::SIZE_16 {
            let arr: [u8; Self::SIZE_16] =
                bytes.try_into().map_err(|_| UuidError::InvalidConversion)?;
            Ok(Self::from_u16(u16::from_le_bytes(arr)))
        } else if bytes.len() == Self::SIZE_32 {
            let arr: [u8; Self::SIZE_32] =
                bytes.try_into().map_err(|_| UuidError::InvalidConversion)?;
            Ok(Self::from_u32(u32::from_le_bytes(arr)))
        } else if bytes.len() == Self::SIZE_128 {
            let arr: [u8; Self::SIZE_128] =
                bytes.try_into().map_err(|_| UuidError::InvalidConversion)?;
            Ok(Self::from_le_bytes(arr))
        } else {
            Err(UuidError::InvalidConversion)
        }
    }
}

impl From<u16> for Uuid {
    fn from(value: u16) -> Self {
        Self::from_u16(value)
    }
}

impl From<u32> for Uuid {
    fn from(value: u32) -> Self {
        Self::from_u32(value)
    }
}

impl From<u128> for Uuid {
    fn from(value: u128) -> Self {
        Self::from_u128(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uuid_to_128_bytes() {
        // BATTERY SERVICE 16-bit UUID is 0x180F
        let uuid_16 = Uuid::from_u16(0x180F);
        let expected_128 = [
            0xFB, 0x34, 0x9B, 0x5F, 0x80, 0x00, 0x00, 0x80, 0x00, 0x10, 0x00, 0x00, 0x0F, 0x18,
            0x00, 0x00,
        ];
        assert_eq!(uuid_16.to_128_bytes(), expected_128);

        // Custom 128-bit UUID
        let custom_bytes = [1u8; 16];
        let uuid_128 = Uuid::from_le_bytes(custom_bytes);
        assert_eq!(uuid_128.to_128_bytes(), custom_bytes);
    }

    #[test]
    fn test_uuid_try_from() {
        // 16-bit SIG UUID
        let uuid_16 = Uuid::from_u16(0x180F);
        assert_eq!(u16::try_from(uuid_16), Ok(0x180F));
        assert_eq!(u32::try_from(uuid_16), Ok(0x0000180F));

        // 32-bit SIG UUID
        let uuid_32 = Uuid::from_u32(0x12345678);
        assert_eq!(u16::try_from(uuid_32), Err(UuidError::InvalidConversion));
        assert_eq!(u32::try_from(uuid_32), Ok(0x12345678));

        // Custom 128-bit UUID
        let uuid_128 = Uuid::from_le_bytes([1u8; 16]);
        assert_eq!(u16::try_from(uuid_128), Err(UuidError::InvalidConversion));
        assert_eq!(u32::try_from(uuid_128), Err(UuidError::InvalidConversion));

        // Verify little-endian byte array conversions and error states.
        assert_eq!(<[u8; 2]>::try_from(uuid_16), Ok([0x0F, 0x18]));
        assert_eq!(<[u8; 4]>::try_from(uuid_16), Ok([0x0F, 0x18, 0x00, 0x00]));
        assert_eq!(<[u8; 2]>::try_from(uuid_32), Err(UuidError::InvalidConversion));
        assert_eq!(<[u8; 4]>::try_from(uuid_32), Ok([0x78, 0x56, 0x34, 0x12]));
        assert_eq!(<[u8; 2]>::try_from(uuid_128), Err(UuidError::InvalidConversion));
        assert_eq!(<[u8; 4]>::try_from(uuid_128), Err(UuidError::InvalidConversion));
    }

    #[test]
    fn test_uuid_from() {
        let u16_val = 0x180F;
        let uuid_16: Uuid = u16_val.into();
        assert_eq!(uuid_16, Uuid::from_u16(u16_val));

        let u32_val = 0x12345678;
        let uuid_32: Uuid = u32_val.into();
        assert_eq!(uuid_32, Uuid::from_u32(u32_val));

        let u128_val = 0x123456789abcdef01122334455667788;
        let uuid_128: Uuid = u128_val.into();
        assert_eq!(uuid_128, Uuid::from_u128(u128_val));
    }

    #[test]
    fn test_uuid_try_from_slice() {
        // Valid 16-bit UUID slice
        let uuid_16_bytes = [0x0F, 0x18];
        let uuid_16 = Uuid::try_from(&uuid_16_bytes[..]);
        assert_eq!(uuid_16, Ok(Uuid::from_u16(0x180F)));

        // Valid 128-bit UUID slice
        let uuid_128_bytes = [1u8; 16];
        let uuid_128 = Uuid::try_from(&uuid_128_bytes[..]);
        assert_eq!(uuid_128, Ok(Uuid::from_le_bytes(uuid_128_bytes)));

        // Valid 32-bit UUID slice
        let uuid_32_bytes = [0x78, 0x56, 0x34, 0x12];
        let uuid_32 = Uuid::try_from(&uuid_32_bytes[..]);
        assert_eq!(uuid_32, Ok(Uuid::from_u32(0x12345678)));

        // Invalid length slices (e.g. 3 bytes or 0 bytes)
        assert_eq!(Uuid::try_from(&[1, 2, 3][..]), Err(UuidError::InvalidConversion));
        assert_eq!(Uuid::try_from(&[][..]), Err(UuidError::InvalidConversion));
    }

    #[cfg(feature = "testing")]
    mod prop_tests {
        use super::*;
        use proptest::proptest;
        proptest! {
            #[test]
            fn prop_u16_round_trip(val: u16) {
                let uuid = Uuid::from_u16(val);
                let extracted_val = u16::try_from(uuid).expect("u16 round trip failed");
                assert_eq!(val, extracted_val);
                // Also check that it converts to the expected u32
                let extracted_u32 = u32::try_from(uuid).expect("u16 to u32 failed");
                assert_eq!(val as u32, extracted_u32);
            }
            #[test]
            fn prop_u32_round_trip(val: u32) {
                let uuid = Uuid::from_u32(val);
                let extracted_val = u32::try_from(uuid).expect("u32 round trip failed");
                assert_eq!(val, extracted_val);
            }
            #[test]
            fn prop_u32_from_u16_is_same_as_u16_from_u16(val: u16) {
                let uuid_from_u16 = Uuid::from_u16(val);
                let uuid_from_u32 = Uuid::from_u32(val as u32);
                assert_eq!(uuid_from_u16, uuid_from_u32);
            }
        }
    }
}
