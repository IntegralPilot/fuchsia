// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::constants::LEGACY_FVM_TYPE_GUID;
use crate::device::{Device, DeviceTag, Parent};
use crate::environment::Environment;
use anyhow::Error;
use async_trait::async_trait;
use fs_management::FVM_TYPE_GUID;
use fs_management::format::DiskFormat;
use fs_management::format::constants::{
    ALL_SYSTEM_PARTITION_LABELS, FVM_PARTITION_LABEL, SUPER_AND_USERDATA_PARTITION_LABEL,
    SUPER_PARTITION_LABEL,
};

mod config_matcher;
pub use config_matcher::get_config_matchers;

#[async_trait]
pub trait Matcher: Send {
    fn matcher_name(&self) -> &str;

    /// Tries to match this device against this matcher. Matching should be infallible.
    async fn match_device(&self, device: &mut dyn Device) -> bool;

    /// Process this device as the format this matcher is for. This is called when this matcher
    /// returns true during matching. This step is fallible - if a device matched a matcher, but
    /// then this step fails, we stop matching and bubble up the error. The matcher may return a
    /// `DeviceTag` which will be used to register the device with the environment.
    async fn process_device(
        &mut self,
        device: &mut dyn Device,
        env: &mut dyn Environment,
    ) -> Result<Option<DeviceTag>, Error>;
}

pub struct Matchers {
    matchers: Vec<Box<dyn Matcher>>,
}

impl Matchers {
    /// Create a new set of matchers. This essentially describes the expected partition layout for
    /// a device.
    #[cfg(test)]
    pub fn new(config: &fshost_config::Config) -> Self {
        Self::new_with_extra_matchers(config, Vec::new())
    }

    /// The extra matchers here get added _almost_ at the end - the publisher matcher is always
    /// last since it catches anything that isn't matched by anything else.
    /// TODO(https://fxbug.dev/417772609): There is likely a better way to do this, but I think the
    /// exact strategy will be informed by how test usage of semantic labels turns out. It's
    /// possible we will want to add these matchers dynamically for tests, which will change how
    /// this is done, otherwise it should probably all be wrapped up in a unified config struct.
    pub fn new_with_extra_matchers(
        config: &fshost_config::Config,
        mut extra_matchers: Vec<Box<dyn Matcher>>,
    ) -> Self {
        // NB: Order is important here!
        // Generally speaking, we want to have more specific matchers first, and more general
        // matchers later.  For example, the GptMatcher needs to come after most others because it
        // will bind to *any* non-removable device, but will only bind once.  It will in turn
        // publish more devices, which will be matched by our other matchers.
        let mut matchers = Vec::<Box<dyn Matcher>>::new();

        // Match the system container.
        // On a regular system, we'll mount the container and its inner volumes.
        // On recovery systems, there might be a ramdisk container as well as an on-disk container.
        // We will mount the ramdisk container and its volumes, but we will only mount the on-disk
        // container (which allows enumerating volumes) and will not mount its volumes.
        if config.fxfs_blob {
            matchers
                .push(Box::new(FxblobMatcher::new(config.ramdisk_image, config.provision_fxfs)));
            if config.ramdisk_image {
                matchers.push(Box::new(FxblobOnRecoveryMatcher::new()));
            }
        } else {
            matchers.push(Box::new(FvmMatcher::new(config.ramdisk_image)));
            if config.ramdisk_image {
                matchers.push(Box::new(FvmOnRecoveryMatcher::new()));
            }
        }

        // Match the primary GPT.
        if config.gpt {
            if !config.gpt_all {
                matchers.push(Box::new(SystemGptMatcher::new()));
            } else {
                matchers.push(Box::new(GptAllMatcher::new()))
            }
        }

        matchers.append(&mut extra_matchers);

        matchers.push(Box::new(PublisherMatcher::new()));

        Matchers { matchers }
    }

    /// Using the set of configured matchers, match and process a block device.
    /// Returns whether the device was matched or not.
    pub async fn match_device(
        &mut self,
        mut device: Box<dyn Device>,
        env: &mut dyn Environment,
    ) -> Result<bool, Error> {
        // Ramdisks created by fshost can appear in multiple locations.  Only process the first one.
        if let Some(path) = env.registered_devices().get_topological_path(DeviceTag::Ramdisk) {
            let topological_path = device.topological_path();
            if topological_path == path {
                // Exact match, ignore duplicates.
                return Ok(false);
            } else if topological_path.starts_with(&path) {
                // Mark any children of the ramdisk as the fshost ramdisk too.
                device.set_fshost_ramdisk(true);
            }
        }

        for (_, m) in self.matchers.iter_mut().enumerate() {
            if m.match_device(device.as_mut()).await {
                log::info!(
                    matcher:% = m.matcher_name(),
                    path:% = device.path();
                    "Matched device"
                );
                let mut tag = m.process_device(device.as_mut(), env).await?;
                // Tag the first Ramdisk device so that it's retained; the ramdisk will be detached
                // if it's dropped.
                if device.is_fshost_ramdisk() {
                    assert!(tag.is_none());
                    tag = Some(DeviceTag::Ramdisk);
                }
                if let Some(tag) = tag {
                    env.registered_devices().register_device(tag, device);
                    log::info!("Registering device {tag:?}");
                }
                return Ok(true);
            }
        }
        Ok(false)
    }
}

struct PublisherMatcher {
    block_index: u32,
}

impl PublisherMatcher {
    fn new() -> Self {
        PublisherMatcher { block_index: 0 }
    }
}

#[async_trait]
impl Matcher for PublisherMatcher {
    fn matcher_name(&self) -> &str {
        "Publisher"
    }

    async fn match_device(&self, device: &mut dyn Device) -> bool {
        device.parent() == Parent::Dev && !device.is_nand()
    }

    async fn process_device(
        &mut self,
        device: &mut dyn Device,
        env: &mut dyn Environment,
    ) -> Result<Option<DeviceTag>, Error> {
        // TODO(https://fxbug.dev/417773172): we should move the paver to use configuration for
        // what specific block devices to export instead of exporting everything. Once we do, this
        // will go away. Until then, we use an incrementing index because the name doesn't matter.
        // It's kind of confusing to have it not match the debug directory but it's fine.
        let index = format!("{:03}", self.block_index);
        log::info!("Publishing {} to /block/{}", device.path(), index);
        env.publish_device(device, &index)?;
        self.block_index += 1;
        Ok(None)
    }
}

