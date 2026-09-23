// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extensions for the fuchsia.net.interfaces FIDL library.

#![deny(missing_docs)]

pub mod admin;
mod reachability;

pub use reachability::{is_globally_routable, to_reachability_stream, wait_for_reachability};

use anyhow::Context as _;
use derivative::Derivative;
use fidl_table_validation::*;
use flex_client::ProxyHasDomain;
use flex_fuchsia_hardware_network as fhardware_network;
use flex_fuchsia_net_interfaces as fnet_interfaces;
use futures::{Stream, TryStreamExt as _};
use std::collections::btree_map::{self, BTreeMap};
use std::collections::hash_map::{self, HashMap};
use std::convert::{Infallible as Never, TryFrom as _};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use thiserror::Error;

/// Like [`fnet_interfaces::PortClass`], with the inner `device` flattened.
///
/// This type also derives additional impls that are not available on
/// `fnet_interfaces::PortClass`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[allow(missing_docs)]
pub enum PortClass {
    Loopback,
    Virtual,
    Ethernet,
    WlanClient,
    WlanAp,
    Ppp,
    Bridge,
    Lowpan,
    Blackhole,
}

impl PortClass {
    /// Returns `true` if this `PortClass` is `Loopback`.
    pub fn is_loopback(&self) -> bool {
        match self {
            PortClass::Loopback => true,
            PortClass::Virtual
            | PortClass::Blackhole
            | PortClass::Ethernet
            | PortClass::WlanClient
            | PortClass::WlanAp
            | PortClass::Ppp
            | PortClass::Bridge
            | PortClass::Lowpan => false,
        }
    }
}

/// An Error returned when converting from `fnet_interfaces::PortClass` to
/// `PortClass`.
#[derive(Debug, Error, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum UnknownPortClassError {
    #[error(transparent)]
    NetInterfaces(UnknownNetInterfacesPortClassError),
    #[error(transparent)]
    HardwareNetwork(UnknownHardwareNetworkPortClassError),
}

/// An error returned when `fnet_interfaces::PortClass` is an unknown variant.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown fuchsia.net.interfaces/PortClass ordinal: {unknown_ordinal}")]
pub struct UnknownNetInterfacesPortClassError {
    unknown_ordinal: u64,
}

/// An error returned when `fhardware_network::PortClass` is an unknown variant.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown fuchsia.hardware.network/PortClass ordinal: {unknown_ordinal}")]
pub struct UnknownHardwareNetworkPortClassError {
    unknown_ordinal: u16,
}

impl TryFrom<fnet_interfaces::PortClass> for PortClass {
    type Error = UnknownPortClassError;
    fn try_from(port_class: fnet_interfaces::PortClass) -> Result<Self, Self::Error> {
        match port_class {
            fnet_interfaces::PortClass::Loopback(fnet_interfaces::Empty) => Ok(PortClass::Loopback),
            fnet_interfaces::PortClass::Blackhole(fnet_interfaces::Empty) => {
                Ok(PortClass::Blackhole)
            }
            fnet_interfaces::PortClass::Device(port_class) => {
                PortClass::try_from(port_class).map_err(UnknownPortClassError::HardwareNetwork)
            }
            fnet_interfaces::PortClass::__SourceBreaking { unknown_ordinal } => {
                Err(UnknownPortClassError::NetInterfaces(UnknownNetInterfacesPortClassError {
                    unknown_ordinal,
                }))
            }
        }
    }
}

impl From<PortClass> for fnet_interfaces::PortClass {
    fn from(port_class: PortClass) -> Self {
        match port_class {
            PortClass::Loopback => fnet_interfaces::PortClass::Loopback(fnet_interfaces::Empty),
            PortClass::Blackhole => fnet_interfaces::PortClass::Blackhole(fnet_interfaces::Empty),
            PortClass::Virtual => {
                fnet_interfaces::PortClass::Device(fhardware_network::PortClass::Virtual)
            }
            PortClass::Ethernet => {
                fnet_interfaces::PortClass::Device(fhardware_network::PortClass::Ethernet)
            }
            PortClass::WlanClient => {
                fnet_interfaces::PortClass::Device(fhardware_network::PortClass::WlanClient)
            }
            PortClass::WlanAp => {
                fnet_interfaces::PortClass::Device(fhardware_network::PortClass::WlanAp)
            }
            PortClass::Ppp => fnet_interfaces::PortClass::Device(fhardware_network::PortClass::Ppp),
            PortClass::Bridge => {
                fnet_interfaces::PortClass::Device(fhardware_network::PortClass::Bridge)
            }
            PortClass::Lowpan => {
                fnet_interfaces::PortClass::Device(fhardware_network::PortClass::Lowpan)
            }
        }
    }
}

impl TryFrom<fhardware_network::PortClass> for PortClass {
    type Error = UnknownHardwareNetworkPortClassError;
    fn try_from(port_class: fhardware_network::PortClass) -> Result<Self, Self::Error> {
        match port_class {
            fhardware_network::PortClass::Virtual => Ok(PortClass::Virtual),
            fhardware_network::PortClass::Ethernet => Ok(PortClass::Ethernet),
            fhardware_network::PortClass::WlanClient => Ok(PortClass::WlanClient),
            fhardware_network::PortClass::WlanAp => Ok(PortClass::WlanAp),
            fhardware_network::PortClass::Ppp => Ok(PortClass::Ppp),
            fhardware_network::PortClass::Bridge => Ok(PortClass::Bridge),
            fhardware_network::PortClass::Lowpan => Ok(PortClass::Lowpan),
            fhardware_network::PortClass::__SourceBreaking { unknown_ordinal } => {
                Err(UnknownHardwareNetworkPortClassError { unknown_ordinal })
            }
        }
    }
}

/// The type of [`Properties::port_identity_koid`].
///
/// This is a new-type to handle the OS abstraction around the availability of
/// [`zx::Koid`] type, offering easy conversions for use both in host and target
/// code.
#[cfg(not(target_os = "fuchsia"))]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct PortIdentityKoid(zx_types::zx_koid_t);

#[cfg(not(target_os = "fuchsia"))]
impl PortIdentityKoid {
    /// Creates a [`PortIdentityKoid`] from a raw `koid` value.
    pub fn from_raw(koid: zx_types::zx_koid_t) -> Self {
        Self(koid)
    }

    /// Returns the raw koid value.
    pub fn raw_koid(&self) -> zx_types::zx_koid_t {
        let Self(raw) = self;
        *raw
    }
}

/// Type alias as [`zx::Koid`] when available in Fuchsia.
#[cfg(target_os = "fuchsia")]
pub type PortIdentityKoid = zx::Koid;

