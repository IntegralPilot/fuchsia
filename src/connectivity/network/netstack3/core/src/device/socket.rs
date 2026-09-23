// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementations of traits defined in foreign modules for the types defined
//! in the device socket module.

use lock_order::lock::{DelegatedOrderedLockAccess, LockLevelFor};
use lock_order::relation::LockBefore;
use netstack3_device::socket::{
    AllSockets, AnyDeviceSockets, DeviceSocketAccessor, DeviceSocketContext, DeviceSocketId,
    DeviceSockets, HeldSockets, SocketStateAccessor, Target,
};
use netstack3_device::{DeviceId, WeakDeviceId, for_any_device_id};

use crate::context::WrapLockLevel;
use crate::context::prelude::*;
use crate::device::integration;
use crate::{BindingsContext, BindingsTypes, CoreCtx, StackState};

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::AllDeviceSockets>>
    DeviceSocketContext<BC> for CoreCtx<'_, BC, L>
{
    type SocketTablesCoreCtx<'a> =
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::AnyDeviceSockets>>;

    fn with_all_device_sockets<
        F: FnOnce(&AllSockets<Self::WeakDeviceId, BC>, &mut Self::SocketTablesCoreCtx<'_>) -> R,
        R,
    >(
        &mut self,
        cb: F,
    ) -> R {
        let (sockets, mut locked) = self.read_lock_and::<crate::lock_ordering::AllDeviceSockets>();
        let mut locked = locked.cast_locked::<crate::lock_ordering::AnyDeviceSockets>();
        cb(&*sockets, &mut locked)
    }

    fn with_all_device_sockets_mut<F: FnOnce(&mut AllSockets<Self::WeakDeviceId, BC>) -> R, R>(
        &mut self,
        cb: F,
    ) -> R {
        let mut locked = self.write_lock::<crate::lock_ordering::AllDeviceSockets>();
        cb(&mut locked)
    }

    fn with_any_device_sockets<
        F: FnOnce(&AnyDeviceSockets<Self::WeakDeviceId, BC>, &mut Self::SocketTablesCoreCtx<'_>) -> R,
        R,
    >(
        &mut self,
        cb: F,
    ) -> R {
        let (sockets, mut locked) = self.read_lock_and::<crate::lock_ordering::AnyDeviceSockets>();
        cb(&*sockets, &mut locked)
    }

    fn with_any_device_sockets_mut<
        F: FnOnce(
            &mut AnyDeviceSockets<Self::WeakDeviceId, BC>,
            &mut Self::SocketTablesCoreCtx<'_>,
        ) -> R,
        R,
    >(
        &mut self,
        cb: F,
    ) -> R {
        let (mut sockets, mut locked) =
            self.write_lock_and::<crate::lock_ordering::AnyDeviceSockets>();
        cb(&mut *sockets, &mut locked)
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::DeviceSocketState>>
    SocketStateAccessor<BC> for CoreCtx<'_, BC, L>
{
    fn with_socket_state<F: FnOnce(&Target<Self::WeakDeviceId>) -> R, R>(
        &mut self,
        id: &DeviceSocketId<Self::WeakDeviceId, BC>,
        cb: F,
    ) -> R {
        let mut locked = self.adopt(id);
        let guard = locked.lock_with::<crate::lock_ordering::DeviceSocketState, _>(|c| c.right());
        cb(&*guard)
    }

    fn with_socket_state_mut<F: FnOnce(&mut Target<Self::WeakDeviceId>) -> R, R>(
        &mut self,
        id: &DeviceSocketId<Self::WeakDeviceId, BC>,
        cb: F,
    ) -> R {
        let mut locked = self.adopt(id);
        let mut guard =
            locked.lock_with::<crate::lock_ordering::DeviceSocketState, _>(|c| c.right());
        cb(&mut *guard)
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::DeviceSockets>>
    DeviceSocketAccessor<BC> for CoreCtx<'_, BC, L>
{
    type DeviceSocketCoreCtx<'a> =
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::DeviceSockets>>;

    fn with_device_sockets<
        F: FnOnce(&DeviceSockets<Self::WeakDeviceId, BC>, &mut Self::DeviceSocketCoreCtx<'_>) -> R,
        R,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> R {
        for_any_device_id!(
            DeviceId,
            device,
            device => {
                let mut core_ctx_and_resource =
                    integration::device_state_and_core_ctx(self, device);
                let (device_sockets, mut locked) = core_ctx_and_resource
                    .read_lock_with_and::<crate::lock_ordering::DeviceSockets, _>(
                    |c| c.right(),
                );
                cb(&*device_sockets, &mut locked.cast_core_ctx())
            }
        )
    }

    fn with_device_sockets_mut<
        F: FnOnce(&mut DeviceSockets<Self::WeakDeviceId, BC>, &mut Self::DeviceSocketCoreCtx<'_>) -> R,
        R,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> R {
        for_any_device_id!(
            DeviceId,
            device,
            device => {
                let mut core_ctx_and_resource =
                    integration::device_state_and_core_ctx(self, device);
                let (mut device_sockets, mut locked) = core_ctx_and_resource
                    .write_lock_with_and::<crate::lock_ordering::DeviceSockets, _>(
                    |c| c.right(),
                );
                cb(&mut *device_sockets, &mut locked.cast_core_ctx())
            }
        )
    }
}

impl<BT: BindingsTypes> DelegatedOrderedLockAccess<AnyDeviceSockets<WeakDeviceId<BT>, BT>>
    for StackState<BT>
{
    type Inner = HeldSockets<BT>;
    fn delegate_ordered_lock_access(&self) -> &Self::Inner {
        &self.device.shared_sockets
    }
}

impl<BT: BindingsTypes> DelegatedOrderedLockAccess<AllSockets<WeakDeviceId<BT>, BT>>
    for StackState<BT>
{
    type Inner = HeldSockets<BT>;
    fn delegate_ordered_lock_access(&self) -> &Self::Inner {
        &self.device.shared_sockets
    }
}

impl<BT: BindingsTypes> LockLevelFor<StackState<BT>> for crate::lock_ordering::AnyDeviceSockets {
    type Data = AnyDeviceSockets<WeakDeviceId<BT>, BT>;
}

impl<BT: BindingsTypes> LockLevelFor<StackState<BT>> for crate::lock_ordering::AllDeviceSockets {
    type Data = AllSockets<WeakDeviceId<BT>, BT>;
}

impl<BT: BindingsTypes> LockLevelFor<DeviceSocketId<WeakDeviceId<BT>, BT>>
    for crate::lock_ordering::DeviceSocketState
{
    type Data = Target<WeakDeviceId<BT>>;
}
