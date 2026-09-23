// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Address definitions for devices.

use core::fmt::{Debug, Display};
use core::hash::Hash;

use net_types::ip::{
    AddrSubnet, GenericOverIp, Ip, IpAddress, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr, Ipv6SourceAddr,
};
use net_types::{NonMappedAddr, NonMulticastAddr, SpecifiedAddr, UnicastAddr, Witness};

use crate::device::{AnyDevice, DeviceIdContext};
use crate::inspect::InspectableValue;
use crate::socket::SocketIpAddr;

/// An extension trait adding an associated type for an IP address assigned to a
/// device.
pub trait AssignedAddrIpExt: Ip {
    /// The witness type for assigned addresses.
    type AssignedWitness: Witness<Self::Addr>
        + Copy
        + Eq
        + PartialEq
        + Debug
        + Display
        + Hash
        + Send
        + Sync
        + Into<SpecifiedAddr<Self::Addr>>
        + Into<IpDeviceAddr<Self::Addr>>;
}

impl AssignedAddrIpExt for Ipv4 {
    type AssignedWitness = Ipv4DeviceAddr;
}

impl AssignedAddrIpExt for Ipv6 {
    type AssignedWitness = Ipv6DeviceAddr;
}

/// A weak IP address ID.
pub trait WeakIpAddressId<A: IpAddress>:
    Clone + Eq + Debug + Hash + Send + Sync + InspectableValue + 'static
{
    /// The strong version of this ID.
    type Strong: IpAddressId<A>;

    /// Attempts to upgrade this ID to the strong version.
    ///
    /// Upgrading fails if this is no longer a valid assigned IP address.
    fn upgrade(&self) -> Option<Self::Strong>;

    /// Returns whether this address is still assigned.
    fn is_assigned(&self) -> bool;
}

/// An IP address ID.
pub trait IpAddressId<A: IpAddress>: Clone + Eq + Debug + Hash {
    /// The weak version of this ID.
    type Weak: WeakIpAddressId<A>;

    /// Downgrades this ID to a weak reference.
    fn downgrade(&self) -> Self::Weak;

    /// Returns the address this ID represents.
    //
    // TODO(https://fxbug.dev/382104850): align this method with `addr_sub` by
    // returning `A::Version::AssignedWitness` directly, and add an
    // `Into: IpDeviceAddr<A>` bound on that associated type.
    fn addr(&self) -> IpDeviceAddr<A>;

    /// Returns the address subnet this ID represents.
    fn addr_sub(&self) -> AddrSubnet<A, <A::Version as AssignedAddrIpExt>::AssignedWitness>
    where
        A::Version: AssignedAddrIpExt;
}

/// Provides the execution context related to address IDs.
pub trait IpDeviceAddressIdContext<I: Ip>: DeviceIdContext<AnyDevice> {
    /// The strong address identifier.
    type AddressId: IpAddressId<I::Addr, Weak = Self::WeakAddressId>;
    /// The weak address identifier.
    type WeakAddressId: WeakIpAddressId<I::Addr, Strong = Self::AddressId>;
}

/// An IP address that witnesses properties needed to be assigned to a device.
#[derive(Copy, Clone, Debug, Eq, GenericOverIp, Hash, PartialEq)]
#[generic_over_ip(A, IpAddress)]
pub struct IpDeviceAddr<A: IpAddress>(NonMulticastAddr<NonMappedAddr<SpecifiedAddr<A>>>);

impl<A: IpAddress> Display for IpDeviceAddr<A> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(addr) = self;
        write!(f, "{}", addr)
    }
}

impl<A: IpAddress> IpDeviceAddr<A> {
    /// Returns the inner address, dropping all witnesses.
    pub fn addr(self) -> A {
        let Self(addr) = self;
        ***addr
    }

    /// Returns the inner address, including all witness types.
    pub fn into_inner(self) -> NonMulticastAddr<NonMappedAddr<SpecifiedAddr<A>>> {
        let IpDeviceAddr(addr) = self;
        addr
    }

    /// Constructs an [`IpDeviceAddr`] if the address is compliant, else `None`.
    pub fn new(addr: A) -> Option<IpDeviceAddr<A>> {
        Some(IpDeviceAddr(NonMulticastAddr::new(NonMappedAddr::new(SpecifiedAddr::new(addr)?)?)?))
    }

    /// Constructs an [`IpDeviceAddr`] from the inner witness.
    pub fn new_from_witness(addr: NonMulticastAddr<NonMappedAddr<SpecifiedAddr<A>>>) -> Self {
        Self(addr)
    }

    /// Attempts to constructs an [`IpDeviceAddr`] from a [`SocketIpAddr`].
    pub fn new_from_socket_ip_addr(addr: SocketIpAddr<A>) -> Option<Self> {
        NonMulticastAddr::new(addr.into_inner()).map(Self)
    }
}

impl IpDeviceAddr<Ipv6Addr> {
    /// Constructs an [`IpDeviceAddr`] from the given [`Ipv6DeviceAddr`].
    pub fn new_from_ipv6_device_addr(addr: Ipv6DeviceAddr) -> Self {
        let addr: UnicastAddr<NonMappedAddr<Ipv6Addr>> = addr.transpose();
        let addr: NonMappedAddr<SpecifiedAddr<Ipv6Addr>> = addr.into_specified().transpose();
        // SAFETY: The address is known to be unicast, because it was provided
        // as a `UnicastAddr`. A unicast address must be `NonMulticast`.
        let addr = unsafe { NonMulticastAddr::new_unchecked(addr) };
        Self(addr)
    }