// Matches against an Fxfs-based system container with a blob and data volume.
struct FxblobMatcher {
    // True if this partition is required to exist on a ramdisk.
    ramdisk_required: bool,
    // Because this matcher binds to the system Fxfs component, we can only match on it once.
    // TODO(https://fxbug.dev/42079130): Can we be more precise here, e.g. give the matcher an
    // expected device path based on system configuration?
    already_matched: bool,
    // True if this device may require provisioning of Fxfs first (only applies the device with
    // partition label SUPER_AND_USERDATA_PARTITION_LABEL).
    provision_fxfs: bool,
}

impl FxblobMatcher {
    fn new(ramdisk_required: bool, provision_fxfs: bool) -> Self {
        Self { ramdisk_required, already_matched: false, provision_fxfs }
    }

    async fn mount_fxblob_and_blob_volume(
        &mut self,
        device: &mut dyn Device,
        env: &mut dyn Environment,
    ) -> Result<(), Error> {
        env.mount_fxblob(device).await?;
        env.mount_blob_volume().await
    }
}

#[async_trait]
impl Matcher for FxblobMatcher {
    fn matcher_name(&self) -> &str {
        "Fxblob"
    }
    async fn match_device(&self, device: &mut dyn Device) -> bool {
        if self.already_matched || device.is_removable().await {
            return false;
        }
        if self.ramdisk_required && !device.is_fshost_ramdisk() {
            return false;
        }
        match device.partition_label().await {
            Ok(label) if !label.is_empty() => {
                // There are a few different labels used depending on the device. If we don't see
                // any of them, this isn't the right partition.
                // TODO(https://fxbug.dev/344018917): Use another mechanism to keep
                // track of partition labels.
                if !ALL_SYSTEM_PARTITION_LABELS.contains(&label) {
                    return false;
                }
                // If device is labelled with SUPER_AND_USERDATA_PARTITION_LABEL, we treat this as
                // an Fxfs-based system container with a blob and data volume. It may need to be
                // provisioned with Fxfs first (which will be handled in `self.process_device(..)`).
                if self.provision_fxfs && label == SUPER_AND_USERDATA_PARTITION_LABEL {
                    return true;
                }
            }
            // If there is an error getting the partition label, or if the label is empty, it might
            // be because this device doesn't support labels (like if it's directly on a raw disk in
            // an emulator).  Continue with content sniffing.
            _ => (),
        }
        // TODO(https://fxbug.dev/438621914): Consider removing content sniffing when we've already
        // checked the labels.
        device.content_format().await.ok() == Some(DiskFormat::Fxfs)
    }

    async fn process_device(
        &mut self,
        device: &mut dyn Device,
        env: &mut dyn Environment,
    ) -> Result<Option<DeviceTag>, Error> {
        self.already_matched = true;

        if device.content_format().await.ok() == Some(DiskFormat::Fxfs) {
            if let Err(err) = self.mount_fxblob_and_blob_volume(device, env).await {
                if self.provision_fxfs {
                    log::info!(err:?; "Expected Fxfs but failed to mount. Provisioning Fxfs.");
                    env.provision_fxfs(device).await?;
                    // TODO(https://fxbug.dev/393194713): Consider recovery when we fail.
                    self.mount_fxblob_and_blob_volume(device, env).await?;
                } else {
                    return Err(err);
                }
            }
        } else {
            if self.provision_fxfs {
                log::info!("Provisioning Fxfs.");
                env.provision_fxfs(device).await?;
            }
            self.mount_fxblob_and_blob_volume(device, env).await?;
        }

        env.mount_data_volume().await?;
        Ok(None)
    }
}

// Matches against the FVM partition and explicitly mounts the data and blob partitions.
// Fails if the blob partition doesn't exist. Creates the data partition if it doesn't
// already exist.
struct FvmMatcher {
    // True if this partition is required to exist on a ramdisk.
    ramdisk_required: bool,

    // Set to true if we already matched a partition. It doesn't make sense to try and match
    // multiple main system partitions.
    already_matched: bool,
}

impl FvmMatcher {
    fn new(ramdisk_required: bool) -> Self {
        Self { ramdisk_required, already_matched: false }
    }

    async fn bind_fvm_component(
        &mut self,
        device: &mut dyn Device,
        env: &mut dyn Environment,
    ) -> Result<(), Error> {
        env.mount_fvm(device).await?;
        env.mount_blob_volume().await?;
        env.mount_data_volume().await?;
        Ok(())
    }
}

#[async_trait]
impl Matcher for FvmMatcher {
    fn matcher_name(&self) -> &str {
        "Fvm"
    }

    async fn match_device(&self, device: &mut dyn Device) -> bool {
        if self.already_matched || device.is_removable().await {
            return false;
        }
        if self.ramdisk_required && !device.is_fshost_ramdisk() {
            return false;
        }
        // Legacy devices have a wide range of FVM labels, so the safest thing to do is to look for
        // the FVM by content sniffing.  These legacy devices can't repair the FVM if it is corrupt
        // anyways.
        device.content_format().await.ok() == Some(DiskFormat::Fvm)
    }

    async fn process_device(
        &mut self,
        device: &mut dyn Device,
        env: &mut dyn Environment,
    ) -> Result<Option<DeviceTag>, Error> {
        self.bind_fvm_component(device, env).await?;
        // Once we have matched and processed the main system partitions, fuse this matcher so we
        // don't match any other partitions.
        self.already_matched = true;
        Ok(None)
    }
}

/// When gpt_all is enabled, this is the main gpt matcher. Gpt is launched or bound on any
/// partition that has the gpt disk format. The partitions in the table are snooped to see if any
/// match a heuristic set of partition labels, and the first gpt that does is registered as the
/// "system" gpt as well, which will serve it to the paver on the partition service.
///
/// Unlike the SystemGptMatcher, this matcher doesn't attempt to tag a partition as the system
/// partition that doesn't already have a gpt disk format. This means the fidl call for
/// InitSystemPartitionTables won't work if the gpt isn't formatted or doesn't contain the right
/// partition to be tagged, but that's okay because if there are multiple gpt devices, we don't
/// know which would be the right one to initialize.
struct GptAllMatcher {
    system_gpt_path: Option<String>,
}

impl GptAllMatcher {
    fn new() -> Self {
        Self { system_gpt_path: None }
    }
}

