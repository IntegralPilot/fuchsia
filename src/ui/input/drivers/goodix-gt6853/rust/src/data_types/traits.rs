// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe register traits for hardware communication with Goodix GT6853.

use zerocopy::{FromBytes, Immutable, IntoBytes};

/// Base trait for registers with a fixed hardware address on the GT6853 controller.
pub trait AddressableRegister {
    /// Register address on the GT6853 controller.
    const ADDRESS: u16;
}

/// Trait implemented by registers that can be read from the GT6853 device over I2C.
pub trait ReadableRegister: AddressableRegister + FromBytes {}

/// Trait implemented by registers that can be written to the GT6853 device over I2C.
pub trait WritableRegister: AddressableRegister + IntoBytes + Immutable {}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocopy::KnownLayout;

    #[derive(Copy, Clone, Debug, PartialEq, Eq, FromBytes, IntoBytes, KnownLayout, Immutable)]
    #[repr(transparent)]
    struct PlaceholderRegister(pub u8);

    impl AddressableRegister for PlaceholderRegister {
        const ADDRESS: u16 = 0x1234;
    }

    impl ReadableRegister for PlaceholderRegister {}
    impl WritableRegister for PlaceholderRegister {}

    #[test]
    fn test_addressable_register() {
        assert_eq!(PlaceholderRegister::ADDRESS, 0x1234);
        let reg = PlaceholderRegister(0x42);
        assert_eq!(reg.as_bytes(), &[0x42]);
    }
}
