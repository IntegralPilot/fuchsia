// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::DeviceMode;
use crate::device::kobject::{Bus, Class, Device, DeviceMetadata};
use crate::fs::sysfs::get_sysfs;
use crate::task::Kernel;
use crate::vfs::pseudo::simple_directory::{SimpleDirectory, SimpleDirectoryMutator};
use crate::vfs::{FileSystemHandle, FsStr, FsString};
use std::sync::{Arc, OnceLock};

/// The owner of all the KObjects in sysfs.
///
/// This structure holds strong references to the KObjects that are visible in sysfs. These
/// objects are organized into hierarchies that make it easier to implement sysfs.
pub struct KObjectStore {
    /// The root of the sysfs hierarchy.
    pub root: Arc<SimpleDirectory>,

    /// The sysfs filesystem in which the KObjects are stored.
    fs: OnceLock<FileSystemHandle>,
}

impl KObjectStore {
    pub fn init(&self, kernel: &Kernel) {
        self.fs.set(get_sysfs(kernel)).unwrap();
    }

    fn fs(&self) -> &FileSystemHandle {
        self.fs.get().expect("sysfs should be initialized")
    }

    /// The virtual bus kobject where all virtual and pseudo devices are stored.
    pub fn virtual_bus(&self) -> Bus {
        let name: FsString = b"virtual".into();
        let dir = self.ensure_dir(&[b"devices".into(), name.as_ref()]);
        Bus::new(name, dir, None)
    }

    /// The device class used for virtual block devices.
    pub fn virtual_block_class(&self) -> Class {
        self.get_or_create_class("block".into(), self.virtual_bus())
    }

    /// The device class used for virtual thermal devices.
    pub fn virtual_thermal_class(&self) -> Class {
        self.get_or_create_class("thermal".into(), self.virtual_bus())
    }

    /// The device class used for virtual graphics devices.
    pub fn graphics_class(&self) -> Class {
        self.get_or_create_class("graphics".into(), self.virtual_bus())
    }

    /// The device class used for virtual input devices.
    pub fn input_class(&self) -> Class {
        self.get_or_create_class("input".into(), self.virtual_bus())
    }

    /// The device class used for virtual mem devices.
    pub fn mem_class(&self) -> Class {
        self.get_or_create_class("mem".into(), self.virtual_bus())
    }

    /// The device class used for virtual net devices.
    pub fn net_class(&self) -> Class {
        self.get_or_create_class("net".into(), self.virtual_bus())
    }

    /// The device class used for virtual misc devices.
    pub fn misc_class(&self) -> Class {
        self.get_or_create_class("misc".into(), self.virtual_bus())
    }

    /// The device class used for virtual tty devices.
    pub fn tty_class(&self) -> Class {
        self.get_or_create_class("tty".into(), self.virtual_bus())
    }

    /// The device class used for virtual dma_heap devices.
    pub fn dma_heap_class(&self) -> Class {
        self.get_or_create_class("dma_heap".into(), self.virtual_bus())
    }

    /// An incorrect device class.
    ///
    /// This class exposes the name "starnix" to userspace, which is incorrect. Instead, devices
    /// should use a class that represents their usage rather than their implementation.
    ///
    /// This class exists because a number of devices incorrectly use this class. We should fix
    /// those devices to report their proper class.
    pub fn starnix_class(&self) -> Class {
        self.get_or_create_class("starnix".into(), self.virtual_bus())
    }

    /// Real-time clock class.
    ///
    /// Becomes `/sys/class/rtc/...`.
    pub fn rtc_class(&self) -> Class {
        self.get_or_create_class("rtc".into(), self.virtual_bus())
    }

    fn ensure_dir(&self, path: &[&FsStr]) -> Arc<SimpleDirectory> {
        let fs = self.fs();
        let mut dir = self.root.clone();
        for component in path {
            dir = dir.subdir(fs, component, 0o755);
        }
        dir
    }

    fn edit_dir(&self, path: &[&FsStr], callback: impl FnOnce(&SimpleDirectoryMutator)) {
        let dir = self.ensure_dir(path);
        let mutator = SimpleDirectoryMutator::new(self.fs().clone(), dir);
        callback(&mutator);
    }

    /// Get a bus by name.
    ///
    /// If the bus does not exist, this function will create it.
    pub fn get_or_create_bus(&self, name: &FsStr) -> Bus {
        let name = name.to_owned();
        let dir = self.ensure_dir(&[b"devices".into(), name.as_ref()]);
        let collection = self.ensure_dir(&[b"bus".into(), name.as_ref(), b"devices".into()]);
        Bus::new(name, dir, Some(collection))
    }

