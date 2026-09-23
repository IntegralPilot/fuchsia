// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common traits abstracting the device layer.
//!
//! Devices are abstracted away throughout the netstack3 crates. This module
//! provides the base abstraction definitions.
//!
//! Abstracting devices provides:
//!
//! * A useful way to remove a lot of state and complexity from tests.
//! * Opaqueness to steer state access towards context traits.
//! * Type signature reduction since real device identifiers are parameterized
//!   by bindings types.
//! * Modularity.

use alloc::borrow::Cow;
use core::borrow::Borrow;
use core::fmt::Debug;
use core::hash::Hash;

pub(crate) mod address;
pub(crate) mod link;

/// An identifier for a device.
pub trait DeviceIdentifier: Clone + Debug + Eq + Hash + PartialEq + Send + Sync + 'static {
    /// Returns true if the device is a loopback device.
    fn is_loopback(&self) -> bool;
}

/// A strong device reference.
///
/// [`StrongDeviceIdentifier`] indicates that the referenced device is alive
/// while the instance exists.
pub trait StrongDeviceIdentifier: DeviceIdentifier + PartialEq<Self::Weak> {
    /// The weak version of this identifier.
    type Weak: WeakDeviceIdentifier<Strong = Self>;

    /// Returns a weak ID for this strong ID.
    fn downgrade(&self) -> Self::Weak;
}

/// A weak device reference.
///
/// This is the weak reference equivalent of [`StrongDeviceIdentifier`].
pub trait WeakDeviceIdentifier: DeviceIdentifier + PartialEq<Self::Strong> {
    /// The strong version of this identifier.
    type Strong: StrongDeviceIdentifier<Weak = Self>;

    /// Attempts to upgrade this weak ID to a strong ID.
    ///
    /// Returns `None` if the resource has been destroyed.
    fn upgrade(&self) -> Option<Self::Strong>;
}

/// A device.
///
/// `Device` is used to identify a particular device implementation. It
/// is only intended to exist at the type level, never instantiated at runtime.
pub trait Device: 'static {}

/// Marker type for a generic device.
///
/// This generally represents a device at the IP layer. Other implementations
/// may exist for type safe link devices.
pub enum AnyDevice {}

impl Device for AnyDevice {}

/// An execution context which provides device ID types type for various
/// netstack internals to share.
pub trait DeviceIdContext<D: Device> {
    /// The type of device IDs.
    type DeviceId: StrongDeviceIdentifier<Weak = Self::WeakDeviceId> + 'static;

    /// The type of weakly referenced device IDs.
    type WeakDeviceId: WeakDeviceIdentifier<Strong = Self::DeviceId> + 'static;
}

/// A marker trait tying [`DeviceIdContext`] implementations.
///
/// To call into the IP layer, we need to be able to represent device
/// identifiers in the [`AnyDevice`] domain. This trait is a statement that a
/// [`DeviceIdContext`] in some domain `D` has its identifiers convertible into
/// the [`AnyDevice`] domain with `From` bounds.
///
/// It is provided as a blanket implementation for [`DeviceIdContext`]s that
/// fulfill the conversion.
pub trait DeviceIdAnyCompatContext<D: Device>:
    DeviceIdContext<D>
    + DeviceIdContext<
        AnyDevice,
        DeviceId: From<<Self as DeviceIdContext<D>>::DeviceId>,
        WeakDeviceId: From<<Self as DeviceIdContext<D>>::WeakDeviceId>,
    >
{
}

impl<CC, D> DeviceIdAnyCompatContext<D> for CC
where
    D: Device,
    CC: DeviceIdContext<D>
        + DeviceIdContext<
            AnyDevice,
            DeviceId: From<<CC as DeviceIdContext<D>>::DeviceId>,
            WeakDeviceId: From<<CC as DeviceIdContext<D>>::WeakDeviceId>,
        >,
{
}