/// Properties of a network interface.
#[derive(Derivative, ValidFidlTable)]
#[derivative(Clone(bound = ""), Debug(bound = ""), Eq(bound = ""), PartialEq(bound = ""))]
#[fidl_table_src(fnet_interfaces::Properties)]
pub struct Properties<I: FieldInterests> {
    /// An opaque identifier for the interface. Its value will not be reused
    /// even if the device is removed and subsequently re-added. Immutable.
    pub id: NonZeroU64,
    /// The name of the interface. Immutable.
    pub name: String,
    /// The device is enabled and its physical state is online.
    pub online: bool,
    /// The addresses currently assigned to the interface.
    pub addresses: Vec<Address<I>>,
    /// Whether there is a default IPv4 route through this interface.
    pub has_default_ipv4_route: bool,
    /// Whether there is a default IPv6 route through this interface.
    pub has_default_ipv6_route: bool,
    /// The device type of the interface. Immutable.
    pub port_class: PortClass,
    /// The KOID of the event vended out by the interface's backing
    /// `fuchsia.hardware.network/Port`.
    ///
    /// Callers may use this identifier to correlate port instances to installed
    /// interfaces.
    ///
    /// Immutable. Absent for interfaces not backed by a
    /// `fuchsia.hardware.network/Port`.
    #[fidl_field_type(optional_converter = PortIdentityKoidConverter)]
    pub port_identity_koid: Option<PortIdentityKoid>,
}

/// An address and its properties.
#[derive(Derivative, ValidFidlTable)]
#[derivative(
    Clone(bound = ""),
    Debug(bound = ""),
    Eq(bound = ""),
    PartialEq(bound = ""),
    Hash(bound = "")
)]
#[fidl_table_src(fnet_interfaces::Address)]
#[fidl_table_strict]
pub struct Address<I: FieldInterests> {
    /// The address and prefix length.
    pub addr: flex_fuchsia_net::Subnet,
    /// The time after which the address will no longer be valid.
    ///
    /// Its value must be greater than 0. A value of zx.time.INFINITE indicates
    /// that the address will always be valid.
    #[fidl_field_type(optional_converter = InterestConverter::<I, ValidUntilInterest>)]
    pub valid_until: FromInterest<I, ValidUntilInterest>,
    /// Preferred lifetime information.
    #[fidl_field_type(optional_converter = InterestConverter::<I, PreferredLifetimeInfoInterest>)]
    pub preferred_lifetime_info: FromInterest<I, PreferredLifetimeInfoInterest>,
    /// The address's assignment state.
    pub assignment_state: fnet_interfaces::AddressAssignmentState,
}

/// Information about the preferred lifetime of an IP address or delegated
/// prefix.
///
/// Type-safe version of [`fnet_interfaces::PreferredLifetimeInfo`].
#[derive(Eq, PartialEq, Ord, PartialOrd, Debug, Copy, Clone, Hash)]
#[allow(missing_docs)]
pub enum PreferredLifetimeInfo {
    PreferredUntil(PositiveMonotonicInstant),
    Deprecated,
}

impl PreferredLifetimeInfo {
    /// Returns a lifetime information for an address that is always preferred.
    pub const fn preferred_forever() -> Self {
        Self::PreferredUntil(PositiveMonotonicInstant::INFINITE_FUTURE)
    }

    /// Converts to the equivalent FIDL type.
    pub const fn to_fidl(self) -> fnet_interfaces::PreferredLifetimeInfo {
        match self {
            PreferredLifetimeInfo::Deprecated => {
                fnet_interfaces::PreferredLifetimeInfo::Deprecated(fnet_interfaces::Empty)
            }
            PreferredLifetimeInfo::PreferredUntil(v) => {
                fnet_interfaces::PreferredLifetimeInfo::PreferredUntil(v.into_nanos())
            }
        }
    }
}

impl TryFrom<fnet_interfaces::PreferredLifetimeInfo> for PreferredLifetimeInfo {
    type Error = NotPositiveMonotonicInstantError;

    fn try_from(value: fnet_interfaces::PreferredLifetimeInfo) -> Result<Self, Self::Error> {
        match value {
            fnet_interfaces::PreferredLifetimeInfo::Deprecated(fnet_interfaces::Empty) => {
                Ok(Self::Deprecated)
            }
            fnet_interfaces::PreferredLifetimeInfo::PreferredUntil(v) => {
                Ok(Self::PreferredUntil(v.try_into()?))
            }
        }
    }
}

impl From<PreferredLifetimeInfo> for fnet_interfaces::PreferredLifetimeInfo {
    fn from(value: PreferredLifetimeInfo) -> Self {
        value.to_fidl()
    }
}

/// The error returned by attempting to convert a non positive instant to
/// `PositiveMonotonicInstant`.
#[derive(Error, Debug)]
#[error("{0} is not a positive monotonic instant")]
pub struct NotPositiveMonotonicInstantError(i64);

/// A positive monotonic instant.
#[derive(Eq, PartialEq, Ord, PartialOrd, Debug, Copy, Clone, Hash)]
pub struct PositiveMonotonicInstant(i64);

impl PositiveMonotonicInstant {
    /// An instant in the infinite future.
    pub const INFINITE_FUTURE: Self = Self(zx_types::ZX_TIME_INFINITE);

    /// Returns the nanoseconds value for the instant.
    pub const fn into_nanos(self) -> i64 {
        let Self(v) = self;
        v
    }

    /// Returns the the positive nanoseconds value from the monotonic timestamp
    /// in nanoseconds, if it's positive.
    pub const fn from_nanos(v: i64) -> Option<Self> {
        if v > 0 { Some(Self(v)) } else { None }
    }

    /// Returns true if `self` is equal to `INFINITE_FUTURE`.
    pub fn is_infinite(&self) -> bool {
        self == &Self::INFINITE_FUTURE
    }
}

#[cfg(target_os = "fuchsia")]
impl From<PositiveMonotonicInstant> for zx::MonotonicInstant {
    fn from(PositiveMonotonicInstant(v): PositiveMonotonicInstant) -> Self {
        zx::MonotonicInstant::from_nanos(v)
    }
}

#[cfg(target_os = "fuchsia")]
impl TryFrom<zx::MonotonicInstant> for PositiveMonotonicInstant {
    type Error = NotPositiveMonotonicInstantError;

    fn try_from(value: zx::MonotonicInstant) -> Result<Self, Self::Error> {
        Self::try_from(value.into_nanos())
    }
}

impl From<PositiveMonotonicInstant> for zx_types::zx_time_t {
    fn from(value: PositiveMonotonicInstant) -> Self {
        value.into_nanos()
    }
}

impl TryFrom<zx_types::zx_time_t> for PositiveMonotonicInstant {
    type Error = NotPositiveMonotonicInstantError;

    fn try_from(value: zx_types::zx_time_t) -> Result<Self, Self::Error> {
        Self::from_nanos(value).ok_or(NotPositiveMonotonicInstantError(value))
    }
}