    /// Get a class by name.
    ///
    /// If the bus does not exist, this function will create it.
    pub fn get_or_create_class(&self, name: &FsStr, bus: Bus) -> Class {
        let name = name.to_owned();
        let dir = bus.dir.subdir(self.fs(), name.as_ref(), 0o755);
        let collection = self.ensure_dir(&[b"class".into(), name.as_ref()]);
        Class::new(name, dir, bus, collection)
    }

    pub fn class_with_dir(
        &self,
        name: &FsStr,
        bus: Bus,
        build_collection: impl FnOnce(&SimpleDirectoryMutator),
    ) -> Class {
        let class = self.get_or_create_class(name, bus);
        let mutator = SimpleDirectoryMutator::new(self.fs().clone(), class.collection.clone());
        build_collection(&mutator);
        class
    }

    fn block(&self, callback: impl FnOnce(&SimpleDirectoryMutator)) {
        self.edit_dir(&[b"block".into()], callback);
    }

    fn dev_block(&self, callback: impl FnOnce(&SimpleDirectoryMutator)) {
        self.edit_dir(&[b"dev".into(), b"block".into()], callback);
    }

    fn dev_char(&self, callback: impl FnOnce(&SimpleDirectoryMutator)) {
        self.edit_dir(&[b"dev".into(), b"char".into()], callback);
    }

    /// Create a device and add that device to the store.
    ///
    /// Rather than use this function directly, you should register your device with the
    /// `DeviceRegistry`. The `DeviceRegistry` will create the KObject for the device as
    /// part of the registration process.
    ///
    /// If you create the device yourself, userspace will not be able to instantiate the
    /// device because the `DeviceId` will not be registered with the `DeviceRegistry`.
    pub(super) fn create_device(
        &self,
        name: &FsStr,
        metadata: Option<DeviceMetadata>,
        class: Class,
        build_directory: impl FnOnce(&Device, &SimpleDirectoryMutator),
    ) -> Device {
        let device = Device::new(name.to_owned(), class, metadata);
        device.class.dir.edit(self.fs(), |dir| {
            dir.subdir2(name, 0o755, |dir| {
                build_directory(&device, dir);
            });
        });
        self.add(&device);
        device
    }

    fn add(&self, device: &Device) {
        let class = &device.class;
        let name = device.name.as_ref();

        let up_device = device.path_from_depth(1);
        let up_up_device = device.path_from_depth(2);

        // Insert the newly created device into various views.
        class.collection.edit(self.fs(), |dir| {
            dir.symlink(name, up_up_device.as_ref());
        });

        if let Some(metadata) = &device.metadata {
            let device_number = FsString::from(metadata.devt.to_string());
            match metadata.mode {
                DeviceMode::Block => {
                    self.block(|dir| dir.symlink(name, up_device.as_ref()));
                    self.dev_block(|dir| {
                        dir.symlink(device_number.as_ref(), up_up_device.as_ref());
                    });
                }
                DeviceMode::Char => {
                    self.dev_char(|dir| {
                        dir.symlink(device_number.as_ref(), up_up_device.as_ref());
                    });
                }
            }
        }

        if let Some(bus_collection) = &class.bus.collection {
            bus_collection.edit(self.fs(), |dir| {
                dir.symlink(name, device.path_from_depth(3).as_ref());
            });
        }
    }

    /// Destroy a device.
    ///
    /// This function removes the KObject for the device from the store.
    ///
    /// Most clients hold weak references to KObjects, which means those references will become
    /// invalid shortly after this function is called.
    pub(super) fn remove(&self, device: &Device) {
        let name = device.name.as_ref();
        // Remove the device from its views in the reverse order in which it was added.
        if let Some(bus_collection) = &device.class.bus.collection {
            bus_collection.remove(name);
        }
        if let Some(metadata) = &device.metadata {
            let device_number: FsString = metadata.devt.to_string().into();
            match metadata.mode {
                DeviceMode::Block => {
                    self.dev_block(|dir| dir.remove(device_number.as_ref()));
                    self.block(|dir| dir.remove(name));
                }
                DeviceMode::Char => {
                    self.dev_char(|dir| dir.remove(device_number.as_ref()));
                }
            }
        }
        device.class.collection.remove(name);
        // Finally, remove the device from the object store.
        device.class.dir.remove(name);
    }
}

impl Default for KObjectStore {
    fn default() -> Self {
        Self { root: SimpleDirectory::new(), fs: OnceLock::new() }
    }
}
