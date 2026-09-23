// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common device identifier types.

use alloc::sync::Arc;
use core::fmt::{self, Debug};
use core::hash::Hash;
use core::num::NonZeroU64;

use derivative::Derivative;
use netstack3_base::sync::{DynDebugReferences, PrimaryRc, StrongRc};
use netstack3_base::{
    Device, DeviceIdentifier, InterfaceProperties, StrongDeviceIdentifier, WeakDeviceIdentifier,
};

use crate::blackhole::{BlackholeDevice, BlackholeDeviceId, BlackholeWeakDeviceId};
use crate::internal::base::{
    DeviceClassMatcher as _, DeviceIdAndNameMatcher as _, DeviceLayerTypes, OriginTracker,
};
use crate::internal::ethernet::EthernetLinkDevice;
use crate::internal::loopback::{LoopbackDevice, LoopbackDeviceId, LoopbackWeakDeviceId};
use crate::internal::pure_ip::{PureIpDevice, PureIpDeviceId, PureIpWeakDeviceId};
use crate::internal::state::{BaseDeviceState, DeviceStateSpec, IpLinkDeviceState, WeakCookie};

/// A weak ID identifying a device.
///
/// This device ID makes no claim about the live-ness of the underlying device.
/// See [`DeviceId`] for a device ID that acts as a witness to the live-ness of
/// a device.
#[derive(Derivative)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Hash(bound = ""))]
#[allow(missing_docs)]
pub enum WeakDeviceId<BT: DeviceLayerTypes> {
    Ethernet(EthernetWeakDeviceId<BT>),
    Loopback(LoopbackWeakDeviceId<BT>),
    PureIp(PureIpWeakDeviceId<BT>),
    Blackhole(BlackholeWeakDeviceId<BT>),
}

impl<BT: DeviceLayerTypes> PartialEq<DeviceId<BT>> for WeakDeviceId<BT> {
    fn eq(&self, other: &DeviceId<BT>) -> bool {
        <DeviceId<BT> as PartialEq<WeakDeviceId<BT>>>::eq(other, self)
    }
}

impl<BT: DeviceLayerTypes> From<EthernetWeakDeviceId<BT>> for WeakDeviceId<BT> {
    fn from(id: EthernetWeakDeviceId<BT>) -> WeakDeviceId<BT> {
        WeakDeviceId::Ethernet(id)
    }
}

impl<BT: DeviceLayerTypes> From<LoopbackWeakDeviceId<BT>> for WeakDeviceId<BT> {
    fn from(id: LoopbackWeakDeviceId<BT>) -> WeakDeviceId<BT> {
        WeakDeviceId::Loopback(id)
    }
}

impl<BT: DeviceLayerTypes> From<PureIpWeakDeviceId<BT>> for WeakDeviceId<BT> {
    fn from(id: PureIpWeakDeviceId<BT>) -> WeakDeviceId<BT> {
        WeakDeviceId::PureIp(id)
    }
}

impl<BT: DeviceLayerTypes> From<BlackholeWeakDeviceId<BT>> for WeakDeviceId<BT> {
    fn from(id: BlackholeWeakDeviceId<BT>) -> WeakDeviceId<BT> {
        WeakDeviceId::Blackhole(id)
    }
}

impl<BT: DeviceLayerTypes> WeakDeviceId<BT> {
    /// Attempts to upgrade the ID.
    pub fn upgrade(&self) -> Option<DeviceId<BT>> {
        for_any_device_id!(WeakDeviceId, self, id => id.upgrade().map(Into::into))
    }

    /// Creates a [`DebugReferences`] instance for this device.
    pub fn debug_references(&self) -> DynDebugReferences {
        for_any_device_id!(
            WeakDeviceId,
            self,
            BaseWeakDeviceId { cookie } => cookie.weak_ref.debug_references().into_dyn()
        )
    }

    /// Returns the bindings identifier associated with the device.
    pub fn bindings_id(&self) -> &BT::DeviceIdentifier {
        for_any_device_id!(WeakDeviceId, self, id => id.bindings_id())
    }
}

impl<BT: DeviceLayerTypes> DeviceIdentifier for WeakDeviceId<BT> {
    fn is_loopback(&self) -> bool {
        match self {
            WeakDeviceId::Loopback(_) => true,
            WeakDeviceId::Ethernet(_) | WeakDeviceId::PureIp(_) | WeakDeviceId::Blackhole(_) => {
                false
            }
        }
    }
}

impl<BT: DeviceLayerTypes> WeakDeviceIdentifier for WeakDeviceId<BT> {
    type Strong = DeviceId<BT>;

