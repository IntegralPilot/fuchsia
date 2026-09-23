// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A blackhole device receives no traffic and drops all traffic sent through it.

use core::convert::Infallible as Never;

use netstack3_base::{ChecksumOffloadSpec, Device, NeverBuffer};

use crate::internal::base::{BlackholeDeviceCounters, DeviceReceiveFrameSpec};
use crate::internal::id::{BaseDeviceId, BasePrimaryDeviceId, BaseWeakDeviceId};
use crate::internal::queue::DeviceBufferSpec;
use crate::internal::queue::tx::TxBufferAllocator;
use crate::internal::state::DeviceStateSpec;

/// A weak device ID identifying a blackhole device.
///
/// This device ID is like [`WeakDeviceId`] but specifically for blackhole
/// devices.
///
/// [`WeakDeviceId`]: crate::device::WeakDeviceId
pub type BlackholeWeakDeviceId<BT> = BaseWeakDeviceId<BlackholeDevice, BT>;

/// A strong device ID identifying a blackhole device.
///
/// This device ID is like [`DeviceId`] but specifically for blackhole devices.
///
/// [`DeviceId`]: crate::device::DeviceId
pub type BlackholeDeviceId<BT> = BaseDeviceId<BlackholeDevice, BT>;

/// The primary reference for a blackhole device.
pub type BlackholePrimaryDeviceId<BT> = BasePrimaryDeviceId<BlackholeDevice, BT>;

/// State for a blackhole device.
pub struct BlackholeDeviceState {}

/// Blackhole device domain.
#[derive(Copy, Clone)]
pub enum BlackholeDevice {}

impl Device for BlackholeDevice {}

impl TxBufferAllocator<NeverBuffer> for () {
    type Error = ();

    fn alloc(&mut self, _len: usize, _qlen: usize) -> Result<NeverBuffer, ()> {
        Err(())
    }
}

impl<BT> DeviceBufferSpec<BT> for BlackholeDevice {
    type TxBuffer = NeverBuffer;
    type TxAllocator = ();
}

impl DeviceStateSpec for BlackholeDevice {
    type State<BT: crate::DeviceLayerTypes> = BlackholeDeviceState;

    type External<BT: crate::DeviceLayerTypes> = BT::BlackholeDeviceState;

    type CreationProperties = ();

    type Counters = BlackholeDeviceCounters;

    type TimerId<D: netstack3_base::WeakDeviceIdentifier> = Never;

    fn new_device_state<
        CC: netstack3_base::CoreTimerContext<Self::TimerId<CC::WeakDeviceId>, BC>
            + netstack3_base::DeviceIdContext<Self>,
        BC: crate::DeviceLayerTypes + netstack3_base::TimerContext,
    >(
        _bindings_ctx: &mut BC,
        _self_id: CC::WeakDeviceId,
        _properties: Self::CreationProperties,
        _tx_allocator: <Self as DeviceBufferSpec<BC>>::TxAllocator,
    ) -> Self::State<BC>
    where
        Self: DeviceBufferSpec<BC>,
    {
        BlackholeDeviceState {}
    }

    const IS_LOOPBACK: bool = false;

    const DEBUG_TYPE: &'static str = "Blackhole";

    fn tx_offload_spec<BT: crate::DeviceLayerTypes>(
        _state: &Self::State<BT>,
    ) -> Option<ChecksumOffloadSpec> {
        None
    }
}

impl DeviceReceiveFrameSpec for BlackholeDevice {
    // Blackhole devices never receive frames from bindings, so make it impossible to
    // instantiate it.
    type FrameMetadata<D> = Never;
}
