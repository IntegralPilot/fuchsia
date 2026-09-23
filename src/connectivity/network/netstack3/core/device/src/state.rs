// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! State maintained by the device layer.

use alloc::sync::Arc;
use core::fmt::Debug;

use lock_order::lock::{OrderedLockAccess, OrderedLockRef};
use net_types::ip::{Ipv4, Ipv6};
use netstack3_base::sync::{RwLock, WeakRc};
use netstack3_base::{
    ChecksumOffloadSpec, CoreTimerContext, Device, DeviceIdContext, Inspectable, TimerContext,
    WeakDeviceIdentifier,
};
use netstack3_ip::RawMetric;
use netstack3_ip::device::{DualStackIpDeviceState, IpDeviceTimerId};

use crate::internal::base::{DeviceCounters, DeviceLayerTypes, OriginTracker};
use crate::internal::queue::DeviceBufferSpec;
use crate::internal::socket::HeldDeviceSockets;

/// Provides the specifications for device state held by [`BaseDeviceId`] in
/// [`BaseDeviceState`].
pub trait DeviceStateSpec: Device + Sized + Send + Sync + 'static {
    /// The device state.
    type State<BT: DeviceLayerTypes>: Send + Sync;
    /// The external (bindings) state.
    type External<BT: DeviceLayerTypes>: Send + Sync;
    /// Properties given to device creation.
    type CreationProperties: Debug;
    /// Device-specific counters.
    type Counters: Inspectable;
    /// The timer identifier required by this device state.
    type TimerId<D: WeakDeviceIdentifier>;

    /// Creates a new device state from the given properties.
    fn new_device_state<
        CC: CoreTimerContext<Self::TimerId<CC::WeakDeviceId>, BC> + DeviceIdContext<Self>,
        BC: DeviceLayerTypes + TimerContext,
    >(
        bindings_ctx: &mut BC,
        self_id: CC::WeakDeviceId,
        properties: Self::CreationProperties,
        tx_allocator: <Self as DeviceBufferSpec<BC>>::TxAllocator,
    ) -> Self::State<BC>
    where
        Self: DeviceBufferSpec<BC>;

    /// Marker for loopback devices.
    const IS_LOOPBACK: bool;
    /// Marker used to print debug information for device identifiers.
    const DEBUG_TYPE: &'static str;

    /// Returns the TX checksum offload specification for the device, if it has one.
    fn tx_offload_spec<BT: DeviceLayerTypes>(
        state: &Self::State<BT>,
    ) -> Option<ChecksumOffloadSpec>;
}

/// Groups state kept by weak device references.
///
/// A weak device reference must be able to carry the bindings identifier
/// infallibly. The `WeakCookie` is kept inside [`BaseDeviceState`] in an `Arc`
/// to group all the information that is cloned out to support weak device
/// references.
pub(crate) struct WeakCookie<T: DeviceStateSpec, BT: DeviceLayerTypes> {
    pub(crate) bindings_id: BT::DeviceIdentifier,
    pub(crate) weak_ref: WeakRc<BaseDeviceState<T, BT>>,
}

pub(crate) struct BaseDeviceState<T: DeviceStateSpec, BT: DeviceLayerTypes> {
    pub(crate) ip: IpLinkDeviceState<T, BT>,
    pub(crate) external_state: T::External<BT>,
    pub(crate) weak_cookie: Arc<WeakCookie<T, BT>>,
}

/// A convenience wrapper around `IpLinkDeviceStateInner` that uses
/// `DeviceStateSpec` to extract the link state type and make type signatures
/// shorter.
pub type IpLinkDeviceState<T, BT> = IpLinkDeviceStateInner<<T as DeviceStateSpec>::State<BT>, BT>;

/// State for a link-device that is also an IP device.
///
/// `D` is the link-specific state.
pub struct IpLinkDeviceStateInner<T, BT: DeviceLayerTypes> {
    /// The device's IP state.
    pub ip: DualStackIpDeviceState<BT>,
    /// The device's link state.
    pub link: T,
    pub(crate) origin: OriginTracker,
    pub(super) sockets: RwLock<HeldDeviceSockets<BT>>,
    /// Common device counters.
    pub counters: DeviceCounters,
}

impl<T, BC: DeviceLayerTypes + TimerContext> IpLinkDeviceStateInner<T, BC> {
    /// Create a new `IpLinkDeviceState` with a link-specific state `link`.
    pub fn new<
        D: WeakDeviceIdentifier,
        CC: CoreTimerContext<IpDeviceTimerId<Ipv6, D, BC>, BC>
            + CoreTimerContext<IpDeviceTimerId<Ipv4, D, BC>, BC>,
    >(
        bindings_ctx: &mut BC,
        device_id: D,
        link: T,
        metric: RawMetric,
        origin: OriginTracker,
    ) -> Self {
        Self {
            ip: DualStackIpDeviceState::new::<D, CC>(bindings_ctx, device_id, metric),
            link,
            origin,
            sockets: RwLock::new(HeldDeviceSockets::default()),
            counters: DeviceCounters::default(),
        }
    }
}

impl<T, BT: DeviceLayerTypes> AsRef<DualStackIpDeviceState<BT>> for IpLinkDeviceStateInner<T, BT> {
    fn as_ref(&self) -> &DualStackIpDeviceState<BT> {
        &self.ip
    }
}

impl<T, BT: DeviceLayerTypes> OrderedLockAccess<HeldDeviceSockets<BT>>
    for IpLinkDeviceStateInner<T, BT>
{
    type Lock = RwLock<HeldDeviceSockets<BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.sockets)
    }
}

/// Context for getting the TX checksum offload specification for a device.
pub trait DeviceTxOffloadSpecContext<D: DeviceStateSpec, BT: DeviceLayerTypes>:
    DeviceIdContext<D>
{
    /// Returns the TX checksum offload specification for `device`, if it has one.
    fn tx_offload_spec(&self, device: &Self::DeviceId) -> Option<ChecksumOffloadSpec>;
}