/// Interface watcher event update errors.
#[derive(Error, Debug)]
pub enum UpdateError {
    /// The update attempted to add an already-known added interface into local state.
    #[error("duplicate added event {0:?}")]
    DuplicateAdded(fnet_interfaces::Properties),
    /// The update attempted to add an already-known existing interface into local state.
    #[error("duplicate existing event {0:?}")]
    DuplicateExisting(fnet_interfaces::Properties),
    /// The event contained one or more invalid properties.
    #[error("failed to validate Properties FIDL table: {0}")]
    InvalidProperties(#[from] PropertiesValidationError),
    /// The event contained one or more invalid addresses.
    #[error("failed to validate Address FIDL table: {0}")]
    InvalidAddress(#[from] AddressValidationError),
    /// The event was required to have contained an ID, but did not.
    #[error("changed event with missing ID {0:?}")]
    MissingId(fnet_interfaces::Properties),
    /// The event did not contain any changes.
    #[error("changed event contains no changed fields {0:?}")]
    EmptyChange(fnet_interfaces::Properties),
    /// The update removed the only interface in the local state.
    #[error("interface has been removed")]
    Removed,
    /// The event contained changes for an interface that did not exist in local state.
    #[error("unknown interface changed {0:?}")]
    UnknownChanged(fnet_interfaces::Properties),
    /// The event removed an interface that did not exist in local state.
    #[error("unknown interface with id {0} deleted")]
    UnknownRemoved(u64),
    /// The event included an interface id = 0, which should never happen.
    #[error("encountered 0 interface id")]
    ZeroInterfaceId,
}

/// The result of updating network interface state with an event.
#[derive(Derivative)]
#[derivative(Debug(bound = "S: Debug"), PartialEq(bound = "S: PartialEq"))]
pub enum UpdateResult<'a, S, I: FieldInterests> {
    /// The update did not change the local state.
    NoChange,
    /// The update inserted an existing interface into the local state.
    Existing {
        /// The properties,
        properties: &'a Properties<I>,
        /// The state.
        state: &'a mut S,
    },
    /// The update inserted an added interface into the local state.
    Added {
        /// The properties,
        properties: &'a Properties<I>,
        /// The state.
        state: &'a mut S,
    },
    /// The update changed an existing interface in the local state.
    Changed {
        /// The previous values of any properties which changed.
        ///
        /// This is sparsely populated: none of the immutable properties are present (they can
        /// all be found on `current`), and a mutable property is present with its value pre-update
        /// iff it has changed as a result of the update.
        previous: fnet_interfaces::Properties,
        /// The properties of the interface post-update.
        current: &'a Properties<I>,
        /// The state of the interface.
        state: &'a mut S,
    },
    /// The update removed an interface from the local state.
    Removed(PropertiesAndState<S, I>),
}

/// The properties and state for an interface.
#[derive(Derivative)]
#[derivative(
    Clone(bound = "S: Clone"),
    Debug(bound = "S: Debug"),
    Eq(bound = "S: Eq"),
    PartialEq(bound = "S: PartialEq")
)]
pub struct PropertiesAndState<S, I: FieldInterests> {
    /// Properties.
    pub properties: Properties<I>,
    /// State.
    pub state: S,
}

/// A trait for types holding interface state that can be updated by change events.
pub trait Update<S> {
    /// The expected watcher interest type for this update target.
    type Interest: FieldInterests;

    /// Update state with the interface change event.
    fn update(
        &mut self,
        event: EventWithInterest<Self::Interest>,
    ) -> Result<UpdateResult<'_, S, Self::Interest>, UpdateError>;
}

impl<S, I: FieldInterests> Update<S> for PropertiesAndState<S, I> {
    type Interest = I;
    fn update(
        &mut self,
        event: EventWithInterest<I>,
    ) -> Result<UpdateResult<'_, S, I>, UpdateError> {
        let Self { properties, state } = self;
        match event.into_inner() {
            fnet_interfaces::Event::Existing(existing) => {
                let existing = Properties::<I>::try_from(existing)?;
                if existing.id == properties.id {
                    return Err(UpdateError::DuplicateExisting(existing.into()));
                }
            }
            fnet_interfaces::Event::Added(added) => {
                let added = Properties::<I>::try_from(added)?;
                if added.id == properties.id {
                    return Err(UpdateError::DuplicateAdded(added.into()));
                }
            }
            fnet_interfaces::Event::Changed(mut change) => {
                let fnet_interfaces::Properties {
                    id,
                    name: _,
                    port_class: _,
                    online,
                    has_default_ipv4_route,
                    has_default_ipv6_route,
                    addresses,
                    port_identity_koid: _,
                    __source_breaking: _,
                } = &mut change;
                if let Some(id) = *id {
                    if properties.id.get() == id {
                        let mut changed = false;
                        macro_rules! swap_if_some {
                            ($field:ident) => {
                                if let Some($field) = $field {
                                    if properties.$field != *$field {
                                        std::mem::swap(&mut properties.$field, $field);
                                        changed = true;
                                    }
                                }
                            };
                        }
                        swap_if_some!(online);
                        swap_if_some!(has_default_ipv4_route);
                        swap_if_some!(has_default_ipv6_route);
                        if let Some(addresses) = addresses {
                            // NB The following iterator comparison assumes that the server is
                            // well-behaved and will not send a permutation of the existing
                            // addresses with no actual changes (additions or removals). Making the
                            // comparison via set equality is possible, but more expensive than
                            // it's worth.
                            // TODO(https://github.com/rust-lang/rust/issues/64295) Use `eq_by` to
                            // compare the iterators once stabilized.
                            if addresses.len() != properties.addresses.len()
                                || !addresses
                                    .iter()
                                    .zip(
                                        properties
                                            .addresses
                                            .iter()
                                            .cloned()
                                            .map(fnet_interfaces::Address::from),
                                    )
                                    .all(|(a, b)| *a == b)
                            {
                                let previous_len = properties.addresses.len();
                                // NB This is equivalent to Vec::try_extend, if such a method
                                // existed.
                                properties.addresses.reserve(addresses.len());
                                for address in addresses.drain(..).map(Address::try_from) {
                                    properties.addresses.push(address?);
                                }
                                addresses.extend(
                                    properties.addresses.drain(..previous_len).map(Into::into),
                                );
                                changed = true;
                            }
                        }
                        if changed {
                            change.id = None;
                            return Ok(UpdateResult::Changed {
                                previous: change,
                                current: properties,
                                state,
                            });
                        } else {
                            return Err(UpdateError::EmptyChange(change));
                        }
                    }
                } else {
                    return Err(UpdateError::MissingId(change));
                }
            }
            fnet_interfaces::Event::Removed(removed_id) => {
                if properties.id.get() == removed_id {
                    return Err(UpdateError::Removed);
                }
            }
            fnet_interfaces::Event::Idle(fnet_interfaces::Empty {}) => {}
        }
        Ok(UpdateResult::NoChange)
    }
}