/// A device id that might be either in its strong or weak form.
#[derive(Copy, Clone)]
#[allow(missing_docs)]
pub enum EitherDeviceId<S, W> {
    Strong(S),
    Weak(W),
}

impl<S: PartialEq, W: PartialEq + PartialEq<S>> PartialEq for EitherDeviceId<S, W> {
    fn eq(&self, other: &EitherDeviceId<S, W>) -> bool {
        match (self, other) {
            (EitherDeviceId::Strong(this), EitherDeviceId::Strong(other)) => this == other,
            (EitherDeviceId::Strong(this), EitherDeviceId::Weak(other)) => other == this,
            (EitherDeviceId::Weak(this), EitherDeviceId::Strong(other)) => this == other,
            (EitherDeviceId::Weak(this), EitherDeviceId::Weak(other)) => this == other,
        }
    }
}

impl<S: StrongDeviceIdentifier, W: WeakDeviceIdentifier<Strong = S>> EitherDeviceId<&'_ S, &'_ W> {
    /// Returns a [`Cow`] reference for the strong variant.
    ///
    /// Attempts to upgrade if this is a `Weak` variant.
    pub fn as_strong_ref<'a>(&'a self) -> Option<Cow<'a, S>> {
        match self {
            EitherDeviceId::Strong(s) => Some(Cow::Borrowed(s)),
            EitherDeviceId::Weak(w) => w.upgrade().map(Cow::Owned),
        }
    }
}

impl<S, W> EitherDeviceId<S, W> {
    /// Returns a borrowed version of this `EitherDeviceId`.
    pub fn as_ref<'a, S2, W2>(&'a self) -> EitherDeviceId<&'a S2, &'a W2>
    where
        S: Borrow<S2>,
        W: Borrow<W2>,
    {
        match self {
            EitherDeviceId::Strong(s) => EitherDeviceId::Strong(s.borrow()),
            EitherDeviceId::Weak(w) => EitherDeviceId::Weak(w.borrow()),
        }
    }
}

impl<S: StrongDeviceIdentifier<Weak = W>, W: WeakDeviceIdentifier<Strong = S>>
    EitherDeviceId<S, W>
{
    /// Returns a [`Cow`] reference for the `Strong` variant.
    ///
    /// Attempts to upgrade if this is a `Weak` variant.
    pub fn as_strong<'a>(&'a self) -> Option<Cow<'a, S>> {
        match self {
            EitherDeviceId::Strong(s) => Some(Cow::Borrowed(s)),
            EitherDeviceId::Weak(w) => w.upgrade().map(Cow::Owned),
        }
    }

    /// Returns a [`Cow`] reference for the `Weak` variant.
    ///
    /// Downgrades if this is a `Strong` variant.
    pub fn as_weak<'a>(&'a self) -> Cow<'a, W> {
        match self {
            EitherDeviceId::Strong(s) => Cow::Owned(s.downgrade()),
            EitherDeviceId::Weak(w) => Cow::Borrowed(w),
        }
    }
}

#[cfg(any(test, feature = "testutils"))]
pub(crate) mod testutil {
    use alloc::sync::Arc;
    use core::sync::atomic::AtomicBool;

    use super::*;

    use crate::InterfaceProperties;
    use crate::testutil::FakeCoreCtx;

    #[derive(Clone, Copy, Debug, PartialOrd, Ord, PartialEq, Eq, Hash)]
    #[allow(missing_docs)]
    pub enum FakeDeviceClass {
        Ethernet,
        Wlan,
    }

    /// A fake weak device id.
    #[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
    pub struct FakeWeakDeviceId<D>(pub D);

    impl<D: PartialEq> PartialEq<D> for FakeWeakDeviceId<D> {
        fn eq(&self, other: &D) -> bool {
            let Self(this) = self;
            this == other
        }
    }

    impl<D: FakeStrongDeviceId> WeakDeviceIdentifier for FakeWeakDeviceId<D> {
        type Strong = D;