#[async_trait]
impl Matcher for GptAllMatcher {
    fn matcher_name(&self) -> &str {
        "GptAll"
    }

    async fn match_device(&self, device: &mut dyn Device) -> bool {
        if device.is_nand() || device.is_fshost_ramdisk() || device.is_removable().await {
            return false;
        }
        device.content_format().await.ok() == Some(DiskFormat::Gpt)
    }

    async fn process_device(
        &mut self,
        device: &mut dyn Device,
        env: &mut dyn Environment,
    ) -> Result<Option<DeviceTag>, Error> {
        let (gpt, partitions) = env.launch_and_enumerate_gpt_component(device).await?;
        // TODO(https://fxbug.dev/344018917): remove this heuristic in favor of
        // assembly-level label configuration options.
        if self.system_gpt_path.is_none()
            && partitions.iter().any(|p| {
                p.label == SUPER_PARTITION_LABEL
                    || p.label == SUPER_AND_USERDATA_PARTITION_LABEL
                    || (p.label == FVM_PARTITION_LABEL && p.type_guid == FVM_TYPE_GUID)
                    || p.type_guid == LEGACY_FVM_TYPE_GUID
            })
        {
            self.system_gpt_path = Some(device.topological_path().to_string());
            env.register_system_gpt(device, gpt)?;
            Ok(Some(DeviceTag::SystemPartitionTable))
        } else {
            env.register_filesystem(gpt);
            Ok(None)
        }
    }
}

/// Matches the system GPT partition, which is expected to be on a non-removable disk.
struct SystemGptMatcher {
    device_path: Option<String>,
}

impl SystemGptMatcher {
    fn new() -> Self {
        Self { device_path: None }
    }
}

#[async_trait]
impl Matcher for SystemGptMatcher {
    fn matcher_name(&self) -> &str {
        "SystemGpt"
    }

    async fn match_device(&self, device: &mut dyn Device) -> bool {
        if self.device_path.is_some() {
            return false;
        }
        if device.is_nand() || device.is_fshost_ramdisk() || device.is_removable().await {
            return false;
        }
        // If the partition has a type GUID, that implies it's inside a partition table so it can't
        // be the system partition table itself.  This is intended to deal with devices like vim3
        // which use the sdmmc partition table and the GPT is one of several sdmmc partitions, but
        // it is reported as having an empty type GUID.
        // NOTE: This is a bit of a hack.  The right way will likely involve a per-board
        // configuration which tells fshost which block device the system partition table is
        // expected to reside in.  For now, this works.
        const EMPTY_GUID: [u8; 16] = [0; 16];
        let has_type_guid = device.partition_type().await.unwrap_or(&EMPTY_GUID) != &EMPTY_GUID;
        // Match the first non-removable device which isn't inside a partition table itself.
        !has_type_guid
    }

    async fn process_device(
        &mut self,
        device: &mut dyn Device,
        env: &mut dyn Environment,
    ) -> Result<Option<DeviceTag>, Error> {
        let (gpt, _) = env.launch_and_enumerate_gpt_component(device).await?;
        env.register_system_gpt(device, gpt)?;

        self.device_path = Some(device.topological_path().to_string());

        Ok(Some(DeviceTag::SystemPartitionTable))
    }
}

// Matches against the first Fxblob partition that isn't the ram-disk
struct FxblobOnRecoveryMatcher {
    // Because this matcher binds to the system Fxfs component, we can only match on it once.
    // TODO(https://fxbug.dev/42079130): Can we be more precise here, e.g. give the matcher an
    // expected device path based on system configuration?
    already_matched: bool,
}

impl FxblobOnRecoveryMatcher {
    fn new() -> Self {
        Self { already_matched: false }
    }
}

#[async_trait]
impl Matcher for FxblobOnRecoveryMatcher {
    fn matcher_name(&self) -> &str {
        "FxblobOnRecovery"
    }

    async fn match_device(&self, device: &mut dyn Device) -> bool {
        if self.already_matched || device.is_fshost_ramdisk() || device.is_removable().await {
            return false;
        }

        // We only check the partition label and not the content format, because in recovery, the
        // partition might not have any data on it yet (the legacy paver might be about to write to
        // it).
        match device.partition_label().await {
            // There are a few different labels used depending on the device. If we don't see any of
            // them, this isn't the right partition.
            // TODO(https://fxbug.dev/344018917): Use another mechanism to keep track of partition
            // labels.
            Ok(label) if ALL_SYSTEM_PARTITION_LABELS.contains(&label) => true,
            _ => false,
        }
    }

    async fn process_device(
        &mut self,
        _device: &mut dyn Device,
        _env: &mut dyn Environment,
    ) -> Result<Option<DeviceTag>, Error> {
        self.already_matched = true;
        Ok(Some(DeviceTag::SystemContainerOnRecovery))
    }
}

// Matches against the first FVM partition that isn't the ram-disk.  Doesn't bind any volumes.
struct FvmOnRecoveryMatcher {
    // Because this matcher binds to the system FVM, we only match on it once.
    // TODO(https://fxbug.dev/42079130): Can we be more precise here, e.g. give the matcher an
    // expected device path based on system configuration?
    already_matched: bool,
}

impl FvmOnRecoveryMatcher {
    fn new() -> Self {
        Self { already_matched: false }
    }
}

#[async_trait]
impl Matcher for FvmOnRecoveryMatcher {
    fn matcher_name(&self) -> &str {
        "FvmOnRecovery"
    }

    async fn match_device(&self, device: &mut dyn Device) -> bool {
        if self.already_matched || device.is_fshost_ramdisk() || device.is_removable().await {
            return false;
        }

        // Legacy devices have a wide range of FVM labels, so the safest thing to do is to look for
        // the FVM by content sniffing.  These legacy devices can't repair the FVM if it is corrupt
        // anyways.
        if device.content_format().await.ok() == Some(DiskFormat::Fvm) {
            return true;
        }
        // The FVM might be corrupt.  As a fallback, attempt to match on label.
        match device.partition_label().await {
            // There are a few different labels used depending on the device. If we don't see any of
            // them, this isn't the right partition.
            // TODO(https://fxbug.dev/344018917): Use another mechanism to keep track of partition
            // labels.
            Ok(label) if ALL_SYSTEM_PARTITION_LABELS.contains(&label) => true,
            _ => false,
        }
    }

