// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
#[cfg(target_os = "fuchsia")]
use fs_management::filesystem::BlockConnector;

/// Block device configuration options.
pub struct BlockDeviceConfig {
    /// If true, the block device will always be added to an FVM volume.  If the system has an FVM
    /// instance, it'll be added in there, and otherwise a test-only FVM instance will be spawned
    /// and the volume will be added to that instance.
    /// Note that even if this is false, the block device might end up in an FVM volume
    /// (particularly when the system uses FVM).
    pub requires_fvm: bool,

    /// If true, zxcrypt is initialized on top of the block device.
    pub use_zxcrypt: bool,

    /// For non-FVM volumes, this is the size of the volume and is required to be set.
    /// For FVM volumes, this is optional, and if set will pre-size the volume.
    pub volume_size: Option<u64>,
}

/// A trait representing a block device.
pub trait BlockDevice: Send + Sync {
    #[cfg(target_os = "fuchsia")]
    fn connector(&self) -> Box<dyn BlockConnector>;
}

/// A trait for constructing block devices.
#[async_trait]
pub trait BlockDeviceFactory: Send + Sync {
    /// Constructs a new block device.
    async fn create_block_device(&self, config: &BlockDeviceConfig) -> Box<dyn BlockDevice>;
}

/// A BlockDeviceFactory that panics when trying to create a block device. This is useful for
/// benchmarking filesystems that don't need to create a block device.
pub struct PanickingBlockDeviceFactory {}

impl PanickingBlockDeviceFactory {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl BlockDeviceFactory for PanickingBlockDeviceFactory {
    async fn create_block_device(&self, _config: &BlockDeviceConfig) -> Box<dyn BlockDevice> {
        panic!("PanickingBlockDeviceFactory can't create block devices");
    }
}