impl<S: Default, I: FieldInterests> Update<S> for InterfaceState<S, I> {
    type Interest = I;
    fn update(
        &mut self,
        event: EventWithInterest<I>,
    ) -> Result<UpdateResult<'_, S, I>, UpdateError> {
        fn get_properties<S, I: FieldInterests>(
            state: &mut InterfaceState<S, I>,
        ) -> &mut PropertiesAndState<S, I> {
            match state {
                InterfaceState::Known(properties) => properties,
                InterfaceState::Unknown(id) => unreachable!(
                    "matched `Unknown({})` immediately after assigning with `Known`",
                    id
                ),
            }
        }
        match self {
            InterfaceState::Unknown(id) => match event.into_inner() {
                fnet_interfaces::Event::Existing(existing) => {
                    let properties = Properties::try_from(existing)?;
                    if properties.id.get() == *id {
                        *self = InterfaceState::Known(PropertiesAndState {
                            properties,
                            state: S::default(),
                        });
                        let PropertiesAndState { properties, state } = get_properties(self);
                        return Ok(UpdateResult::Existing { properties, state });
                    }
                }
                fnet_interfaces::Event::Added(added) => {
                    let properties = Properties::try_from(added)?;
                    if properties.id.get() == *id {
                        *self = InterfaceState::Known(PropertiesAndState {
                            properties,
                            state: S::default(),
                        });
                        let PropertiesAndState { properties, state } = get_properties(self);
                        return Ok(UpdateResult::Added { properties, state });
                    }
                }
                fnet_interfaces::Event::Changed(change) => {
                    if let Some(change_id) = change.id {
                        if change_id == *id {
                            return Err(UpdateError::UnknownChanged(change));
                        }
                    } else {
                        return Err(UpdateError::MissingId(change));
                    }
                }
                fnet_interfaces::Event::Removed(removed_id) => {
                    if removed_id == *id {
                        return Err(UpdateError::UnknownRemoved(removed_id));
                    }
                }
                fnet_interfaces::Event::Idle(fnet_interfaces::Empty {}) => {}
            },
            InterfaceState::Known(properties) => return properties.update(event),
        }
        Ok(UpdateResult::NoChange)
    }
}

/// An error indicated an unexpected zero value.
pub struct ZeroError {}

/// A type that may fallibly convert from a u64 because the value is 0.
pub trait TryFromMaybeNonZero: Sized {
    /// Try to convert the u64 into `Self`.
    fn try_from(value: u64) -> Result<Self, ZeroError>;
}

impl TryFromMaybeNonZero for u64 {
    fn try_from(value: u64) -> Result<Self, ZeroError> {
        Ok(value)
    }
}

impl TryFromMaybeNonZero for NonZeroU64 {
    fn try_from(value: u64) -> Result<Self, ZeroError> {
        NonZeroU64::new(value).ok_or(ZeroError {})
    }
}

macro_rules! impl_map {
    ($map_type:ident, $map_mod:tt) => {
        impl<K, S, I> Update<S> for $map_type<K, PropertiesAndState<S, I>>
        where
            K: TryFromMaybeNonZero + Copy + From<NonZeroU64> + Eq + Ord + std::hash::Hash,
            S: Default,
            I: FieldInterests,
        {
            type Interest = I;

            fn update(
                &mut self,
                event: EventWithInterest<I>,
            ) -> Result<UpdateResult<'_, S, I>, UpdateError> {
                match event.into_inner() {
                    fnet_interfaces::Event::Existing(existing) => {
                        let existing = Properties::try_from(existing)?;
                        match self.entry(existing.id.into()) {
                            $map_mod::Entry::Occupied(_) => {
                                Err(UpdateError::DuplicateExisting(existing.into()))
                            }
                            $map_mod::Entry::Vacant(entry) => {
                                let PropertiesAndState { properties, state } =
                                    entry.insert(PropertiesAndState {
                                        properties: existing,
                                        state: S::default(),
                                    });
                                Ok(UpdateResult::Existing { properties, state })
                            }
                        }
                    }
                    fnet_interfaces::Event::Added(added) => {
                        let added = Properties::try_from(added)?;
                        match self.entry(added.id.into()) {
                            $map_mod::Entry::Occupied(_) => {
                                Err(UpdateError::DuplicateAdded(added.into()))
                            }
                            $map_mod::Entry::Vacant(entry) => {
                                let PropertiesAndState { properties, state } =
                                    entry.insert(PropertiesAndState {
                                        properties: added,
                                        state: S::default(),
                                    });
                                Ok(UpdateResult::Added { properties, state })
                            }
                        }
                    }
                    fnet_interfaces::Event::Changed(change) => {
                        let id = if let Some(id) = change.id {
                            id
                        } else {
                            return Err(UpdateError::MissingId(change));
                        };
                        if let Some(properties) = self.get_mut(
                            &K::try_from(id)
                                .map_err(|ZeroError {}| UpdateError::ZeroInterfaceId)?,
                        ) {
                            properties.update(EventWithInterest::new(
                                fnet_interfaces::Event::Changed(change),
                            ))
                        } else {
                            Err(UpdateError::UnknownChanged(change))
                        }
                    }
                    fnet_interfaces::Event::Removed(removed_id) => {
                        if let Some(properties) = self.remove(
                            &K::try_from(removed_id)
                                .map_err(|ZeroError {}| UpdateError::ZeroInterfaceId)?,
                        ) {
                            Ok(UpdateResult::Removed(properties))
                        } else {
                            Err(UpdateError::UnknownRemoved(removed_id))
                        }
                    }
                    fnet_interfaces::Event::Idle(fnet_interfaces::Empty {}) => {
                        Ok(UpdateResult::NoChange)
                    }
                }
            }
        }
    };
}

impl_map!(BTreeMap, btree_map);
impl_map!(HashMap, hash_map);

/// Interface watcher operational errors.
#[derive(Error, Debug)]
pub enum WatcherOperationError<S: Debug, B: Update<S> + Debug> {
    /// Watcher event stream yielded an error.
    #[error("event stream error: {0}")]
    EventStream(fidl::Error),
    /// Watcher event stream yielded an event that could not be applied to the local state.
    #[error("failed to update: {0}")]
    Update(UpdateError),
    /// Watcher event stream ended unexpectedly.
    #[error("watcher event stream ended unexpectedly, final state: {final_state:?}")]
    UnexpectedEnd {
        /// The local state at the time of the watcher event stream's end.
        final_state: B,
        /// Marker for the state held alongside interface properties.
        marker: std::marker::PhantomData<S>,
    },
    /// Watcher event stream yielded an event with unexpected type.
    #[error("unexpected event type: {0:?}")]
    UnexpectedEvent(fnet_interfaces::Event),
}

/// Interface watcher creation errors.
#[derive(Error, Debug)]
pub enum WatcherCreationError {
    /// Proxy creation failed.
    #[error("failed to create interface watcher proxy: {0}")]
    CreateProxy(fidl::Error),
    /// Watcher acquisition failed.
    #[error("failed to get interface watcher: {0}")]
    GetWatcher(fidl::Error),
}