    fn upgrade(&self) -> Option<Self::Strong> {
        self.upgrade()
    }
}

impl<BT: DeviceLayerTypes> Debug for WeakDeviceId<BT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for_any_device_id!(WeakDeviceId, self, id => Debug::fmt(id, f))
    }
}

/// A strong ID identifying a device.
///
/// Holders may safely assume that the underlying device is "alive" in the sense
/// that the device is still recognized by the stack. That is, operations that
/// use this device ID will never fail as a result of "unrecognized device"-like
/// errors.
#[derive(Derivative)]
#[derivative(Eq(bound = ""), PartialEq(bound = ""), Hash(bound = ""))]
#[allow(missing_docs)]
pub enum DeviceId<BT: DeviceLayerTypes> {
    Ethernet(EthernetDeviceId<BT>),
    Loopback(LoopbackDeviceId<BT>),
    PureIp(PureIpDeviceId<BT>),
    Blackhole(BlackholeDeviceId<BT>),
}

/// Evaluates the expression for the given device_id, regardless of its variant.
///
/// This macro supports multiple forms.
///
/// Form #1: for_any_device_id!(device_id_enum, device_id,
///          variable => expression)
///   - `device_id_enum`: the type of device ID, either [`DeviceId`] or
///     [`WeakDeviceId`].
///   - `device_id`: The id to match on. I.e. a value of type `device_id_enum`.
///   - `variable`: The local variable to bind the inner device id type to. This
///     variable may be referenced from `expression`.
///   - `expression`: The expression to evaluate against the given `device_id`.
///
/// Example: for_any_device_id!(DeviceId, my_device, id => println!({:?}, id))
///
/// Form #2: for_any_device_id!(device_id_enum, provider_trait, type_param,
///          device_id, variable => expression)
///   - `device_id_enum`, `device_id`, `variable`, and `expression`: Same as for
///     Form #1.
///   - `provider_trait`: The [`DeviceProvider`] trait.
///   - `type_param`: The name of the type parameter to hold the values provided
///     by `provider_trait`. This type parameter may be referenced from
///     `expression`.
///
/// The second form is useful in situations where the compiler cannot infer a
/// device type that differs for each variant of `device_id_enum`.
///
/// Example: for_any_device_id!(
///     DeviceId,
///     DeviceProvider,
///     D,
///     my_device, id => fn_with_id::<D>(id)
/// )
#[macro_export]
macro_rules! for_any_device_id {
    // Form #1
    ($device_id_enum_type:ident, $device_id:expr, $variable:pat => $expression:expr) => {
        match $device_id {
            $device_id_enum_type::Loopback($variable) => $expression,
            $device_id_enum_type::Ethernet($variable) => $expression,
            $device_id_enum_type::PureIp($variable) => $expression,
            $device_id_enum_type::Blackhole($variable) => $expression,
        }
    };
    // Form #2
    (
        $device_id_enum_type:ident,
        $provider_trait:ident,
        $type_param:ident,
        $device_id:expr, $variable:pat => $expression:expr) => {
        match $device_id {
            $device_id_enum_type::Loopback($variable) => {
                type $type_param = <() as $provider_trait>::Loopback;
                $expression
            }
            $device_id_enum_type::Ethernet($variable) => {
                type $type_param = <() as $provider_trait>::Ethernet;
                $expression
            }
            $device_id_enum_type::PureIp($variable) => {
                type $type_param = <() as $provider_trait>::PureIp;
                $expression
            }
            $device_id_enum_type::Blackhole($variable) => {
                type $type_param = <() as $provider_trait>::Blackhole;
                $expression
            }
        }
    };
}
pub(crate) use crate::for_any_device_id;

/// Provides the [`Device`] type for each device domain.
pub trait DeviceProvider {
    /// The [`Device`] type for Ethernet devices.
    type Ethernet: Device;
    /// The [`Device`] type for Loopback devices.
    type Loopback: Device;
    /// The [`Device`] type for pure IP devices.
    type PureIp: Device;
    /// The [`Device`] type for Blackhole devices.
    type Blackhole: Device;
}

/// This implementation is used in the `for_any_device_id` macro.
impl DeviceProvider for () {
    type Ethernet = EthernetLinkDevice;
    type Loopback = LoopbackDevice;
    type PureIp = PureIpDevice;
    type Blackhole = BlackholeDevice;
}

impl<BT: DeviceLayerTypes> Clone for DeviceId<BT> {
    #[cfg_attr(feature = "instrumented", track_caller)]
    fn clone(&self) -> Self {
        for_any_device_id!(DeviceId, self, id => id.clone().into())
    }
}