    async fn process_device(
        &mut self,
        _device: &mut dyn Device,
        _env: &mut dyn Environment,
    ) -> Result<Option<DeviceTag>, Error> {
        self.already_matched = true;
        Ok(Some(DeviceTag::SystemContainerOnRecovery))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Device, DiskFormat, Environment, FvmMatcher, FxblobMatcher, GptAllMatcher, Matcher,
        Matchers, SystemGptMatcher,
    };
    use crate::config::default_test_config;
    use crate::device::constants::LEGACY_FVM_TYPE_GUID;
    use crate::device::{DeviceTag, Parent, RegisteredDevices};
    use crate::environment::{Filesystem, PartitionInfo, SinglePublisher};
    use crate::matcher::config_matcher::ConfigMatcher;
    use anyhow::{Error, anyhow};
    use async_trait::async_trait;
    use fidl_fuchsia_storage_block::{BlockInfo, BlockProxy, DeviceFlag};
    use fs_management::FVM_TYPE_GUID;
    use fs_management::filesystem::{BlockConnector, ServingMultiVolumeFilesystem};
    use fs_management::format::constants::{
        ALL_SYSTEM_PARTITION_LABELS, FUCHSIA_FVM_PARTITION_LABEL, FVM_PARTITION_LABEL,
    };
    use fuchsia_sync::Mutex;
    use std::sync::Arc;

    #[derive(Clone)]
    struct MockDevice {
        content_format: DiskFormat,
        topological_path: String,
        partition_label: Option<String>,
        partition_type: Option<[u8; 16]>,
        is_fshost_ramdisk: bool,
        is_removable: bool,
        parent: Parent,
    }

    impl MockDevice {
        fn new() -> Self {
            MockDevice {
                content_format: DiskFormat::Unknown,
                topological_path: "mock_device".to_string(),
                partition_label: None,
                partition_type: None,
                is_fshost_ramdisk: false,
                is_removable: false,
                // Default to system partition table here mostly so we don't trip the publisher
                // matcher unless we are testing it.
                parent: Parent::SystemPartitionTable,
            }
        }
        fn set_content_format(mut self, format: DiskFormat) -> Self {
            self.content_format = format;
            self
        }
        fn set_topological_path(mut self, path: impl ToString) -> Self {
            self.topological_path = path.to_string().into();
            self
        }
        fn set_partition_label(mut self, label: impl ToString) -> Self {
            self.partition_label = Some(label.to_string());
            self
        }
        fn set_partition_type(mut self, type_guid: [u8; 16]) -> Self {
            self.partition_type = Some(type_guid);
            self
        }
        fn set_fshost_ramdisk(mut self) -> Self {
            self.is_fshost_ramdisk = true;
            self
        }
        fn set_is_removable(mut self, is_removable: bool) -> Self {
            self.is_removable = is_removable;
            self
        }
        fn set_parent(mut self, parent: Parent) -> Self {
            self.parent = parent;
            self
        }
    }

    #[async_trait]
    impl Device for MockDevice {
        async fn get_block_info(&self) -> Result<BlockInfo, Error> {
            Ok(BlockInfo {
                block_count: 0,
                block_size: 0,
                max_transfer_size: 0,
                flags: DeviceFlag::empty(),
            })
        }
        fn is_nand(&self) -> bool {
            false
        }
        async fn is_removable(&self) -> bool {
            self.is_removable
        }
        async fn content_format(&mut self) -> Result<DiskFormat, Error> {
            Ok(self.content_format)
        }
        fn topological_path(&self) -> &str {
            &self.topological_path
        }
        fn path(&self) -> &str {
            &self.topological_path
        }
        fn source(&self) -> &str {
            &self.topological_path
        }
        fn parent(&self) -> Parent {
            self.parent
        }
        async fn partition_label(&mut self) -> Result<&str, Error> {
            match self.partition_label.as_ref() {
                Some(label) => Ok(label.as_str()),
                None => Err(anyhow!("partition label not set")),
            }
        }
        async fn partition_type(&mut self) -> Result<&[u8; 16], Error> {
            self.partition_type.as_ref().ok_or_else(|| anyhow!("partition type not set"))
        }
        fn block_connector(&self) -> Result<Box<dyn BlockConnector>, Error> {
            unreachable!()
        }
        fn block_proxy(&self) -> Result<BlockProxy, Error> {
            unreachable!()
        }
        fn is_fshost_ramdisk(&self) -> bool {
            self.is_fshost_ramdisk
        }
        fn set_fshost_ramdisk(&mut self, v: bool) {
            self.is_fshost_ramdisk = v;
        }
    }

    #[derive(Default)]
    struct MockEnv {
        expect_mount_fxblob: Mutex<bool>,
        expect_mount_fvm: Mutex<bool>,
        expect_mount_blob_volume: Mutex<bool>,
        expect_mount_data_volume: Mutex<bool>,
        expect_publish_device: Mutex<Option<String>>,
        expect_launch_and_enumerate_gpt_component: Mutex<Option<Vec<PartitionInfo>>>,
        expect_register_system_gpt: Mutex<bool>,
        expect_register_filesystem: Mutex<bool>,
        registered_devices: Arc<RegisteredDevices>,
    }

    impl MockEnv {
        fn new() -> Self {
            MockEnv::default()
        }
        fn expect_mount_fxblob(mut self) -> Self {
            *self.expect_mount_fxblob.get_mut() = true;
            self
        }
        fn expect_mount_fvm(mut self) -> Self {
            *self.expect_mount_fvm.get_mut() = true;
            self
        }
        fn expect_mount_blob_volume(mut self) -> Self {
            *self.expect_mount_blob_volume.get_mut() = true;
            self
        }
        fn expect_mount_data_volume(mut self) -> Self {
            *self.expect_mount_data_volume.get_mut() = true;
            self
        }
        fn expect_launch_and_enumerate_gpt_component(
            mut self,
            partitions: Vec<PartitionInfo>,
        ) -> Self {
            *self.expect_launch_and_enumerate_gpt_component.get_mut() = Some(partitions);
            self
        }

        fn expect_register_system_gpt(mut self) -> Self {
            *self.expect_register_system_gpt.get_mut() = true;
            self
        }

        fn expect_register_filesystem(mut self) -> Self {
            *self.expect_register_filesystem.get_mut() = true;
            self
        }

