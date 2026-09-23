// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// The length of a Bluetooth device address in bytes.
pub const ADDR_LEN: usize = 6;

/// Represents the type of a Bluetooth device address.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum AddressType {
    Public,
    RandomStatic,
    ResolvablePrivate,
    NonResolvablePrivate,
}

impl AddressType {
    /// Parses the random `AddressType` from the two most significant bits of `bytes`:
    /// - `0b00`: `AddressType::NonResolvablePrivate`
    /// - `0b01`: `AddressType::ResolvablePrivate`
    /// - `0b11`: `AddressType::RandomStatic`
    ///
    /// Returns `None` if the MSB bits are `0b10` (RFU).
    pub const fn from_random_bytes(bytes: [u8; ADDR_LEN]) -> Option<Self> {
        match bytes[ADDR_LEN - 1] >> 6 {
            0b00 => Some(Self::NonResolvablePrivate),
            0b01 => Some(Self::ResolvablePrivate),
            0b11 => Some(Self::RandomStatic),
            _ => None,
        }
    }
}

/// Represents a Bluetooth device address, encapsulating the 48-bit (6-byte) device address
/// and its address type.
///
/// The address type MUST be included in the address and used for address comparisons.
///
/// Core Spec v6.3 Vol 6 Part B Sec 1.3 says:
/// > Whenever two device addresses are compared, the comparison shall include
/// > the device address type (i.e. if the two addresses have different types, they are
/// > different even if the two 48-bit addresses are the same).
///
/// The bits that indicate the address type only differentiate between *random* device
/// addresses. Random and Public LE addresses can coincide.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeviceAddress {
    kind: AddressType,
    bytes: [u8; ADDR_LEN],
}

impl DeviceAddress {
    /// Creates a new `DeviceAddress` if `bytes` are valid for `kind`.
    ///
    /// For random address types, the two most significant bits of the address are validated:
    /// - `NonResolvablePrivate`: `0b00`
    /// - `ResolvablePrivate`: `0b01`
    /// - `RandomStatic`: `0b11`
    ///
    /// Returns `None` if the MSB bits do not match the address type or if the bits are `0b10` (RFU).
    pub const fn new(kind: AddressType, bytes: [u8; ADDR_LEN]) -> Option<Self> {
        match (kind, AddressType::from_random_bytes(bytes)) {
            (AddressType::Public, _)
            | (AddressType::RandomStatic, Some(AddressType::RandomStatic))
            | (AddressType::ResolvablePrivate, Some(AddressType::ResolvablePrivate))
            | (AddressType::NonResolvablePrivate, Some(AddressType::NonResolvablePrivate)) => {
                Some(Self { kind, bytes })
            }
            _ => None,
        }
    }

    /// Creates a new random `DeviceAddress` by parsing the `AddressType` from `bytes`.
    ///
    /// The two most significant bits of `bytes` determine the address type:
    /// - `0b00`: `AddressType::NonResolvablePrivate`
    /// - `0b01`: `AddressType::ResolvablePrivate`
    /// - `0b11`: `AddressType::RandomStatic`
    ///
    /// Returns `None` if the MSB bits are `0b10` (RFU).
    pub const fn from_random(bytes: [u8; ADDR_LEN]) -> Option<Self> {
        match AddressType::from_random_bytes(bytes) {
            Some(kind) => Some(Self { kind, bytes }),
            None => None,
        }
    }

    /// Returns the address type.
    pub const fn kind(&self) -> AddressType {
        self.kind
    }

