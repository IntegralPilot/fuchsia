// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Link device (L2) definitions.
//!
//! This module contains definitions of link-layer devices, otherwise known as
//! L2 devices.

use core::fmt::Debug;

use net_types::ethernet::Mac;
use net_types::{MulticastAddress, UnicastAddress};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

use crate::Device;

/// The type of address used by a link device.
pub trait LinkAddress:
    'static
    + FromBytes
    + IntoBytes
    + KnownLayout
    + Immutable
    + Unaligned
    + Copy
    + Clone
    + Debug
    + Eq
    + Send
{
    /// The length of the address in bytes.
    const BYTES_LENGTH: usize;

    /// Returns the underlying bytes of a `LinkAddress`.
    fn bytes(&self) -> &[u8];

    /// Constructs a `LinkLayerAddress` from the provided bytes.
    ///
    /// # Panics
    ///
    /// `from_bytes` may panic if `bytes` is not **exactly** [`BYTES_LENGTH`]
    /// long.
    fn from_bytes(bytes: &[u8]) -> Self;
}

impl LinkAddress for Mac {
    const BYTES_LENGTH: usize = 6;

    fn bytes(&self) -> &[u8] {
        self.as_ref()
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        // Assert that contract is being held.
        debug_assert_eq!(bytes.len(), Self::BYTES_LENGTH);
        let mut b = [0; Self::BYTES_LENGTH];
        b.copy_from_slice(bytes);
        Self::new(b)
    }
}

/// A link address that can be unicast and multicast.
pub trait LinkDeviceAddress: LinkAddress + UnicastAddress + MulticastAddress {}
impl<L: LinkAddress + UnicastAddress + MulticastAddress> LinkDeviceAddress for L {}

/// A link device.
///
/// `LinkDevice` is used to identify a particular link device implementation. It
/// is only intended to exist at the type level, never instantiated at runtime.
pub trait LinkDevice: Device + Debug {
    /// The type of address used to address link devices of this type.
    type Address: LinkDeviceAddress;
}

/// Utilities for testing link devices.
#[cfg(any(test, feature = "testutils"))]
pub(crate) mod testutil {
    use net_types::BroadcastAddress;
    use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

    use super::*;
    use crate::testutil::{FakeCoreCtx, FakeStrongDeviceId, FakeWeakDeviceId};
    use crate::{DeviceIdContext, DeviceIdentifier, StrongDeviceIdentifier};

    /// A fake [`LinkDevice`].
    #[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
    pub enum FakeLinkDevice {}

    const FAKE_LINK_ADDRESS_LEN: usize = 1;

    /// A fake [`LinkAddress`].
    ///
    /// The value 0xFF is the broadcast address. The LSB is the individual/group bit.
    #[derive(
        KnownLayout,
        FromBytes,
        IntoBytes,
        Immutable,
        Unaligned,
        Copy,
        Clone,
        Debug,
        Hash,
        PartialEq,
        Eq,
    )]
    #[repr(transparent)]
    pub struct FakeLinkAddress(pub [u8; FAKE_LINK_ADDRESS_LEN]);

    impl FakeLinkAddress {
        const BROADCAST: Self = Self([0xff; FAKE_LINK_ADDRESS_LEN]);
    }

    impl UnicastAddress for FakeLinkAddress {
        #[inline]
        fn is_unicast(&self) -> bool {
            self.0[0] & 1 == 0
        }
    }

    impl MulticastAddress for FakeLinkAddress {
        #[inline]
        fn is_multicast(&self) -> bool {
            (self.0[0] & 1 == 1) && !self.is_broadcast()
        }
    }

    impl BroadcastAddress for FakeLinkAddress {
        #[inline]
        fn is_broadcast(&self) -> bool {
            *self == Self::BROADCAST
        }
    }

    impl LinkAddress for FakeLinkAddress {
        const BYTES_LENGTH: usize = FAKE_LINK_ADDRESS_LEN;

        fn bytes(&self) -> &[u8] {
            &self.0[..]
        }

        fn from_bytes(bytes: &[u8]) -> FakeLinkAddress {
            FakeLinkAddress(bytes.try_into().unwrap())
        }
    }

    impl Device for FakeLinkDevice {}

    impl LinkDevice for FakeLinkDevice {
        type Address = FakeLinkAddress;
    }

    /// A fake ID identifying a [`FakeLinkDevice`].
    #[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
    pub struct FakeLinkDeviceId;

    impl StrongDeviceIdentifier for FakeLinkDeviceId {
        type Weak = FakeWeakDeviceId<Self>;

        fn downgrade(&self) -> Self::Weak {
            FakeWeakDeviceId(*self)
        }
    }

    impl DeviceIdentifier for FakeLinkDeviceId {
        fn is_loopback(&self) -> bool {
            false
        }
    }

    impl<S, M> DeviceIdContext<FakeLinkDevice> for FakeCoreCtx<S, M, FakeLinkDeviceId> {
        type DeviceId = FakeLinkDeviceId;
        type WeakDeviceId = FakeWeakDeviceId<FakeLinkDeviceId>;
    }

    impl FakeStrongDeviceId for FakeLinkDeviceId {
        fn is_alive(&self) -> bool {
            true
        }
    }

    impl PartialEq<FakeWeakDeviceId<FakeLinkDeviceId>> for FakeLinkDeviceId {
        fn eq(&self, FakeWeakDeviceId(other): &FakeWeakDeviceId<FakeLinkDeviceId>) -> bool {
            self == other
        }
    }
}