        fn expect_publish_device(mut self, expected_alias: impl ToString) -> Self {
            *self.expect_publish_device.get_mut() = Some(expected_alias.to_string());
            self
        }
    }

    #[async_trait]
    impl Environment for MockEnv {
        async fn launch_and_enumerate_gpt_component(
            &mut self,
            _device: &mut dyn Device,
        ) -> Result<(Filesystem, Vec<PartitionInfo>), Error> {
            let partitions = self
                .expect_launch_and_enumerate_gpt_component
                .lock()
                .take()
                .expect("Unexpected call to attach_driver");
            Ok((Filesystem::Queue(vec![]), partitions))
        }

        fn register_system_gpt(
            &mut self,
            _device: &dyn Device,
            _gpt: Filesystem,
        ) -> Result<(), Error> {
            assert_eq!(
                std::mem::take(&mut *self.expect_register_system_gpt.lock()),
                true,
                "Unexpected call to register_system_gpt"
            );
            Ok(())
        }

        fn partition_manager_exposed_dir(
            &mut self,
        ) -> Result<fidl_fuchsia_io::DirectoryProxy, Error> {
            unreachable!()
        }

        async fn mount_fxblob(&mut self, _device: &mut dyn Device) -> Result<(), Error> {
            assert_eq!(
                std::mem::take(&mut *self.expect_mount_fxblob.lock()),
                true,
                "Unexpected call to mount_fxblob"
            );
            Ok(())
        }

        async fn mount_fvm(&mut self, _device: &mut dyn Device) -> Result<(), Error> {
            assert_eq!(
                std::mem::take(&mut *self.expect_mount_fvm.lock()),
                true,
                "Unexpected call to mount_fvm"
            );
            Ok(())
        }

        async fn mount_blob_volume(&mut self) -> Result<(), Error> {
            assert_eq!(
                std::mem::take(&mut *self.expect_mount_blob_volume.lock()),
                true,
                "Unexpected call to mount_blob_volume"
            );
            Ok(())
        }

        async fn mount_data_volume(&mut self) -> Result<(), Error> {
            assert_eq!(
                std::mem::take(&mut *self.expect_mount_data_volume.lock()),
                true,
                "Unexpected call to mount_data_volume"
            );
            Ok(())
        }

        async fn shutdown(&mut self) -> Result<(), Error> {
            unreachable!();
        }

        fn publish_device(&mut self, _device: &mut dyn Device, name: &str) -> Result<(), Error> {
            assert_eq!(
                name,
                self.expect_publish_device
                    .lock()
                    .take()
                    .expect("Unexpected call to publish_device")
            );
            Ok(())
        }

        fn publish_device_to_debug_block(
            &mut self,
            _device: &dyn Device,
            _name: &str,
        ) -> Result<(), Error> {
            unreachable!();
        }

        fn registered_devices(&self) -> &Arc<RegisteredDevices> {
            &self.registered_devices
        }

        fn get_container(&mut self) -> Option<&mut ServingMultiVolumeFilesystem> {
            unreachable!();
        }

        fn register_filesystem(&mut self, _filesystem: Filesystem) {
            assert_eq!(
                std::mem::take(&mut *self.expect_register_filesystem.lock()),
                true,
                "Unexpected call to register_filesystem"
            );
        }
        async fn provision_fxfs(&mut self, _device: &mut dyn Device) -> Result<(), Error> {
            Ok(())
        }

        async fn shred_data_online(&mut self) -> Result<(), Error> {
            Ok(())
        }

        fn report_corruption(&self, _format: &str, _error: &Error) {
            unreachable!()
        }
    }

    impl Drop for MockEnv {
        fn drop(&mut self) {
            assert!(!*self.expect_mount_fxblob.lock());
            assert!(!*self.expect_mount_fvm.lock());
            assert!(!*self.expect_mount_blob_volume.lock());
            assert!(!*self.expect_mount_data_volume.lock());
            assert!(self.expect_publish_device.get_mut().is_none());
            assert!(self.expect_launch_and_enumerate_gpt_component.get_mut().is_none());
            assert!(!*self.expect_register_system_gpt.lock());
            assert!(!*self.expect_register_filesystem.lock());
        }
    }

    #[fuchsia::test]
    async fn test_fxblob_matcher() {
        let mut matchers = Matchers::new(&fshost_config::Config {
            fxfs_blob: true,
            data_filesystem_format: "fxfs".to_string(),
            gpt: false,
            ..default_test_config()
        });

        // A device with the wrong label should fail.
        assert!(
            !matchers
                .match_device(
                    Box::new(
                        MockDevice::new()
                            .set_content_format(DiskFormat::Fxfs)
                            .set_partition_label("wrong_label")
                    ),
                    &mut MockEnv::new()
                )
                .await
                .expect("match_device failed")
        );

        // A device with the right label should succeed.
        assert!(
            matchers
                .match_device(
                    Box::new(
                        MockDevice::new()
                            .set_content_format(DiskFormat::Fxfs)
                            .set_partition_label(FVM_PARTITION_LABEL)
                    ),
                    &mut MockEnv::new()
                        .expect_mount_fxblob()
                        .expect_mount_blob_volume()
                        .expect_mount_data_volume()
                )
                .await
                .expect("match_device failed")
        );

        // We should only be able to match Fxblob once.
        assert!(
            !matchers
                .match_device(
                    Box::new(
                        MockDevice::new()
                            .set_content_format(DiskFormat::Fxfs)
                            .set_partition_label(FVM_PARTITION_LABEL)
                    ),
                    &mut MockEnv::new(),
                )
                .await
                .expect("match_device failed")
        );
    }

    #[fuchsia::test]
    async fn test_fvm_component_matcher() {
        let new_matchers = || {
            Matchers::new(&fshost_config::Config {
                data_filesystem_format: "minfs".to_string(),
                gpt: false,
                ..default_test_config()
            })
        };

        let mut matchers = new_matchers();

        // A device with the right label but the wrong content format should fail.
        assert!(
            !matchers
                .match_device(
                    Box::new(MockDevice::new().set_partition_label(FVM_PARTITION_LABEL)),
                    &mut MockEnv::new()
                )
                .await
                .expect("match_device failed")
        );

        // A device with the wrong label but the correct content format should succeed.
        assert!(
            matchers
                .match_device(
                    Box::new(
                        MockDevice::new()
                            .set_content_format(DiskFormat::Fvm)
                            .set_partition_label("wrong_label")
                    ),
                    &mut MockEnv::new()
                        .expect_mount_fvm()
                        .expect_mount_blob_volume()
                        .expect_mount_data_volume()
                )
                .await
                .expect("match_device failed")
        );

        // We should only be able to match Fvm once.
        assert!(
            !matchers
                .match_device(
                    Box::new(
                        MockDevice::new()
                            .set_content_format(DiskFormat::Fvm)
                            .set_partition_label(FVM_PARTITION_LABEL)
                    ),
                    &mut MockEnv::new(),
                )
                .await
                .expect("match_device failed")
        );
    }

