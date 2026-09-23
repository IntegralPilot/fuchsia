// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Product and firmware version identification registers for Goodix GT6853.

use crate::data_types::traits::{AddressableRegister, ReadableRegister};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Hardware product identification, sensor ID, and firmware version information.
//
// @cite(gt6853-hardware-description): sec=3.1 title="Product Identification Registers"
// @cite(gt6853-programming-guide): sec=3.1 title="Product Version"
// @alias(gt6853-hardware-description): theirs="Product Identification Registers"
// @alias(gt6853-programming-guide): theirs="Product Version"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct ProductIdentification {
    /// Stored as null-padded ASCII characters.
    pub mask_product_id: [u8; 6],
    /// BCD-encoded.
    pub mask_firmware_major_version: u8,
    /// BCD-encoded.
    pub mask_firmware_minor_version: u8,
    pub mask_firmware_internal_version: u8,
    /// Stored as null-padded ASCII characters.
    pub patch_product_id: [u8; 8],
    pub customer_id: u8,
    /// BCD-encoded.
    pub patch_firmware_major_version: u8,
    /// BCD-encoded.
    pub patch_firmware_minor_version: u8,
    pub patch_firmware_internal_version: u8,
    pub sensor_id: u8,
    pub active_config_id: [u8; 4],
    pub reserved: [u8; 45],
    /// Checksum byte covering the 72-byte product identification block.
    ///
    /// Valid when the sum of all 72 bytes in this register block equals 0 modulo 256.
    pub config_checksum: u8,
}

impl AddressableRegister for ProductIdentification {
    const ADDRESS: u16 = 0x452C;
}

impl ReadableRegister for ProductIdentification {}

/// Converts a 2-digit packed BCD byte to a decimal integer.
const fn bcd_to_u8(val: u8) -> u8 {
    let tens = val >> 4;
    let units = val & 0x0F;
    (tens * 10) + units
}

impl ProductIdentification {
    /// Returns `true` iff [`Self::config_checksum`] is valid.
    pub fn is_checksum_valid(&self) -> bool {
        let sum = self.as_bytes().iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
        sum == 0
    }

    /// Returns the mask product ID as a UTF-8 string slice, trimming any trailing null bytes.
    pub fn mask_product_id_as_str(&self) -> Result<&str, core::str::Utf8Error> {
        let len =
            self.mask_product_id.iter().position(|&b| b == 0).unwrap_or(self.mask_product_id.len());
        core::str::from_utf8(&self.mask_product_id[..len])
    }

    /// Returns the patch product ID as a UTF-8 string slice, trimming any trailing null bytes.
    pub fn patch_product_id_as_str(&self) -> Result<&str, core::str::Utf8Error> {
        let len = self
            .patch_product_id
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.patch_product_id.len());
        core::str::from_utf8(&self.patch_product_id[..len])
    }

    /// Formats the mask firmware version as a human-readable version string.
    pub fn mask_firmware_version(&self) -> String {
        format!(
            "{}.{}.{}",
            bcd_to_u8(self.mask_firmware_major_version),
            bcd_to_u8(self.mask_firmware_minor_version),
            self.mask_firmware_internal_version
        )
    }

    /// Formats the patch firmware version as a human-readable version string.
    pub fn patch_firmware_version(&self) -> String {
        format!(
            "{}.{}.{}",
            bcd_to_u8(self.patch_firmware_major_version),
            bcd_to_u8(self.patch_firmware_minor_version),
            self.patch_firmware_internal_version
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_product_identification_size() {
        assert_eq!(core::mem::size_of::<ProductIdentification>(), 72);
    }

    #[test]
    fn test_product_identification_checksum_validation() {
        let mut data = [0u8; 72];
        data[0] = 0x10;
        data[1] = 0x20;
        // Sum of first two bytes is 0x30. Set checksum byte so total sum % 256 == 0.
        data[71] = 0xD0; // 0x30 + 0xD0 = 0x100 -> 0 (mod 256)

        let info = ProductIdentification::read_from_bytes(&data).unwrap();
        assert!(info.is_checksum_valid());

        // Corrupt a byte
        data[2] = 0x01;
        let corrupted_info = ProductIdentification::read_from_bytes(&data).unwrap();
        assert!(!corrupted_info.is_checksum_valid());
    }

    #[test]
    fn test_product_identification_version_strings() {
        let mut data = [0u8; 72];
        // Mask firmware: major=0x01 (1), minor=0x02 (2), internal=3
        data[6] = 0x01;
        data[7] = 0x02;
        data[8] = 3;

        // Patch firmware: major=0x10 (10 in BCD), minor=0x25 (25 in BCD), internal=5
        data[18] = 0x10;
        data[19] = 0x25;
        data[20] = 5;

        let info = ProductIdentification::read_from_bytes(&data).unwrap();
        assert_eq!(info.mask_firmware_version(), "1.2.3");
        assert_eq!(info.patch_firmware_version(), "10.25.5");
    }

    #[test]
    fn test_product_identification_string_conversions() {
        let mut data = [0u8; 72];
        // "GT6853" in mask_product_id
        data[0..6].copy_from_slice(b"GT6853");
        // "PATCH1\0\0" in patch_product_id
        data[9..15].copy_from_slice(b"PATCH1");
        data[15] = 0;
        data[16] = 0;

        let info = ProductIdentification::read_from_bytes(&data).unwrap();
        assert_eq!(info.mask_product_id_as_str(), Ok("GT6853"));
        assert_eq!(info.patch_product_id_as_str(), Ok("PATCH1"));
    }
}