    /// Constructs an [`IpDeviceAddr`] from the given [`Ipv6SourceAddr`].
    pub fn new_from_ipv6_source(addr: Ipv6SourceAddr) -> Option<Self> {
        match addr {
            Ipv6SourceAddr::Unicast(addr) => Some(Self::new_from_ipv6_device_addr(addr)),
            Ipv6SourceAddr::Unspecified => None,
        }
    }
}

impl<A: IpAddress> From<IpDeviceAddr<A>> for SpecifiedAddr<A> {
    fn from(addr: IpDeviceAddr<A>) -> Self {
        **addr.into_inner()
    }
}

impl<A: IpAddress> AsRef<SpecifiedAddr<A>> for IpDeviceAddr<A> {
    fn as_ref(&self) -> &SpecifiedAddr<A> {
        let Self(addr) = self;
        addr.as_ref()
    }
}

impl<A: IpAddress> From<IpDeviceAddr<A>> for SocketIpAddr<A> {
    fn from(addr: IpDeviceAddr<A>) -> Self {
        SocketIpAddr::new_from_witness(*addr.into_inner())
    }
}

#[derive(Debug)]
pub enum IpDeviceAddrError {
    NotNonMapped,
    NotNonMulticast,
}

impl<A: IpAddress> TryFrom<SpecifiedAddr<A>> for IpDeviceAddr<A> {
    type Error = IpDeviceAddrError;
    fn try_from(addr: SpecifiedAddr<A>) -> Result<Self, Self::Error> {
        Ok(IpDeviceAddr::new_from_witness(
            NonMulticastAddr::new(NonMappedAddr::new(addr).ok_or(IpDeviceAddrError::NotNonMapped)?)
                .ok_or(IpDeviceAddrError::NotNonMulticast)?,
        ))
    }
}

/// An Ipv4 address that witnesses properties needed to be assigned to a device.
///
/// These witnesses are the same that are held by [`IpDeviceAddr`], however that
/// type cannot be used here directly because we need [`Ipv4DeviceAddr`] to
/// implement `Witness<Ipv4Addr>`. That implementation is not present for our
/// new type [`IpDeviceAddr`] which wraps the true witnesses from the net-types
/// crate.
pub type Ipv4DeviceAddr = NonMulticastAddr<NonMappedAddr<SpecifiedAddr<Ipv4Addr>>>;

impl From<Ipv4DeviceAddr> for IpDeviceAddr<Ipv4Addr> {
    fn from(addr: Ipv4DeviceAddr) -> IpDeviceAddr<Ipv4Addr> {
        IpDeviceAddr::new_from_witness(addr)
    }
}

/// An IPv6 address that witnesses properties needed to be assigned to a device.
///
/// Like [`IpDeviceAddr`] but with stricter witnesses that are permitted for
/// IPv6 addresses.
pub type Ipv6DeviceAddr = NonMappedAddr<UnicastAddr<Ipv6Addr>>;

impl From<Ipv6DeviceAddr> for IpDeviceAddr<Ipv6Addr> {
    fn from(addr: Ipv6DeviceAddr) -> IpDeviceAddr<Ipv6Addr> {
        IpDeviceAddr::new_from_ipv6_device_addr(addr)
    }
}

#[cfg(any(test, feature = "testutils"))]
pub mod testutil {
    use net_types::ip::GenericOverIp;

    use super::*;
    use crate::StrongDeviceIdentifier;
    use crate::inspect::Inspector;
    use crate::testutil::FakeCoreCtx;

    /// A fake weak address ID.
    #[derive(Clone, Debug, Hash, Eq, PartialEq)]
    pub struct FakeWeakAddressId<T>(pub T);

    impl<A: IpAddress, T: IpAddressId<A> + Send + Sync + 'static> WeakIpAddressId<A>
        for FakeWeakAddressId<T>
    {
        type Strong = T;

        fn upgrade(&self) -> Option<Self::Strong> {
            let Self(inner) = self;
            Some(inner.clone())
        }

        fn is_assigned(&self) -> bool {
            true
        }
    }

    impl<T> InspectableValue for FakeWeakAddressId<T> {
        fn record<I: Inspector>(&self, _name: &str, _inspector: &mut I) {
            unimplemented!()
        }
    }

    impl<A: IpAddress> IpAddressId<A>
        for AddrSubnet<A, <A::Version as AssignedAddrIpExt>::AssignedWitness>
    where
        A::Version: AssignedAddrIpExt,
    {
        type Weak = FakeWeakAddressId<Self>;

        fn downgrade(&self) -> Self::Weak {
            FakeWeakAddressId(self.clone())
        }

        fn addr(&self) -> IpDeviceAddr<A> {
            #[derive(GenericOverIp)]
            #[generic_over_ip(I, Ip)]
            struct WrapIn<I: AssignedAddrIpExt>(I::AssignedWitness);
            A::Version::map_ip(
                WrapIn(self.addr()),
                |WrapIn(v4_addr)| IpDeviceAddr::new_from_witness(v4_addr),
                |WrapIn(v6_addr)| IpDeviceAddr::new_from_ipv6_device_addr(v6_addr),
            )
        }

        fn addr_sub(&self) -> AddrSubnet<A, <A::Version as AssignedAddrIpExt>::AssignedWitness> {
            self.clone()
        }
    }

    impl<I: AssignedAddrIpExt, S, Meta, DeviceId: StrongDeviceIdentifier>
        IpDeviceAddressIdContext<I> for FakeCoreCtx<S, Meta, DeviceId>
    {
        type AddressId = AddrSubnet<I::Addr, I::AssignedWitness>;
        type WeakAddressId = FakeWeakAddressId<Self::AddressId>;
    }
}
