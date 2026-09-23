// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::kobject::Device;
use crate::task::Kernel;
use crate::task::waiter::WaitQueue;
use crate::vfs::{FsStr, FsString};
use starnix_sync::{LockDepMutex, NetstackDevicesLock};
use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Debug)]
pub struct NetstackDevice {
    pub device: Device,
    pub interface_id: NonZeroU64,
}

/// Keeps track of network devices and their [`Device`].
#[derive(Default)]
pub struct NetstackDevices {
    /// The known netstack devices.
    devices: LockDepMutex<BTreeMap<FsString, NetstackDevice>, NetstackDevicesLock>,
    /// `NetstackDevices` listens for events from the netlink worker, which
    /// is initialized lazily and asynchronously. This waits for at least
    /// the `Idle` event is observed on the interface watcher.
    pub initialized_and_wq: (AtomicBool, WaitQueue),
}

impl NetstackDevices {
    pub fn add_device(&self, kernel: &Kernel, name: &FsStr, interface_id: NonZeroU64) {
        let mut devices = self.devices.lock();
        let device = kernel.device_registry.add_net_device(name);
        devices.insert(name.into(), NetstackDevice { device, interface_id });
    }

    pub fn remove_device(&self, kernel: &Kernel, name: &FsStr) {
        let mut devices = self.devices.lock();
        if let Some(NetstackDevice { device, interface_id: _ }) = devices.remove(name) {
            kernel.device_registry.remove_net_device(device);
        }
    }

    pub fn get_device(&self, name: &FsStr) -> Option<NetstackDevice> {
        let devices = self.devices.lock();
        devices.get(name).cloned()
    }

    pub fn snapshot_devices(&self) -> Vec<(FsString, NetstackDevice)> {
        let devices = self.devices.lock();
        devices.iter().map(|(name, device)| (name.clone(), device.clone())).collect()
    }
}