        fn upgrade(&self) -> Option<D> {
            let Self(inner) = self;
            inner.is_alive().then(|| inner.clone())
        }
    }

    impl<D: DeviceIdentifier> DeviceIdentifier for FakeWeakDeviceId<D> {
        fn is_loopback(&self) -> bool {
            let Self(inner) = self;
            inner.is_loopback()
        }
    }

    /// A fake device ID for use in testing.
    #[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
    pub struct FakeDeviceId;

    impl FakeDeviceId {
        /// The name of the fake device.
        pub const FAKE_NAME: &'static str = "FakeDeviceId";
    }

    impl StrongDeviceIdentifier for FakeDeviceId {
        type Weak = FakeWeakDeviceId<Self>;

        fn downgrade(&self) -> Self::Weak {
            FakeWeakDeviceId(self.clone())
        }
    }

    impl DeviceIdentifier for FakeDeviceId {
        fn is_loopback(&self) -> bool {
            false
        }
    }

    impl FakeStrongDeviceId for FakeDeviceId {
        fn is_alive(&self) -> bool {
            true
        }
    }

    impl PartialEq<FakeWeakDeviceId<FakeDeviceId>> for FakeDeviceId {
        fn eq(&self, FakeWeakDeviceId(other): &FakeWeakDeviceId<FakeDeviceId>) -> bool {
            self == other
        }
    }

    impl InterfaceProperties<()> for FakeDeviceId {
        fn id_matches(&self, _: &core::num::NonZeroU64) -> bool {
            unimplemented!()
        }

        fn name_matches(&self, name: &str) -> bool {
            name == Self::FAKE_NAME
        }

        fn device_class_matches(&self, _: &()) -> bool {
            unimplemented!()
        }
    }

    /// A fake device ID for use in testing.
    ///
    /// [`FakeReferencyDeviceId`] behaves like a referency device ID, each
    /// constructed instance represents a new device.
    #[derive(Clone, Debug, Default)]
    pub struct FakeReferencyDeviceId {
        removed: Arc<AtomicBool>,
    }

