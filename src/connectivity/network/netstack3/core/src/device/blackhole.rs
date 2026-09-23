// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementations of traits defined in foreign modules for the types defined
//! in the blackhole module.

use lock_order::relation::LockBefore;
use lock_order::wrap::LockedWrapperApi;
use net_types::ip::Ip;
use netstack3_base::DeviceIdContext;
use netstack3_device::blackhole::{
    BlackholeDevice, BlackholeDeviceId, BlackholePrimaryDeviceId, BlackholeWeakDeviceId,
};
use netstack3_device::{DeviceCollectionContext, DeviceConfigurationContext};
use netstack3_ip::nud::NudUserConfig;

use crate::{BindingsTypes, CoreCtx};

impl<BT: BindingsTypes, L> DeviceIdContext<BlackholeDevice> for CoreCtx<'_, BT, L> {
    type DeviceId = BlackholeDeviceId<BT>;
    type WeakDeviceId = BlackholeWeakDeviceId<BT>;
}

impl<'a, BT, L> DeviceCollectionContext<BlackholeDevice, BT> for CoreCtx<'a, BT, L>
where
    BT: BindingsTypes,
    L: LockBefore<crate::lock_ordering::DeviceLayerState>,
{
    fn insert(&mut self, device: BlackholePrimaryDeviceId<BT>) {
        let mut devices = self.write_lock::<crate::lock_ordering::DeviceLayerState>();
        let strong = device.clone_strong();
        assert!(devices.blackhole.insert(strong, device).is_none());
    }

    fn remove(&mut self, device: &BlackholeDeviceId<BT>) -> Option<BlackholePrimaryDeviceId<BT>> {
        let mut devices = self.write_lock::<crate::lock_ordering::DeviceLayerState>();
        devices.blackhole.remove(device)
    }
}

impl<'a, BT, L> DeviceConfigurationContext<BlackholeDevice> for CoreCtx<'a, BT, L>
where
    BT: BindingsTypes,
{
    fn with_nud_config<I: Ip, O, F: FnOnce(Option<&NudUserConfig>) -> O>(
        &mut self,
        _device_id: &Self::DeviceId,
        f: F,
    ) -> O {
        // Blackhole doesn't support NUD.
        f(None)
    }

    fn with_nud_config_mut<I: Ip, O, F: FnOnce(Option<&mut NudUserConfig>) -> O>(
        &mut self,
        _device_id: &Self::DeviceId,
        f: F,
    ) -> O {
        // Blackhole doesn't support NUD.
        f(None)
    }
}