    /// Returns the address bytes.
    pub const fn bytes(&self) -> &[u8; ADDR_LEN] {
        &self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_address() {
        let addr = DeviceAddress::new(AddressType::Public, [1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(addr.kind(), AddressType::Public);
        assert_eq!(addr.bytes(), &[1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn test_address_validation() {
        let bytes_00 = [1, 2, 3, 4, 5, 0b0000_0000];
        let bytes_01 = [1, 2, 3, 4, 5, 0b0100_0000];
        let bytes_10 = [1, 2, 3, 4, 5, 0b1000_0000]; // RFU
        let bytes_11 = [1, 2, 3, 4, 5, 0b1100_0000];

        // Public accepts any MSB bits
        assert!(DeviceAddress::new(AddressType::Public, bytes_00).is_some());
        assert!(DeviceAddress::new(AddressType::Public, bytes_01).is_some());
        assert!(DeviceAddress::new(AddressType::Public, bytes_10).is_some());
        assert!(DeviceAddress::new(AddressType::Public, bytes_11).is_some());

        // NonResolvablePrivate requires 00
        assert!(DeviceAddress::new(AddressType::NonResolvablePrivate, bytes_00).is_some());
        assert!(DeviceAddress::new(AddressType::NonResolvablePrivate, bytes_01).is_none());
        assert!(DeviceAddress::new(AddressType::NonResolvablePrivate, bytes_10).is_none());
        assert!(DeviceAddress::new(AddressType::NonResolvablePrivate, bytes_11).is_none());

        // ResolvablePrivate requires 01
        assert!(DeviceAddress::new(AddressType::ResolvablePrivate, bytes_00).is_none());
        assert!(DeviceAddress::new(AddressType::ResolvablePrivate, bytes_01).is_some());
        assert!(DeviceAddress::new(AddressType::ResolvablePrivate, bytes_10).is_none());
        assert!(DeviceAddress::new(AddressType::ResolvablePrivate, bytes_11).is_none());

        // RandomStatic requires 11
        assert!(DeviceAddress::new(AddressType::RandomStatic, bytes_00).is_none());
        assert!(DeviceAddress::new(AddressType::RandomStatic, bytes_01).is_none());
        assert!(DeviceAddress::new(AddressType::RandomStatic, bytes_10).is_none());
        assert!(DeviceAddress::new(AddressType::RandomStatic, bytes_11).is_some());
    }

    #[test]
    fn test_from_random() {
        let bytes_00 = [1, 2, 3, 4, 5, 0b0000_0000];
        let bytes_01 = [1, 2, 3, 4, 5, 0b0100_0000];
        let bytes_10 = [1, 2, 3, 4, 5, 0b1000_0000]; // RFU
        let bytes_11 = [1, 2, 3, 4, 5, 0b1100_0000];

        let addr_00 = DeviceAddress::from_random(bytes_00).unwrap();
        assert_eq!(addr_00.kind(), AddressType::NonResolvablePrivate);
        assert_eq!(addr_00.bytes(), &bytes_00);

        let addr_01 = DeviceAddress::from_random(bytes_01).unwrap();
        assert_eq!(addr_01.kind(), AddressType::ResolvablePrivate);
        assert_eq!(addr_01.bytes(), &bytes_01);

        assert!(DeviceAddress::from_random(bytes_10).is_none());

        let addr_11 = DeviceAddress::from_random(bytes_11).unwrap();
        assert_eq!(addr_11.kind(), AddressType::RandomStatic);
        assert_eq!(addr_11.bytes(), &bytes_11);
    }

    #[test]
    fn test_address_type_from_random_bytes() {
        let bytes_00 = [1, 2, 3, 4, 5, 0b0000_0000];
        let bytes_01 = [1, 2, 3, 4, 5, 0b0100_0000];
        let bytes_10 = [1, 2, 3, 4, 5, 0b1000_0000]; // RFU
        let bytes_11 = [1, 2, 3, 4, 5, 0b1100_0000];

        assert_eq!(
            AddressType::from_random_bytes(bytes_00),
            Some(AddressType::NonResolvablePrivate)
        );
        assert_eq!(AddressType::from_random_bytes(bytes_01), Some(AddressType::ResolvablePrivate));
        assert_eq!(AddressType::from_random_bytes(bytes_10), None);
        assert_eq!(AddressType::from_random_bytes(bytes_11), Some(AddressType::RandomStatic));
    }

    #[test]
    fn test_address_equality_includes_type() {
        let addr1 = DeviceAddress::new(AddressType::Public, [1, 2, 3, 4, 5, 0b1100_0000]).unwrap();
        let addr2 =
            DeviceAddress::new(AddressType::RandomStatic, [1, 2, 3, 4, 5, 0b1100_0000]).unwrap();
        let addr3 = DeviceAddress::new(AddressType::Public, [1, 2, 3, 4, 5, 0b1100_0000]).unwrap();
        assert_ne!(addr1, addr2);
        assert_eq!(addr1, addr3);
    }
}