    #[fuchsia::test]
    async fn test_fxblob_matcher_alternate_label() {
        let mut matchers = Matchers::new(&fshost_config::Config {
            fxfs_blob: true,
            data_filesystem_format: "fxfs".to_string(),
            gpt: false,
            ..default_test_config()
        });

        // A device with the wrong label should fail.
        assert!(
            !matchers
                .match_device(
                    Box::new(
                        MockDevice::new()
                            .set_content_format(DiskFormat::Fxfs)
                            .set_partition_label("wrong_label")
                    ),
                    &mut MockEnv::new()
                )
                .await
                .expect("match_device failed")
        );

        // A device with the right label should succeed.
        assert!(
            matchers
                .match_device(
                    Box::new(
                        MockDevice::new()
                            .set_content_format(DiskFormat::Fxfs)
                            .set_partition_label(FUCHSIA_FVM_PARTITION_LABEL)
                    ),
                    &mut MockEnv::new()
                        .expect_mount_fxblob()
                        .expect_mount_blob_volume()
                        .expect_mount_data_volume()
                )
                .await
                .expect("match_device failed")
        );

        // We should only be able to match Fxblob once.
        assert!(
            !matchers
                .match_device(
                    Box::new(
                        MockDevice::new()
                            .set_content_format(DiskFormat::Fxfs)
                            .set_partition_label(FUCHSIA_FVM_PARTITION_LABEL)
                    ),
                    &mut MockEnv::new(),
                )
                .await
                .expect("match_device failed")
        );
    }

    #[fuchsia::test]
    async fn test_fxblob_matcher_without_label() {
        let mut matchers = Matchers::new(&fshost_config::Config {
            fxfs_blob: true,
            data_filesystem_format: "fxfs".to_string(),
            gpt: false,
            ..default_test_config()
        });

        assert!(
            matchers
                .match_device(
                    Box::new(MockDevice::new().set_content_format(DiskFormat::Fxfs)),
                    &mut MockEnv::new()
                        .expect_mount_fxblob()
                        .expect_mount_blob_volume()
                        .expect_mount_data_volume()
                )
                .await
                .expect("match_device failed")
        );

        // We should only be able to match Fxblob once.
        assert!(
            !matchers
                .match_device(
                    Box::new(MockDevice::new().set_content_format(DiskFormat::Fxfs)),
                    &mut MockEnv::new(),
                )
                .await
                .expect("match_device failed")
        );
    }

    #[fuchsia::test]
    async fn test_system_gpt_matcher() {
        let mut matchers = Matchers::new(&default_test_config());

        // Don't match devices with a partition type, since they are likely nested in another GPT.
        assert!(
            !matchers
                .match_device(
                    Box::new(
                        MockDevice::new()
                            .set_content_format(DiskFormat::Gpt)
                            .set_partition_type([1u8; 16])
                    ),
                    &mut MockEnv::new()
                )
                .await
                .expect("match_device failed")
        );

        assert!(
            matchers
                .match_device(
                    Box::new(MockDevice::new()),
                    &mut MockEnv::new()
                        .expect_launch_and_enumerate_gpt_component(Vec::new())
                        .expect_register_system_gpt()
                )
                .await
                .expect("match_device failed")
        );

        // Any future devices shouldn't bind.
        assert!(
            !matchers
                .match_device(
                    Box::new(MockDevice::new().set_content_format(DiskFormat::Gpt)),
                    &mut MockEnv::new()
                )
                .await
                .expect("match_device failed")
        );
    }

    #[fuchsia::test]
    async fn test_gpt_all_matcher_fvm_label() {
        let mut matchers =
            Matchers::new(&fshost_config::Config { gpt_all: true, ..default_test_config() });

        // Without the appropriate partitions inside it, this gpt will not be registered as the
        // system gpt, just registered as a regular filesystem for shutdown.
        assert!(
            matchers
                .match_device(
                    Box::new(MockDevice::new().set_content_format(DiskFormat::Gpt)),
                    &mut MockEnv::new()
                        .expect_launch_and_enumerate_gpt_component(vec![PartitionInfo {
                            label: "test-part".into(),
                            type_guid: [0; 16]
                        }])
                        .expect_register_filesystem()
                )
                .await
                .expect("match_device failed")
        );

        // With a partition labeled "fvm", it is registered as the system gpt.
        assert!(
            matchers
                .match_device(
                    Box::new(MockDevice::new().set_content_format(DiskFormat::Gpt)),
                    &mut MockEnv::new()
                        .expect_launch_and_enumerate_gpt_component(vec![
                            PartitionInfo { label: "boot_a".into(), type_guid: [0; 16] },
                            PartitionInfo { label: "boot_b".into(), type_guid: [0; 16] },
                            PartitionInfo { label: "fvm".into(), type_guid: FVM_TYPE_GUID }
                        ])
                        .expect_register_system_gpt()
                )
                .await
                .expect("match_device failed")
        );

        // Following gpt-formatted devices, even with the recognized partition labels, just get
        // registered normally.
        assert!(
            matchers
                .match_device(
                    Box::new(MockDevice::new().set_content_format(DiskFormat::Gpt)),
                    &mut MockEnv::new()
                        .expect_launch_and_enumerate_gpt_component(vec![
                            PartitionInfo { label: "boot_a".into(), type_guid: [0; 16] },
                            PartitionInfo { label: "boot_b".into(), type_guid: [0; 16] },
                            PartitionInfo { label: "fvm".into(), type_guid: FVM_TYPE_GUID }
                        ])
                        .expect_register_filesystem()
                )
                .await
                .expect("match_device failed")
        );
    }