    impl core::hash::Hash for FakeReferencyDeviceId {
        fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
            let Self { removed } = self;
            core::ptr::hash(alloc::sync::Arc::as_ptr(removed), state)
        }
    }

    impl core::cmp::Eq for FakeReferencyDeviceId {}

    impl core::cmp::PartialEq for FakeReferencyDeviceId {
        fn eq(&self, Self { removed: other }: &Self) -> bool {
            let Self { removed } = self;
            alloc::sync::Arc::ptr_eq(removed, other)
        }
    }

    impl core::cmp::Ord for FakeReferencyDeviceId {
        fn cmp(&self, Self { removed: other }: &Self) -> core::cmp::Ordering {
            let Self { removed } = self;
            alloc::sync::Arc::as_ptr(removed).cmp(&alloc::sync::Arc::as_ptr(other))
        }
    }

    impl core::cmp::PartialOrd for FakeReferencyDeviceId {
        fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    impl FakeReferencyDeviceId {
        /// Marks this device as removed, all weak references will not be able
        /// to upgrade anymore.
        pub fn mark_removed(&self) {
            self.removed.store(true, core::sync::atomic::Ordering::Relaxed);
        }

        const FAKE_NAME: &'static str = "FakeReferencyDeviceId";
    }

    impl StrongDeviceIdentifier for FakeReferencyDeviceId {
        type Weak = FakeWeakDeviceId<Self>;

        fn downgrade(&self) -> Self::Weak {
            FakeWeakDeviceId(self.clone())
        }
    }

    impl DeviceIdentifier for FakeReferencyDeviceId {
        fn is_loopback(&self) -> bool {
            false
        }
    }

    impl FakeStrongDeviceId for FakeReferencyDeviceId {
        fn is_alive(&self) -> bool {
            !self.removed.load(core::sync::atomic::Ordering::Relaxed)
        }
    }

    impl PartialEq<FakeWeakDeviceId<FakeReferencyDeviceId>> for FakeReferencyDeviceId {
        fn eq(&self, FakeWeakDeviceId(other): &FakeWeakDeviceId<FakeReferencyDeviceId>) -> bool {
            self == other
        }
    }

    impl InterfaceProperties<()> for FakeReferencyDeviceId {
        fn id_matches(&self, _: &core::num::NonZeroU64) -> bool {
            unimplemented!()
        }

        fn name_matches(&self, name: &str) -> bool {
            name == Self::FAKE_NAME
        }

        fn device_class_matches(&self, _: &()) -> bool {
            unimplemented!()
        }
    }

    /// Marks a fake strong device id.
    pub trait FakeStrongDeviceId:
        StrongDeviceIdentifier<Weak = FakeWeakDeviceId<Self>> + 'static + Ord
    {
        /// Returns whether this ID is still alive.
        ///
        /// This is used by [`FakeWeakDeviceId`] to return `None` when trying to
        /// upgrade back a `FakeStrongDeviceId`.
        fn is_alive(&self) -> bool;
    }

    /// A device ID type that supports identifying more than one distinct
    /// device.
    #[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
    #[allow(missing_docs)]
    pub enum MultipleDevicesId {
        A,
        B,
        C,
    }

    impl MultipleDevicesId {
        /// Returns all variants.
        pub fn all() -> [Self; 3] {
            [Self::A, Self::B, Self::C]
        }

        fn fake_name(&self) -> &'static str {
            match self {
                MultipleDevicesId::A => "A",
                MultipleDevicesId::B => "B",
                MultipleDevicesId::C => "C",
            }
        }
    }

    impl DeviceIdentifier for MultipleDevicesId {
        fn is_loopback(&self) -> bool {
            false
        }
    }

    impl StrongDeviceIdentifier for MultipleDevicesId {
        type Weak = FakeWeakDeviceId<Self>;

        fn downgrade(&self) -> Self::Weak {
            FakeWeakDeviceId(self.clone())
        }
    }

    impl FakeStrongDeviceId for MultipleDevicesId {
        fn is_alive(&self) -> bool {
            true
        }
    }

    impl InterfaceProperties<()> for MultipleDevicesId {
        fn id_matches(&self, _: &core::num::NonZeroU64) -> bool {
            unimplemented!()
        }

        fn name_matches(&self, name: &str) -> bool {
            self.fake_name() == name
        }

        fn device_class_matches(&self, _: &()) -> bool {
            unimplemented!()
        }
    }

    /// Holds per-device state for a [`MultipleDevicesId`].
    #[derive(Default)]
    pub struct MultipleDevicesIdState<T> {
        a: T,
        b: T,
        c: T,
    }

    impl<T> MultipleDevicesIdState<T> {
        /// Accesses the per-device state for the given device ID.
        pub fn state(&self, device: &MultipleDevicesId) -> &T {
            match device {
                MultipleDevicesId::A => &self.a,
                MultipleDevicesId::B => &self.b,
                MultipleDevicesId::C => &self.c,
            }
        }
    }

    impl<S, Meta, D: StrongDeviceIdentifier> DeviceIdContext<AnyDevice> for FakeCoreCtx<S, Meta, D> {
        type DeviceId = D;
        type WeakDeviceId = D::Weak;
    }

    impl<S, Meta, D: StrongDeviceIdentifier> DeviceIdContext<AnyDevice>
        for &mut FakeCoreCtx<S, Meta, D>
    {
        type DeviceId = D;
        type WeakDeviceId = D::Weak;
    }

    impl PartialEq<FakeWeakDeviceId<MultipleDevicesId>> for MultipleDevicesId {
        fn eq(&self, FakeWeakDeviceId(other): &FakeWeakDeviceId<MultipleDevicesId>) -> bool {
            self == other
        }
    }
}