impl<BT: DeviceLayerTypes> PartialEq<WeakDeviceId<BT>> for DeviceId<BT> {
    fn eq(&self, other: &WeakDeviceId<BT>) -> bool {
        match (self, other) {
            (DeviceId::Ethernet(strong), WeakDeviceId::Ethernet(weak)) => strong == weak,
            (DeviceId::Loopback(strong), WeakDeviceId::Loopback(weak)) => strong == weak,
            (DeviceId::PureIp(strong), WeakDeviceId::PureIp(weak)) => strong == weak,
            (DeviceId::Blackhole(strong), WeakDeviceId::Blackhole(weak)) => strong == weak,
            (DeviceId::Ethernet(_), _)
            | (DeviceId::Loopback(_), _)
            | (DeviceId::PureIp(_), _)
            | (DeviceId::Blackhole(_), _) => false,
        }
    }
}

impl<BT: DeviceLayerTypes> PartialOrd for DeviceId<BT> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<BT: DeviceLayerTypes> Ord for DeviceId<BT> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        // Assigns an arbitrary but orderable identifier, `u8`, to each variant
        // of `DeviceId`.
        fn discriminant<BT: DeviceLayerTypes>(d: &DeviceId<BT>) -> u8 {
            match d {
                // Each variant must provide a unique discriminant!
                DeviceId::Ethernet(_) => 0,
                DeviceId::Loopback(_) => 1,
                DeviceId::PureIp(_) => 2,
                DeviceId::Blackhole(_) => 3,
            }
        }
        match (self, other) {
            (DeviceId::Ethernet(me), DeviceId::Ethernet(other)) => me.cmp(other),
            (DeviceId::Loopback(me), DeviceId::Loopback(other)) => me.cmp(other),
            (DeviceId::PureIp(me), DeviceId::PureIp(other)) => me.cmp(other),
            (DeviceId::Blackhole(me), DeviceId::Blackhole(other)) => me.cmp(other),
            (me @ DeviceId::Ethernet(_), other)
            | (me @ DeviceId::Loopback(_), other)
            | (me @ DeviceId::PureIp(_), other)
            | (me @ DeviceId::Blackhole(_), other) => discriminant(me).cmp(&discriminant(other)),
        }
    }
}

impl<BT: DeviceLayerTypes> From<EthernetDeviceId<BT>> for DeviceId<BT> {
    fn from(id: EthernetDeviceId<BT>) -> DeviceId<BT> {
        DeviceId::Ethernet(id)
    }
}

impl<BT: DeviceLayerTypes> From<LoopbackDeviceId<BT>> for DeviceId<BT> {
    fn from(id: LoopbackDeviceId<BT>) -> DeviceId<BT> {
        DeviceId::Loopback(id)
    }
}

impl<BT: DeviceLayerTypes> From<PureIpDeviceId<BT>> for DeviceId<BT> {
    fn from(id: PureIpDeviceId<BT>) -> DeviceId<BT> {
        DeviceId::PureIp(id)
    }
}

impl<BT: DeviceLayerTypes> From<BlackholeDeviceId<BT>> for DeviceId<BT> {
    fn from(id: BlackholeDeviceId<BT>) -> DeviceId<BT> {
        DeviceId::Blackhole(id)
    }
}

impl<BT: DeviceLayerTypes> DeviceId<BT> {
    /// Downgrade to a [`WeakDeviceId`].
    pub fn downgrade(&self) -> WeakDeviceId<BT> {
        for_any_device_id!(DeviceId, self, id => id.downgrade().into())
    }

    /// Returns the bindings identifier associated with the device.
    pub fn bindings_id(&self) -> &BT::DeviceIdentifier {
        for_any_device_id!(DeviceId, self, id => id.bindings_id())
    }
}

impl<BT: DeviceLayerTypes> DeviceIdentifier for DeviceId<BT> {
    fn is_loopback(&self) -> bool {
        match self {
            DeviceId::Loopback(_) => true,
            DeviceId::Ethernet(_) | DeviceId::PureIp(_) | DeviceId::Blackhole(_) => false,
        }
    }
}

impl<BT: DeviceLayerTypes> StrongDeviceIdentifier for DeviceId<BT> {
    type Weak = WeakDeviceId<BT>;

    fn downgrade(&self) -> Self::Weak {
        self.downgrade()
    }
}

impl<BT: DeviceLayerTypes> Debug for DeviceId<BT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for_any_device_id!(DeviceId, self, id => Debug::fmt(id, f))
    }
}

impl<BT: DeviceLayerTypes> InterfaceProperties<BT::DeviceClass> for DeviceId<BT> {
    fn id_matches(&self, id: &NonZeroU64) -> bool {
        self.bindings_id().id_matches(id)
    }