    #[fuchsia::test]
    async fn test_gpt_all_matcher_super_label() {
        let mut matchers =
            Matchers::new(&fshost_config::Config { gpt_all: true, ..default_test_config() });

        // Without the appropriate partitions inside it, this gpt will not be registered as the
        // system gpt, just registered as a regular filesystem for shutdown.
        assert!(
            matchers
                .match_device(
                    Box::new(MockDevice::new().set_content_format(DiskFormat::Gpt)),
                    &mut MockEnv::new()
                        .expect_launch_and_enumerate_gpt_component(vec![PartitionInfo {
                            label: "test-part".into(),
                            type_guid: [0; 16]
                        }])
                        .expect_register_filesystem()
                )
                .await
                .expect("match_device failed")
        );

        // With a partition labeled "super", it is registered as the system gpt.
        assert!(
            matchers
                .match_device(
                    Box::new(MockDevice::new().set_content_format(DiskFormat::Gpt)),
                    &mut MockEnv::new()
                        .expect_launch_and_enumerate_gpt_component(vec![
                            PartitionInfo { label: "boot_a".into(), type_guid: [0; 16] },
                            PartitionInfo { label: "boot_b".into(), type_guid: [0; 16] },
                            PartitionInfo { label: "super".into(), type_guid: [0; 16] }
                        ])
                        .expect_register_system_gpt()
                )
                .await
                .expect("match_device failed")
        );

        // Following gpt-formatted devices, even with the recognized partition labels, just get
        // registered normally.
        assert!(
            matchers
                .match_device(
                    Box::new(MockDevice::new().set_content_format(DiskFormat::Gpt)),
                    &mut MockEnv::new()
                        .expect_launch_and_enumerate_gpt_component(vec![
                            PartitionInfo { label: "boot_a".into(), type_guid: [0; 16] },
                            PartitionInfo { label: "boot_b".into(), type_guid: [0; 16] },
                            PartitionInfo { label: "super".into(), type_guid: [0; 16] }
                        ])
                        .expect_register_filesystem()
                )
                .await
                .expect("match_device failed")
        );
    }

    #[fuchsia::test]
    async fn test_gpt_all_matcher_legacy_fvm_type_guid() {
        let mut matchers =
            Matchers::new(&fshost_config::Config { gpt_all: true, ..default_test_config() });

        // With a partition with the legacy fvm type guid, it is registered as the system gpt.
        assert!(
            matchers
                .match_device(
                    Box::new(MockDevice::new().set_content_format(DiskFormat::Gpt)),
                    &mut MockEnv::new()
                        .expect_launch_and_enumerate_gpt_component(vec![
                            PartitionInfo { label: "boot_a".into(), type_guid: [0; 16] },
                            PartitionInfo { label: "boot_b".into(), type_guid: [0; 16] },
                            PartitionInfo { label: "fvm".into(), type_guid: LEGACY_FVM_TYPE_GUID }
                        ])
                        .expect_register_system_gpt()
                )
                .await
                .expect("match_device failed")
        );
    }

    #[fuchsia::test]
    async fn test_fxblob_on_recovery_matcher() {
        let mut matchers = Matchers::new(&fshost_config::Config {
            ramdisk_image: true,
            fxfs_blob: true,
            ..default_test_config()
        });

        // The non-ramdisk should match.
        let mut env = MockEnv::new();
        assert!(
            matchers
                .match_device(
                    Box::new(MockDevice::new().set_partition_label(FUCHSIA_FVM_PARTITION_LABEL)),
                    &mut env
                )
                .await
                .expect("match_device failed")
        );

        assert!(
            env.registered_devices
                .get_topological_path(DeviceTag::SystemContainerOnRecovery)
                .is_some()
        );

        let mut env =
            env.expect_mount_fxblob().expect_mount_blob_volume().expect_mount_data_volume();

        // And the ramdisk Fxblob should too.
        assert!(
            matchers
                .match_device(
                    Box::new(
                        MockDevice::new().set_content_format(DiskFormat::Fxfs).set_fshost_ramdisk()
                    ),
                    &mut env
                )
                .await
                .expect("match_device failed")
        );

        assert!(env.registered_devices.get_topological_path(DeviceTag::Ramdisk).is_some());
    }

    #[fuchsia::test]
    async fn test_fvm_on_recovery_matcher() {
        let mut matchers =
            Matchers::new(&fshost_config::Config { ramdisk_image: true, ..default_test_config() });

        // The non-ramdisk should match by content format, but all we expect is the device to be
        // tagged.
        let mut env = MockEnv::new();
        assert!(
            matchers
                .match_device(
                    Box::new(MockDevice::new().set_content_format(DiskFormat::Fvm)),
                    &mut env
                )
                .await
                .expect("match_device failed")
        );

        assert!(
            env.registered_devices
                .get_topological_path(DeviceTag::SystemContainerOnRecovery)
                .is_some()
        );

        let mut env = env.expect_mount_fvm().expect_mount_blob_volume().expect_mount_data_volume();

        // The ramdisk FVM should still be able to match.
        assert!(
            matchers
                .match_device(
                    Box::new(
                        MockDevice::new().set_content_format(DiskFormat::Fvm).set_fshost_ramdisk()
                    ),
                    &mut env
                )
                .await
                .expect("match_device failed")
        );

        assert!(env.registered_devices.get_topological_path(DeviceTag::Ramdisk).is_some());

        // The non-ramdisk FVM should be able to match on label as well.
        for label in ALL_SYSTEM_PARTITION_LABELS {
            let mut env = MockEnv::new();
            matchers = Matchers::new(&fshost_config::Config {
                ramdisk_image: true,
                ..default_test_config()
            });
            assert!(
                matchers
                    .match_device(Box::new(MockDevice::new().set_partition_label(label)), &mut env)
                    .await
                    .expect("match_device failed")
            );
            assert!(
                env.registered_devices
                    .get_topological_path(DeviceTag::SystemContainerOnRecovery)
                    .is_some()
            );
        }
    }