/// Wait for a condition on interface state to be satisfied.
///
/// With the initial state in `init`, take events from `stream` and update the state, calling
/// `predicate` whenever the state changes. When `predicate` returns `Some(T)`, yield `Ok(T)`.
///
/// Since the state passed via `init` is mutably updated for every event, when this function
/// returns successfully, the state can be used as the initial state in a subsequent call with a
/// stream of events from the same watcher.
pub async fn wait_interface<S, B, St, F, T>(
    stream: St,
    init: &mut B,
    mut predicate: F,
) -> Result<T, WatcherOperationError<S, B>>
where
    S: Debug + Default,
    B: Update<S> + Clone + Debug,
    St: Stream<Item = Result<EventWithInterest<B::Interest>, fidl::Error>>,
    F: FnMut(&B) -> Option<T>,
{
    async_utils::fold::try_fold_while(
        stream.map_err(WatcherOperationError::EventStream),
        init,
        |acc, event| {
            futures::future::ready(match acc.update(event) {
                Ok(changed) => match changed {
                    UpdateResult::Existing { .. }
                    | UpdateResult::Added { .. }
                    | UpdateResult::Changed { .. }
                    | UpdateResult::Removed(_) => {
                        if let Some(rtn) = predicate(acc) {
                            Ok(async_utils::fold::FoldWhile::Done(rtn))
                        } else {
                            Ok(async_utils::fold::FoldWhile::Continue(acc))
                        }
                    }
                    UpdateResult::NoChange => Ok(async_utils::fold::FoldWhile::Continue(acc)),
                },
                Err(e) => Err(WatcherOperationError::Update(e)),
            })
        },
    )
    .await?
    .short_circuited()
    .map_err(|final_state| WatcherOperationError::UnexpectedEnd {
        final_state: final_state.clone(),
        marker: Default::default(),
    })
}

/// The local state of an interface's properties.
#[derive(Derivative)]
#[derivative(
    Clone(bound = "S: Clone"),
    Debug(bound = "S: Debug"),
    PartialEq(bound = "S: PartialEq")
)]
pub enum InterfaceState<S, I: FieldInterests> {
    /// Not yet known.
    Unknown(u64),
    /// Locally known.
    Known(PropertiesAndState<S, I>),
}

/// Wait for a condition on a specific interface to be satisfied.
///
/// Note that `stream` must be created from a watcher with interest in all
/// fields, such as one created from [`event_stream_from_state`].
///
/// With the initial state in `init`, take events from `stream` and update the state, calling
/// `predicate` whenever the state changes. When `predicate` returns `Some(T)`, yield `Ok(T)`.
///
/// Since the state passed via `init` is mutably updated for every event, when this function
/// returns successfully, the state can be used as the initial state in a subsequent call with a
/// stream of events from the same watcher.
pub async fn wait_interface_with_id<S, St, F, T, I>(
    stream: St,
    init: &mut InterfaceState<S, I>,
    mut predicate: F,
) -> Result<T, WatcherOperationError<S, InterfaceState<S, I>>>
where
    S: Default + Clone + Debug,
    St: Stream<Item = Result<EventWithInterest<I>, fidl::Error>>,
    F: FnMut(&PropertiesAndState<S, I>) -> Option<T>,
    I: FieldInterests,
{
    wait_interface(stream, init, |state| {
        match state {
            InterfaceState::Known(properties) => predicate(properties),
            // NB This is technically unreachable because a successful update will always change
            // `Unknown` to `Known` (and `Known` will stay `Known`).
            InterfaceState::Unknown(_) => None,
        }
    })
    .await
}

/// Read Existing interface events from `stream`, updating `init` until the Idle
/// event is detected, returning the resulting state.
///
/// Note that `stream` must be created from a watcher with interest in the
/// correct fields, such as one created from [`event_stream_from_state`].
pub async fn existing<S, St, B>(stream: St, init: B) -> Result<B, WatcherOperationError<S, B>>
where
    S: Debug,
    St: futures::Stream<Item = Result<EventWithInterest<B::Interest>, fidl::Error>>,
    B: Update<S> + Debug,
{
    async_utils::fold::try_fold_while(
        stream.map_err(WatcherOperationError::EventStream),
        init,
        |mut acc, event| {
            futures::future::ready(match event.inner() {
                fnet_interfaces::Event::Existing(_) => match acc.update(event) {
                    Ok::<UpdateResult<'_, _, _>, _>(_) => {
                        Ok(async_utils::fold::FoldWhile::Continue(acc))
                    }
                    Err(e) => Err(WatcherOperationError::Update(e)),
                },
                fnet_interfaces::Event::Idle(fnet_interfaces::Empty {}) => {
                    Ok(async_utils::fold::FoldWhile::Done(acc))
                }
                fnet_interfaces::Event::Added(_)
                | fnet_interfaces::Event::Removed(_)
                | fnet_interfaces::Event::Changed(_) => {
                    Err(WatcherOperationError::UnexpectedEvent(event.into_inner()))
                }
            })
        },
    )
    .await?
    .short_circuited()
    .map_err(|acc| WatcherOperationError::UnexpectedEnd {
        final_state: acc,
        marker: Default::default(),
    })
}

/// The kind of addresses included from the watcher.
#[derive(Default)]
pub enum IncludedAddresses {
    /// All addresses are returned from the watcher.
    All,
    /// Only assigned addresses are returned rom the watcher.
    #[default]
    OnlyAssigned,
}

/// The options given to [`event_stream_from_state`] for interface state
/// observation.
#[derive(Default)]
pub struct WatchOptions {
    /// The addresses returned from the watcher.
    pub included_addresses: IncludedAddresses,
    /// The port identity to filter on, if any.
    pub port_identity_koid_filter: Option<PortIdentityKoid>,
}

/// Initialize a watcher with interest in all fields and return its events as a
/// stream.
///
/// If `included_addresses` is `All`, then all addresses will be returned, not
/// just assigned addresses.
pub fn event_stream_from_state<I: FieldInterests>(
    interface_state: &fnet_interfaces::StateProxy,
    options: WatchOptions,
) -> Result<
    impl Stream<Item = Result<EventWithInterest<I>, fidl::Error>> + use<I>,
    WatcherCreationError,
> {
    let (watcher, server) =
        interface_state.domain().create_proxy::<fnet_interfaces::WatcherMarker>();
    let WatchOptions { included_addresses, port_identity_koid_filter } = options;
    interface_state
        .get_watcher(
            &fnet_interfaces::WatcherOptions {
                address_properties_interest: Some(interest_from_params::<I>()),
                include_non_assigned_addresses: Some(match included_addresses {
                    IncludedAddresses::All => true,
                    IncludedAddresses::OnlyAssigned => false,
                }),
                port_identity_koid_filter: port_identity_koid_filter.map(|p| p.raw_koid()),
                ..Default::default()
            },
            server,
        )
        .map_err(WatcherCreationError::GetWatcher)?;
    Ok(futures::stream::try_unfold(watcher, |watcher| async {
        Ok(Some((EventWithInterest::new(watcher.watch().await?), watcher)))
    }))
}

fn interest_from_params<I: FieldInterests>() -> fnet_interfaces::AddressPropertiesInterest {
    let mut interest = fnet_interfaces::AddressPropertiesInterest::empty();
    if <I::ValidUntil as MaybeInterest<_>>::ENABLED {
        interest |= fnet_interfaces::AddressPropertiesInterest::VALID_UNTIL;
    }
    if <I::PreferredLifetimeInfo as MaybeInterest<_>>::ENABLED {
        interest |= fnet_interfaces::AddressPropertiesInterest::PREFERRED_LIFETIME_INFO;
    }
    interest
}

/// A marker for a field that didn't register interest with the watcher.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Default)]
pub struct NoInterest;

mod interest {
    use super::*;

    use Debug;
    use std::hash::Hash;