    fn name_matches(&self, name: &str) -> bool {
        self.bindings_id().name_matches(name)
    }

    fn device_class_matches(&self, device_class: &BT::DeviceClass) -> bool {
        for_any_device_id!(
            DeviceId,
            self,
            id => id.external_state().device_class_matches(device_class)
        )
    }
}

/// A base weak device identifier.
///
/// Allows multiple device implementations to share the same shape for
/// maintaining reference identifiers.
#[derive(Derivative)]
#[derivative(Clone(bound = ""))]
pub struct BaseWeakDeviceId<T: DeviceStateSpec, BT: DeviceLayerTypes> {
    // NB: This is not a tuple struct because regular structs play nicer with
    // type aliases, which is how we use BaseDeviceId.
    cookie: Arc<WeakCookie<T, BT>>,
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> PartialEq for BaseWeakDeviceId<T, BT> {
    fn eq(&self, other: &Self) -> bool {
        self.cookie.weak_ref.ptr_eq(&other.cookie.weak_ref)
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> Eq for BaseWeakDeviceId<T, BT> {}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> Hash for BaseWeakDeviceId<T, BT> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.cookie.weak_ref.hash(state)
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> PartialEq<BaseDeviceId<T, BT>>
    for BaseWeakDeviceId<T, BT>
{
    fn eq(&self, other: &BaseDeviceId<T, BT>) -> bool {
        <BaseDeviceId<T, BT> as PartialEq<BaseWeakDeviceId<T, BT>>>::eq(other, self)
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> Debug for BaseWeakDeviceId<T, BT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { cookie } = self;
        write!(f, "Weak{}({:?})", T::DEBUG_TYPE, cookie.bindings_id)
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> DeviceIdentifier for BaseWeakDeviceId<T, BT> {
    fn is_loopback(&self) -> bool {
        T::IS_LOOPBACK
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> WeakDeviceIdentifier for BaseWeakDeviceId<T, BT> {
    type Strong = BaseDeviceId<T, BT>;

    fn upgrade(&self) -> Option<Self::Strong> {
        self.upgrade()
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> BaseWeakDeviceId<T, BT> {
    /// Attempts to upgrade the ID to a strong ID, failing if the
    /// device no longer exists.
    pub fn upgrade(&self) -> Option<BaseDeviceId<T, BT>> {
        let Self { cookie } = self;
        cookie.weak_ref.upgrade().map(|rc| BaseDeviceId { rc })
    }

    /// Returns the bindings identifier associated with the device.
    pub fn bindings_id(&self) -> &BT::DeviceIdentifier {
        &self.cookie.bindings_id
    }
}

/// A base device identifier.
///
/// Allows multiple device implementations to share the same shape for
/// maintaining reference identifiers.
#[derive(Derivative)]
#[derivative(Hash(bound = ""), Eq(bound = ""), PartialEq(bound = ""))]
pub struct BaseDeviceId<T: DeviceStateSpec, BT: DeviceLayerTypes> {
    // NB: This is not a tuple struct because regular structs play nicer with
    // type aliases, which is how we use BaseDeviceId.
    rc: StrongRc<BaseDeviceState<T, BT>>,
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> Clone for BaseDeviceId<T, BT> {
    #[cfg_attr(feature = "instrumented", track_caller)]
    fn clone(&self) -> Self {
        let Self { rc } = self;
        Self { rc: StrongRc::clone(rc) }
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> PartialEq<BaseWeakDeviceId<T, BT>>
    for BaseDeviceId<T, BT>
{
    fn eq(&self, BaseWeakDeviceId { cookie }: &BaseWeakDeviceId<T, BT>) -> bool {
        let Self { rc: me_rc } = self;
        StrongRc::weak_ptr_eq(me_rc, &cookie.weak_ref)
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> PartialEq<BasePrimaryDeviceId<T, BT>>
    for BaseDeviceId<T, BT>
{
    fn eq(&self, BasePrimaryDeviceId { rc: other_rc }: &BasePrimaryDeviceId<T, BT>) -> bool {
        let Self { rc: me_rc } = self;
        PrimaryRc::ptr_eq(other_rc, me_rc)
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> PartialOrd for BaseDeviceId<T, BT> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> Ord for BaseDeviceId<T, BT> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        let Self { rc: me } = self;
        let Self { rc: other } = other;

        StrongRc::ptr_cmp(me, other)
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> Debug for BaseDeviceId<T, BT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { rc } = self;
        write!(f, "{}({:?})", T::DEBUG_TYPE, rc.weak_cookie.bindings_id)
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> DeviceIdentifier for BaseDeviceId<T, BT> {
    fn is_loopback(&self) -> bool {
        T::IS_LOOPBACK
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> StrongDeviceIdentifier for BaseDeviceId<T, BT> {
    type Weak = BaseWeakDeviceId<T, BT>;

    fn downgrade(&self) -> Self::Weak {
        self.downgrade()
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> BaseDeviceId<T, BT> {
    /// Returns a reference to the device state.
    ///
    /// Requires an OriginTracker to ensure this is being access from the proper
    /// context and disallow usage in bindings.
    pub fn device_state(&self, tracker: &OriginTracker) -> &IpLinkDeviceState<T, BT> {
        debug_assert_eq!(tracker, &self.rc.ip.origin);
        &self.rc.ip
    }

    /// Returns a reference to the external state for the device.
    pub fn external_state(&self) -> &T::External<BT> {
        &self.rc.external_state
    }

    /// Returns the bindings identifier associated with the device.
    pub fn bindings_id(&self) -> &BT::DeviceIdentifier {
        &self.rc.weak_cookie.bindings_id
    }

    /// Downgrades the ID to an [`EthernetWeakDeviceId`].
    pub fn downgrade(&self) -> BaseWeakDeviceId<T, BT> {
        let Self { rc } = self;
        BaseWeakDeviceId { cookie: Arc::clone(&rc.weak_cookie) }
    }
}

/// The primary reference to a device.
pub struct BasePrimaryDeviceId<T: DeviceStateSpec, BT: DeviceLayerTypes> {
    // NB: This is not a tuple struct because regular structs play nicer with
    // type aliases, which is how we use BaseDeviceId.
    rc: PrimaryRc<BaseDeviceState<T, BT>>,
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> Debug for BasePrimaryDeviceId<T, BT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { rc } = self;
        write!(f, "Primary{}({:?})", T::DEBUG_TYPE, rc.weak_cookie.bindings_id)
    }
}

impl<T: DeviceStateSpec, BT: DeviceLayerTypes> BasePrimaryDeviceId<T, BT> {
    /// Returns a strong clone of this primary ID.
    #[cfg_attr(feature = "instrumented", track_caller)]
    pub fn clone_strong(&self) -> BaseDeviceId<T, BT> {
        let Self { rc } = self;
        BaseDeviceId { rc: PrimaryRc::clone_strong(rc) }
    }

    pub(crate) fn new<F: FnOnce(BaseWeakDeviceId<T, BT>) -> IpLinkDeviceState<T, BT>>(
        ip: F,
        external_state: T::External<BT>,
        bindings_id: BT::DeviceIdentifier,
    ) -> Self {
        Self {
            rc: PrimaryRc::new_cyclic(move |weak_ref| {
                let weak_cookie = Arc::new(WeakCookie { bindings_id, weak_ref });
                let ip = ip(BaseWeakDeviceId { cookie: Arc::clone(&weak_cookie) });
                BaseDeviceState { ip, external_state, weak_cookie }
            }),
        }
    }

    pub(crate) fn into_inner(self) -> PrimaryRc<BaseDeviceState<T, BT>> {
        self.rc
    }
}

/// A strong device ID identifying an ethernet device.
///
/// This device ID is like [`DeviceId`] but specifically for ethernet devices.
pub type EthernetDeviceId<BT> = BaseDeviceId<EthernetLinkDevice, BT>;
/// A weak device ID identifying an ethernet device.
pub type EthernetWeakDeviceId<BT> = BaseWeakDeviceId<EthernetLinkDevice, BT>;
/// The primary Ethernet device reference.
pub type EthernetPrimaryDeviceId<BT> = BasePrimaryDeviceId<EthernetLinkDevice, BT>;

#[cfg(any(test, feature = "testutils"))]
mod testutil {
    use super::*;

    impl<BT: DeviceLayerTypes> TryFrom<DeviceId<BT>> for EthernetDeviceId<BT> {
        type Error = DeviceId<BT>;
        fn try_from(id: DeviceId<BT>) -> Result<EthernetDeviceId<BT>, DeviceId<BT>> {
            match id {
                DeviceId::Ethernet(id) => Ok(id),
                DeviceId::Loopback(_) | DeviceId::PureIp(_) | DeviceId::Blackhole(_) => Err(id),
            }
        }
    }

    impl<BT: DeviceLayerTypes> DeviceId<BT> {
        /// Extracts an ethernet device from self or panics.
        pub fn unwrap_ethernet(self) -> EthernetDeviceId<BT> {
            assert_matches::assert_matches!(self, DeviceId::Ethernet(e) => e)
        }
    }
}