    #[fuchsia::test]
    async fn test_publisher_matcher() {
        let mut matchers = Matchers::new(&default_test_config());

        // First unmanaged device should match and be published as "000". Devices are unmanaged if
        // the come from Dev, instead of Fshost or SystemPartitionTable.
        let device1 = MockDevice::new()
            .set_topological_path("dev1")
            .set_partition_type([0x01; 16])
            .set_parent(Parent::Dev);
        let mut env1 = MockEnv::new().expect_publish_device("000");
        assert!(
            matchers
                .match_device(Box::new(device1), &mut env1)
                .await
                .expect("match_device failed for device 1")
        );

        // Second unmanaged device should match and be published as "001".
        let device2 = MockDevice::new()
            .set_topological_path("dev2")
            .set_partition_type([0x01; 16])
            .set_parent(Parent::Dev);
        let mut env2 = MockEnv::new().expect_publish_device("001");
        assert!(
            matchers
                .match_device(Box::new(device2), &mut env2)
                .await
                .expect("match_device failed for device 2")
        );

        // A device in the SystemPartitionTable should not match.
        let managed_device = MockDevice::new()
            .set_topological_path("managed")
            .set_partition_type([0x01; 16])
            .set_parent(Parent::SystemPartitionTable);
        let mut env3 = MockEnv::new(); // No expectations
        assert!(
            !matchers
                .match_device(Box::new(managed_device), &mut env3)
                .await
                .expect("match_device failed for managed device")
        );
    }

    struct MockPublisher {
        should_publish: Mutex<bool>,
    }

    impl MockPublisher {
        fn new(should_publish: bool) -> Self {
            Self { should_publish: Mutex::new(should_publish) }
        }
    }

    impl SinglePublisher for MockPublisher {
        fn publish(self: Box<Self>, _device: &dyn Device) -> Result<(), Error> {
            assert_eq!(
                std::mem::take(&mut *self.should_publish.lock()),
                true,
                "Unexpected call to publish"
            );
            Ok(())
        }
    }

    impl Drop for MockPublisher {
        fn drop(&mut self) {
            assert!(!*self.should_publish.lock());
        }
    }

    #[fuchsia::test]
    async fn test_config_matcher() {
        let mut matchers = Matchers::new_with_extra_matchers(
            &default_test_config(),
            vec![ConfigMatcher::new(
                String::from("fts-semantic"),
                String::from("fts-partition"),
                Parent::SystemPartitionTable,
                Box::new(MockPublisher::new(true)),
            )],
        );

        // Wrong label doesn't match.
        let device1 = MockDevice::new()
            .set_topological_path("dev1")
            .set_partition_type([0x01; 16])
            .set_partition_label("not-the-right-label")
            .set_parent(Parent::SystemPartitionTable);
        let mut env1 = MockEnv::new(); // No expectations
        assert!(
            !matchers
                .match_device(Box::new(device1), &mut env1)
                .await
                .expect("match_device failed for device 1")
        );

        // Wrong parent doesn't match.
        let device2 = MockDevice::new()
            .set_topological_path("dev1")
            .set_partition_type([0x01; 16])
            .set_partition_label("fts-partition")
            .set_parent(Parent::Fshost);
        let mut env2 = MockEnv::new(); // No expectations
        assert!(
            !matchers
                .match_device(Box::new(device2), &mut env2)
                .await
                .expect("match_device failed for device 2")
        );

        // Matching device gets published.
        let device3 = MockDevice::new()
            .set_topological_path("dev1")
            .set_partition_type([0x01; 16])
            .set_partition_label("fts-partition")
            .set_parent(Parent::SystemPartitionTable);
        let mut env3 = MockEnv::new(); // No expectations
        assert!(
            matchers
                .match_device(Box::new(device3), &mut env3)
                .await
                .expect("match_device failed for device 2")
        );

        // The matcher is fused after it finds a matching device. No further devices are published.
        let device4 = MockDevice::new()
            .set_topological_path("dev1")
            .set_partition_type([0x01; 16])
            .set_partition_label("fts-partition")
            .set_parent(Parent::SystemPartitionTable);
        let mut env4 = MockEnv::new(); // No expectations
        assert!(
            !matchers
                .match_device(Box::new(device4), &mut env4)
                .await
                .expect("match_device failed for device 2")
        );
    }

    #[fuchsia::test]
    async fn test_system_gpt_ignores_removable_devices() {
        let matcher = SystemGptMatcher::new();
        let mut removable_device = MockDevice::new()
            .set_topological_path("/dev/sys/platform/pci/00:14.0/xhci/usb-bus/001/usb-mass-storage")
            .set_is_removable(true);
        assert!(!matcher.match_device(&mut removable_device).await);

        let mut internal_device = MockDevice::new()
            .set_topological_path("/dev/sys/platform/pci/00:1f.2/nvme")
            .set_is_removable(false);
        assert!(matcher.match_device(&mut internal_device).await);
    }

    #[fuchsia::test]
    async fn test_gpt_all_ignores_removable_devices() {
        let matcher = GptAllMatcher::new();
        let mut removable_device = MockDevice::new()
            .set_content_format(DiskFormat::Gpt)
            .set_topological_path("/dev/sys/platform/pci/00:14.0/xhci/usb-bus/001/usb-mass-storage")
            .set_is_removable(true);
        assert!(!matcher.match_device(&mut removable_device).await);

        let mut internal_device = MockDevice::new()
            .set_content_format(DiskFormat::Gpt)
            .set_topological_path("/dev/sys/platform/pci/00:1f.2/nvme")
            .set_is_removable(false);
        assert!(matcher.match_device(&mut internal_device).await);
    }

    #[fuchsia::test]
    async fn test_fxblob_ignores_removable_devices() {
        let matcher = FxblobMatcher::new(false, false);
        let mut removable_device = MockDevice::new()
            .set_content_format(DiskFormat::Fxfs)
            .set_partition_label("super")
            .set_is_removable(true);
        assert!(!matcher.match_device(&mut removable_device).await);

        let mut internal_device = MockDevice::new()
            .set_content_format(DiskFormat::Fxfs)
            .set_partition_label("super")
            .set_is_removable(false);
        assert!(matcher.match_device(&mut internal_device).await);
    }

    #[fuchsia::test]
    async fn test_fvm_ignores_removable_devices() {
        let matcher = FvmMatcher::new(false);
        let mut removable_device = MockDevice::new()
            .set_content_format(DiskFormat::Fvm)
            .set_partition_label("fvm")
            .set_is_removable(true);
        assert!(!matcher.match_device(&mut removable_device).await);

        let mut internal_device = MockDevice::new()
            .set_content_format(DiskFormat::Fvm)
            .set_partition_label("fvm")
            .set_is_removable(false);
        assert!(matcher.match_device(&mut internal_device).await);
    }
}