    /// A trait that parameterizes interest in fields from interfaces watcher.
    ///
    /// Use [`EnableInterest`] or [`DisableInterest`] in each type to
    /// enable/disable interest in receiving those fields from the server,
    /// respectively.
    pub trait FieldInterests {
        /// Interest in the `preferred_lifetime_info` field.
        type PreferredLifetimeInfo: MaybeInterest<PreferredLifetimeInfo>;
        /// Interest in the `valid_until` field.
        type ValidUntil: MaybeInterest<PositiveMonotonicInstant>;
    }

    /// Helper trait to implement conversion with optional field interest.
    pub trait MaybeInterest<T> {
        /// Whether this is an enabled interest.
        const ENABLED: bool;

        /// The actual type carried by the validated struct.
        type Ty: Clone + Debug + Eq + Hash + PartialEq + 'static;

        /// Converts from an optional FIDL input to the target type `Self::Ty`.
        fn try_from_fidl<F: TryInto<T, Error: Into<anyhow::Error>>>(
            fidl: Option<F>,
        ) -> Result<Self::Ty, anyhow::Error>;

        /// Converts from the target type `Self::Ty` into an optional FIDL
        /// value.
        fn into_fidl<F: From<T>>(value: Self::Ty) -> Option<F>;
    }

    /// Enabled interest in a FIDL field.
    ///
    /// Use as a type parameter in [`FieldInterests`].
    pub struct EnableInterest;

    impl<T: Clone + Debug + Eq + Hash + PartialEq + 'static> MaybeInterest<T> for EnableInterest {
        const ENABLED: bool = true;
        type Ty = T;

        fn try_from_fidl<F: TryInto<T, Error: Into<anyhow::Error>>>(
            fidl: Option<F>,
        ) -> Result<Self::Ty, anyhow::Error> {
            fidl.map(|f| f.try_into().map_err(Into::into))
                .unwrap_or_else(|| Err(anyhow::anyhow!("missing field with registered interest")))
        }

        fn into_fidl<F: From<T>>(value: Self::Ty) -> Option<F> {
            Some(value.into())
        }
    }

    /// Disabled interest in a FIDL field.
    ///
    /// Use as a type parameter in [`FieldInterests`].
    pub struct DisableInterest;
    impl<T> MaybeInterest<T> for DisableInterest {
        const ENABLED: bool = false;

        type Ty = NoInterest;

        fn try_from_fidl<F: TryInto<T, Error: Into<anyhow::Error>>>(
            fidl: Option<F>,
        ) -> Result<Self::Ty, anyhow::Error> {
            match fidl {
                Some(_) => Err(anyhow::anyhow!("unexpected set field with no registered interest")),
                None => Ok(NoInterest),
            }
        }

        fn into_fidl<F: From<T>>(_value: Self::Ty) -> Option<F> {
            None
        }
    }

    /// A handy alias to shorten the signature of a type derived from
    /// [`MaybeInterest`] based on [`FieldSpec`].
    pub(super) type FromInterest<I, T> =
        <<T as FieldSpec>::Interest<I> as MaybeInterest<<T as FieldSpec>::Present>>::Ty;

    /// Parameterizes interest fields.
    ///
    /// This trait allows a common converter implementation for the FIDL table
    /// validation structure and unifies the schema of how interest fields
    /// behave.
    pub trait FieldSpec {
        /// Extracts the interest type from [`FieldInterests`].
        type Interest<I: FieldInterests>: MaybeInterest<Self::Present>;

        /// The FIDL representation of the field.
        type Fidl: From<Self::Present>;

        /// The validated representation of the field when interest is
        /// expressed.
        type Present: TryFrom<Self::Fidl, Error: Into<anyhow::Error>>;

        /// The field name in the originating struct. This helps generate better
        /// error messages.
        const FIELD_NAME: &'static str;
    }

    pub struct InterestConverter<I, P>(PhantomData<(I, P)>);

    impl<I, P> fidl_table_validation::Converter for InterestConverter<I, P>
    where
        I: FieldInterests,
        P: FieldSpec,
    {
        type Fidl = Option<P::Fidl>;
        type Validated = <P::Interest<I> as MaybeInterest<P::Present>>::Ty;
        type Error = anyhow::Error;

        fn try_from_fidl(value: Self::Fidl) -> std::result::Result<Self::Validated, Self::Error> {
            <P::Interest<I> as MaybeInterest<_>>::try_from_fidl(value).context(P::FIELD_NAME)
        }

        fn from_validated(validated: Self::Validated) -> Self::Fidl {
            <P::Interest<I> as MaybeInterest<_>>::into_fidl(validated)
        }
    }

    pub struct ValidUntilInterest;

    impl FieldSpec for ValidUntilInterest {
        type Interest<I: FieldInterests> = I::ValidUntil;
        type Fidl = zx_types::zx_time_t;
        type Present = PositiveMonotonicInstant;
        const FIELD_NAME: &'static str = "valid_until";
    }

    pub struct PreferredLifetimeInfoInterest;

    impl FieldSpec for PreferredLifetimeInfoInterest {
        type Interest<I: FieldInterests> = I::PreferredLifetimeInfo;
        type Fidl = fnet_interfaces::PreferredLifetimeInfo;
        type Present = PreferredLifetimeInfo;
        const FIELD_NAME: &'static str = "preferred_lifetime_info";
    }
}
pub use interest::{DisableInterest, EnableInterest, FieldInterests};
use interest::{
    FromInterest, InterestConverter, MaybeInterest, PreferredLifetimeInfoInterest,
    ValidUntilInterest,
};

/// A marker for interest in all optional fields.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AllInterest;
impl FieldInterests for AllInterest {
    type PreferredLifetimeInfo = EnableInterest;
    type ValidUntil = EnableInterest;
}

/// A marker for the default interest options as defined by the interfaces
/// watcher API.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DefaultInterest;
impl FieldInterests for DefaultInterest {
    type PreferredLifetimeInfo = DisableInterest;
    type ValidUntil = DisableInterest;
}

/// An [`fnet_interfaces::Event`] tagged with the interest parameters that
/// created it.
#[derive(Derivative)]
#[derivative(Clone(bound = ""), Debug(bound = ""), Eq(bound = ""), PartialEq(bound = ""))]
pub struct EventWithInterest<I: FieldInterests> {
    event: fnet_interfaces::Event,
    #[derivative(Debug = "ignore")]
    _marker: PhantomData<I>,
}

impl<I: FieldInterests> EventWithInterest<I> {
    /// Creates a new `EventWithInterest` with the provided event.
    ///
    /// Note that this type exists to steer proper usage of this crate. Creating
    /// `EventWithInterest` with arbitrary interests is potentially dangerous if
    /// the combination of field expectations don't match what was used to
    /// create the watcher.
    pub fn new(event: fnet_interfaces::Event) -> Self {
        Self { event, _marker: PhantomData }
    }

    /// Retrieves the internal event.
    pub fn into_inner(self) -> fnet_interfaces::Event {
        self.event
    }

    /// Borrows the internal event.
    pub fn inner(&self) -> &fnet_interfaces::Event {
        &self.event
    }
}

impl<I: FieldInterests> From<fnet_interfaces::Event> for EventWithInterest<I> {
    fn from(value: fnet_interfaces::Event) -> Self {
        Self::new(value)
    }
}

impl<I: FieldInterests> From<EventWithInterest<I>> for fnet_interfaces::Event {
    fn from(value: EventWithInterest<I>) -> Self {
        value.into_inner()
    }
}

struct PortIdentityKoidConverter;

impl From<Never> for PropertiesValidationError {
    fn from(value: Never) -> Self {
        match value {}
    }
}

impl fidl_table_validation::Converter for PortIdentityKoidConverter {
    type Fidl = Option<u64>;
    type Validated = Option<PortIdentityKoid>;
    type Error = Never;

    fn try_from_fidl(value: Self::Fidl) -> Result<Self::Validated, Self::Error> {
        Ok(value.map(|value| PortIdentityKoid::from_raw(value)))
    }

    fn from_validated(validated: Self::Validated) -> Self::Fidl {
        validated.map(|validated| validated.raw_koid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use flex_fuchsia_net as fnet;
    use futures::FutureExt as _;
    use futures::task::Poll;
    use net_declare::fidl_subnet;
    use std::cell::RefCell;
    use std::convert::TryInto as _;
    use std::pin::Pin;
    use std::rc::Rc;
    use test_case::test_case;

    fn fidl_properties(id: u64) -> fnet_interfaces::Properties {
        fnet_interfaces::Properties {
            id: Some(id),
            name: Some("test1".to_string()),
            port_class: Some(fnet_interfaces::PortClass::Loopback(fnet_interfaces::Empty {})),
            online: Some(false),
            has_default_ipv4_route: Some(false),
            has_default_ipv6_route: Some(false),
            addresses: Some(vec![fidl_address(ADDR, zx_types::ZX_TIME_INFINITE)]),
            ..Default::default()
        }
    }

    fn validated_properties(id: u64) -> PropertiesAndState<(), AllInterest> {
        PropertiesAndState {
            properties: fidl_properties(id).try_into().expect("failed to validate FIDL Properties"),
            state: (),
        }
    }

    fn properties_delta(id: u64) -> fnet_interfaces::Properties {
        fnet_interfaces::Properties {
            id: Some(id),
            name: None,
            port_class: None,
            online: Some(true),
            has_default_ipv4_route: Some(true),
            has_default_ipv6_route: Some(true),
            addresses: Some(vec![fidl_address(ADDR2, zx_types::ZX_TIME_INFINITE)]),
            ..Default::default()
        }
    }

    fn fidl_properties_after_change(id: u64) -> fnet_interfaces::Properties {
        fnet_interfaces::Properties {
            id: Some(id),
            name: Some("test1".to_string()),
            port_class: Some(fnet_interfaces::PortClass::Loopback(fnet_interfaces::Empty {})),
            online: Some(true),
            has_default_ipv4_route: Some(true),
            has_default_ipv6_route: Some(true),
            addresses: Some(vec![fidl_address(ADDR2, zx_types::ZX_TIME_INFINITE)]),
            ..Default::default()
        }
    }

    fn validated_properties_after_change(id: u64) -> PropertiesAndState<(), AllInterest> {
        PropertiesAndState {
            properties: fidl_properties_after_change(id)
                .try_into()
                .expect("failed to validate FIDL Properties"),
            state: (),
        }
    }

    fn fidl_address(
        addr: fnet::Subnet,
        valid_until: zx_types::zx_time_t,
    ) -> fnet_interfaces::Address {
        fnet_interfaces::Address {
            addr: Some(addr),
            valid_until: Some(valid_until.try_into().unwrap()),
            assignment_state: Some(fnet_interfaces::AddressAssignmentState::Assigned),
            preferred_lifetime_info: Some(PreferredLifetimeInfo::preferred_forever().into()),
            __source_breaking: Default::default(),
        }
    }

    const ID: u64 = 1;
    const ID2: u64 = 2;
    const ADDR: fnet::Subnet = fidl_subnet!("1.2.3.4/24");
    const ADDR2: fnet::Subnet = fidl_subnet!("5.6.7.8/24");

    #[test_case(
        &mut std::iter::once((ID, validated_properties(ID))).collect::<HashMap<_, _>>();
        "hashmap"
    )]
    #[test_case(&mut InterfaceState::Known(validated_properties(ID)); "interface_state_known")]
    #[test_case(&mut validated_properties(ID); "properties")]
    fn test_duplicate_error(state: &mut impl Update<(), Interest = AllInterest>) {
        assert_matches::assert_matches!(
            state.update(fnet_interfaces::Event::Added(fidl_properties(ID)).into()),
            Err(UpdateError::DuplicateAdded(added)) if added == fidl_properties(ID)
        );
        assert_matches::assert_matches!(
            state.update(fnet_interfaces::Event::Existing(fidl_properties(ID)).into()),
            Err(UpdateError::DuplicateExisting(existing)) if existing == fidl_properties(ID)
        );
    }

    #[test_case(&mut HashMap::<u64, _>::new(); "hashmap")]
    #[test_case(&mut InterfaceState::Unknown(ID); "interface_state_unknown")]
    fn test_unknown_error(state: &mut impl Update<(), Interest = AllInterest>) {
        let unknown =
            fnet_interfaces::Properties { id: Some(ID), online: Some(true), ..Default::default() };
        assert_matches::assert_matches!(
            state.update(fnet_interfaces::Event::Changed(unknown.clone()).into()),
            Err(UpdateError::UnknownChanged(changed)) if changed == unknown
        );
        assert_matches::assert_matches!(
            state.update(fnet_interfaces::Event::Removed(ID).into()),
            Err(UpdateError::UnknownRemoved(id)) if id == ID
        );
    }

    #[test_case(&mut InterfaceState::Known(validated_properties(ID)); "interface_state_known")]
    #[test_case(&mut validated_properties(ID); "properties")]
    fn test_removed_error(state: &mut impl Update<()>) {
        assert_matches::assert_matches!(
            state.update(fnet_interfaces::Event::Removed(ID).into()),
            Err(UpdateError::Removed)
        );
    }

    #[test_case(&mut HashMap::<u64, _>::new(); "hashmap")]
    #[test_case(&mut InterfaceState::Unknown(ID); "interface_state_unknown")]
    #[test_case(&mut InterfaceState::Known(validated_properties(ID)); "interface_state_known")]
    #[test_case(&mut validated_properties(ID); "properties")]
    fn test_missing_id_error(state: &mut impl Update<(), Interest = AllInterest>) {
        let missing_id = fnet_interfaces::Properties { online: Some(true), ..Default::default() };
        assert_matches::assert_matches!(
            state.update(fnet_interfaces::Event::Changed(missing_id.clone()).into()),
            Err(UpdateError::MissingId(properties)) if properties == missing_id
        );
    }

    #[test_case(
        &mut std::iter::once((ID, validated_properties(ID))).collect::<HashMap<_, _>>();
        "hashmap"
    )]
    #[test_case(&mut InterfaceState::Known(validated_properties(ID)); "interface_state_known")]
    #[test_case(&mut validated_properties(ID); "properties")]
    fn test_empty_change_error(state: &mut impl Update<()>) {
        let empty_change = fnet_interfaces::Properties { id: Some(ID), ..Default::default() };
        let net_zero_change =
            fnet_interfaces::Properties { name: None, port_class: None, ..fidl_properties(ID) };
        assert_matches::assert_matches!(
            state.update(fnet_interfaces::Event::Changed(empty_change.clone()).into()),
            Err(UpdateError::EmptyChange(properties)) if properties == empty_change
        );
        assert_matches::assert_matches!(
            state.update(fnet_interfaces::Event::Changed(net_zero_change.clone()).into()),
            Err(UpdateError::EmptyChange(properties)) if properties == net_zero_change
        );
    }

    #[test_case(
        &mut std::iter::once((ID, validated_properties(ID))).collect::<HashMap<_, _>>();
        "hashmap"
    )]
    #[test_case(&mut InterfaceState::Known(validated_properties(ID)); "interface_state_known")]
    #[test_case(&mut validated_properties(ID); "properties")]
    fn test_update_changed_result(state: &mut impl Update<(), Interest = AllInterest>) {
        let want_previous = fnet_interfaces::Properties {
            online: Some(false),
            has_default_ipv4_route: Some(false),
            has_default_ipv6_route: Some(false),
            addresses: Some(vec![fidl_address(ADDR, zx_types::ZX_TIME_INFINITE)]),
            ..Default::default()
        };
        assert_matches::assert_matches!(
            state.update(fnet_interfaces::Event::Changed(properties_delta(ID).clone()).into()),
            Ok(UpdateResult::Changed { previous, current, state: _ }) => {
                assert_eq!(previous, want_previous);
                let PropertiesAndState { properties, state: () } =
                    validated_properties_after_change(ID);
                assert_eq!(*current, properties);
            }
        );
    }

    #[derive(Derivative)]
    #[derivative(Clone(bound = ""))]
    struct EventStream<I: FieldInterests>(Rc<RefCell<Vec<fnet_interfaces::Event>>>, PhantomData<I>);

    impl<I: FieldInterests> Stream for EventStream<I> {
        type Item = Result<EventWithInterest<I>, fidl::Error>;

        fn poll_next(
            self: Pin<&mut Self>,
            _cx: &mut futures::task::Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            let EventStream(events_vec, _marker) = &*self;
            if events_vec.borrow().is_empty() {
                Poll::Ready(None)
            } else {
                Poll::Ready(Some(Ok(EventWithInterest::new(events_vec.borrow_mut().remove(0)))))
            }
        }
    }

    fn test_event_stream<I: FieldInterests>() -> EventStream<I> {
        EventStream(
            Rc::new(RefCell::new(vec![
                fnet_interfaces::Event::Existing(fidl_properties(ID)),
                fnet_interfaces::Event::Idle(fnet_interfaces::Empty {}),
                fnet_interfaces::Event::Added(fidl_properties(ID2)),
                fnet_interfaces::Event::Changed(properties_delta(ID)),
                fnet_interfaces::Event::Changed(properties_delta(ID2)),
                fnet_interfaces::Event::Removed(ID),
                fnet_interfaces::Event::Removed(ID2),
            ])),
            PhantomData,
        )
    }

    #[test]
    fn test_wait_one_interface() {
        let event_stream = test_event_stream::<AllInterest>();
        let mut state = InterfaceState::Unknown(ID);
        for want in &[validated_properties(ID), validated_properties_after_change(ID)] {
            wait_interface_with_id(event_stream.clone(), &mut state, |got| {
                assert_eq!(got, want);
                Some(())
            })
            .now_or_never()
            .expect("wait_interface_with_id did not complete immediately")
            .expect("wait_interface_with_id error");
            assert_matches!(state, InterfaceState::Known(ref got) if got == want);
        }
    }

    fn test_wait_interface<'a, B>(state: &mut B, want_states: impl IntoIterator<Item = &'a B>)
    where
        B: 'a + Update<()> + Clone + Debug + std::cmp::PartialEq,
    {
        let event_stream = test_event_stream::<B::Interest>();
        for want in want_states.into_iter() {
            wait_interface(event_stream.clone(), state, |got| {
                assert_eq!(got, want);
                Some(())
            })
            .now_or_never()
            .expect("wait_interface did not complete immediately")
            .expect("wait_interface error");
            assert_eq!(state, want);
        }
    }

    #[test]
    fn test_wait_interface_hashmap() {
        test_wait_interface(
            &mut HashMap::new(),
            &[
                std::iter::once((ID, validated_properties(ID))).collect::<HashMap<_, _>>(),
                [(ID, validated_properties(ID)), (ID2, validated_properties(ID2))]
                    .iter()
                    .cloned()
                    .collect::<HashMap<_, _>>(),
                [(ID, validated_properties_after_change(ID)), (ID2, validated_properties(ID2))]
                    .iter()
                    .cloned()
                    .collect::<HashMap<_, _>>(),
                [
                    (ID, validated_properties_after_change(ID)),
                    (ID2, validated_properties_after_change(ID2)),
                ]
                .iter()
                .cloned()
                .collect::<HashMap<_, _>>(),
                std::iter::once((ID2, validated_properties_after_change(ID2)))
                    .collect::<HashMap<_, _>>(),
                HashMap::new(),
            ],
        );
    }

    #[test]
    fn test_wait_interface_interface_state() {
        test_wait_interface(
            &mut InterfaceState::Unknown(ID),
            &[
                InterfaceState::Known(validated_properties(ID)),
                InterfaceState::Known(validated_properties_after_change(ID)),
            ],
        );
    }

    const ID_NON_EXISTENT: u64 = 0xffffffff;
    #[test_case(
        InterfaceState::Unknown(ID_NON_EXISTENT),
        InterfaceState::Unknown(ID_NON_EXISTENT);
        "interface_state_unknown_different_id"
    )]
    #[test_case(
        InterfaceState::Unknown(ID),
        InterfaceState::Known(validated_properties(ID));
        "interface_state_unknown")]
    #[test_case(
        HashMap::new(),
        [(ID, validated_properties(ID)), (ID2, validated_properties(ID2))]
            .iter()
            .cloned()
            .collect::<HashMap<_, _>>();
        "hashmap"
    )]
    fn test_existing<B>(state: B, want: B)
    where
        B: Update<(), Interest = AllInterest> + Debug + std::cmp::PartialEq,
    {
        let events = [
            fnet_interfaces::Event::Existing(fidl_properties(ID)),
            fnet_interfaces::Event::Existing(fidl_properties(ID2)),
            fnet_interfaces::Event::Idle(fnet_interfaces::Empty {}),
        ];
        let event_stream =
            futures::stream::iter(events.iter().cloned().map(|e| Ok(EventWithInterest::new(e))));
        assert_eq!(
            existing(event_stream, state)
                .now_or_never()
                .expect("existing did not complete immediately")
                .expect("existing returned error"),
            want,
        );
    }

    #[test]
    fn positive_instant() {
        assert_eq!(PositiveMonotonicInstant::from_nanos(-1), None);
        assert_eq!(PositiveMonotonicInstant::from_nanos(0), None);
        assert_eq!(PositiveMonotonicInstant::from_nanos(1), Some(PositiveMonotonicInstant(1)));
    }
}
