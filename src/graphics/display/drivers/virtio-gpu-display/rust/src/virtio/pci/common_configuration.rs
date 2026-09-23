// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(doc)]
use super::capability_type::PciCapabilityType;

#[cfg(doc)]
use crate::virtio::VirtioFeatureBits;

#[cfg(doc)]
use crate::virtio::common::device_status::DeviceStatus;

use mmio::{register, register_block};

register! {
    // TODO(https://fxbug.dev/522840090): Formalize `mmio::register!` guarantees
    // around representation. Add `#[repr(transparent)]` to each register
    // definition if necessary.

    #[register(offset = 0, mode = RW)]
    /// Selects the feature bits accessed by [`DeviceFeaturesWord`].
    ///
    /// Index 0 selects feature bits 0-31. Index 1 selects bits 32-63, etc.
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="device_feature_select"
    pub struct DeviceFeaturesWordIndex(u32);

    #[register(offset = 4, mode = RO)]
    /// Window into the feature bits supported by the device.
    ///
    /// The accessed feature bits depend on [`DeviceFeaturesWordIndex`].
    ///
    /// Read-only. The driver communicates feature support via [`DriverFeaturesWord`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="device_feature"
    pub struct DeviceFeaturesWord(u32);

    #[register(offset = 8, mode = RW)]
    /// Selects the feature bits accessed by [`DriverFeaturesWord`].
    ///
    /// Index 0 selects feature bits 0-31. Index 1 selects bits 32-63, etc.
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="driver_feature_select"
    pub struct DriverFeaturesWordIndex(u32);

    #[register(offset = 12, mode = RW)]
    /// Window into the feature bits reported as supported by the driver.
    ///
    /// The accessed feature bits depend on [`DriverFeaturesWordIndex`].
    ///
    /// The feature bits reported as supported by the driver must be a subset of
    /// the feature bits reported as supported by the device. Said differently,
    /// the driver must not report support for a feature that is not supported
    /// by the device.
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="driver_feature"
    pub struct DriverFeaturesWord(u32);

    #[register(offset = 16, mode = RW)]
    /// The MSI-X vector issued for a configuration change notification.
    ///
    /// Only valid if the MSI-X PCI capability is enabled.
    // @cite(virtio): sec="4.1.5.1.2" title="MSI-X Vector Configuration"
    // @alias(virtio): theirs="config_msix_vector"
    pub struct ConfigurationChangeMsixVector(u16);

    #[register(offset = 18, mode = RO)]
    /// The number of virtqueues implemented by the device.
    ///
    /// The number excludes administration virtqueues.
    ///
    /// Some devices allow drivers to decide which queues get enabled.
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="num_queues"
    pub struct QueueCount(u16);

    // TODO(https://fxbug.dev/522840090): Figure out `mmio::register!` support
    // for unbundling the value newtype definition from the register definition.
    // The underlying type for [`DeviceStatusReg`] should be [`DeviceStatus`]
    // instead of [`u8`].

    #[register(offset = 20, mode = RW)]
    /// Driver-reported status.
    ///
    /// Clearing all the status bits (writing 0) initiates a device reset. The
    /// device reset is complete when reading the field returns 0.
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="device_status"
    pub struct DeviceStatusReg(u8);

    #[register(offset = 21, mode = RO)]
    /// MVCC version for the device configuration data.
    ///
    /// virtio does not guarantee that configuration data is updated atomically,
    /// and instead requires drivers to use a Multiversion concurrency control
    /// (MVCC) protocol when reading the configuration data. The driver is
    /// expected to read this version value before and after reading any data,
    /// and retry the data read if the two version values are different.
    ///
    /// Recommends changing the value on demand, to avoid false
    /// negatives due to wrap-around. So, devices may present an unchanged
    /// version number if there are configuration changes that wouldn't be
    /// observed given the configuration field reads issued by the driver.
    // @cite(virtio): sec="4.1.4.3.1" title="Device Requirements: Common configuration structure layout"
    // @alias(virtio): theirs="config_generation"
    pub struct DeviceConfigurationVersion(u8);

    #[register(offset = 22, mode = RW)]

    /// Selects the virtqueue configured by the virtqueue-specific registers.
    ///
    /// The following registers are impacted by this value:
    /// [`ConfiguredQueueCapacity`], [`ConfiguredQueueMsixVector`],
    /// [`ConfiguredQueueEnabled`], [`ConfiguredQueueNotificationOffset`],
    /// [`ConfiguredQueueDescriptorTableAddress`],
    /// [`ConfiguredQueueDriverAreaAddress`],
    /// [`ConfiguredQueueDeviceAreaAddress`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="queue_select"
    pub struct ConfiguredQueueIndex(u16);

    #[register(offset = 24, mode = RW)]
    /// The number of entries in the queue.
    ///
    /// This value dictates the memory requirements for the virtqueue's data
    /// structures. After reset, the value is set to the maximum value supported
    /// by the device. The driver can lower the value to reduce memory usage.
    /// Setting the value to 0 makes the queue unavailable.
    ///
    /// The configured virtqueue depends on [`ConfiguredQueueIndex`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="queue_size"
    pub struct ConfiguredQueueCapacity(u16);

    #[register(offset = 26, mode = RW)]
    /// The MSI-X vector issued for a virtqueue notification.
    ///
    /// Only valid if the MSI-X PCI capability is enabled.
    ///
    /// The configured virtqueue depends on [`ConfiguredQueueIndex`].
    // @cite(virtio): sec="4.1.5.1.2" title="MSI-X Vector Configuration"
    // @alias(virtio): theirs="queue_msix_vector"
    pub struct ConfiguredQueueMsixVector(u16);

    #[register(offset = 28, mode = RW)]
    /// If false (0), the device will not execute requests added to the virtqueue.
    ///
    /// Valid values are true (1) and false (0).
    ///
    /// The configured virtqueue depends on [`ConfiguredQueueIndex`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="queue_enable"
    pub struct ConfiguredQueueEnabled(u16);

    #[register(offset = 30, mode = RO)]
    /// Points to the virtqueue's driver notification memory.
    ///
    /// The values is used to compute the offset in the memory area described by
    /// the notifications capability.
    ///
    /// The configured virtqueue depends on [`ConfiguredQueueIndex`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="queue_notify_off"
    pub struct ConfiguredQueueNotificationOffset(u16);

    #[register(offset = 32, mode = RW)]
    /// Physical address (bits 0-31) of the first byte in the virtqueue's Descriptor Area.
    ///
    /// The configured virtqueue depends on [`ConfiguredQueueIndex`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="queue_desc"
    pub struct ConfiguredQueueDescriptorTableAddressLow(u32);

    #[register(offset = 36, mode = RW)]
    /// Physical address (bits 32-63) of the first byte in the virtqueue's Descriptor Area.
    ///
    /// The configured virtqueue depends on [`ConfiguredQueueIndex`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="queue_desc"
    pub struct ConfiguredQueueDescriptorTableAddressHigh(u32);

    #[register(offset = 40, mode = RW)]
    /// Physical address (bits 0-31) of the first byte in the virtqueue's Driver Area.
    ///
    /// The configured virtqueue depends on [`ConfiguredQueueIndex`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="queue_driver"
    pub struct ConfiguredQueueDriverAreaAddressLow(u32);

    #[register(offset = 44, mode = RW)]
    /// Physical address (bits 32-63) of the first byte in the virtqueue's Driver Area.
    ///
    /// The configured virtqueue depends on [`ConfiguredQueueIndex`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="queue_driver"
    pub struct ConfiguredQueueDriverAreaAddressHigh(u32);

    #[register(offset = 48, mode = RW)]
    /// Physical address (bits 0-31) of the first byte in the virtqueue's Device Area.
    ///
    /// The configured virtqueue depends on [`ConfiguredQueueIndex`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="queue_device"
    pub struct ConfiguredQueueDeviceAreaAddressLow(u32);

    #[register(offset = 52, mode = RW)]
    /// Physical address (bits 32-63) of the first byte in the virtqueue's Device Area.
    ///
    /// The configured virtqueue depends on [`ConfiguredQueueIndex`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="queue_device"
    pub struct ConfiguredQueueDeviceAreaAddressHigh(u32);

    #[register(offset = 56, mode = RO)]

    /// Device data to be sent with submitted buffer notifications.
    ///
    /// Only valid if [`VirtioFeatureBits::uses_custom_virtqueue_ids`] was
    /// negotiated. In this case, the driver must pass the value into every
    /// issued notification targeting the configured virtqueue.
    ///
    /// The configured virtqueue depends on [`ConfiguredQueueIndex`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="queue_notif_config_data"
    pub struct ConfiguredQueueNotificationConfigData(u16);

    #[register(offset = 58, mode = RW)]
    /// True iff a virtqueue reset is pending.
    ///
    /// The driver sets this bit to true (1) to trigger a virtqueue reset. The
    /// device sets this bit to false (0) when it finishes resetting the virtqueue.
    ///
    /// Only valid if [`VirtioFeatureBits::supports_queue_reset`] was
    /// negotiated.
    ///
    /// The configured virtqueue depends on [`ConfiguredQueueIndex`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="queue_reset"
    pub struct ConfiguredQueueReset(u16);

    #[register(offset = 60, mode = RO)]
    /// The index of the first administration virtqueue.
    ///
    /// Value guaranteed to be at least [`QueueCount`], so administration virtqueue
    /// indices do not overlap with regular virtqueues.
    ///
    /// Only valid if [`VirtioFeatureBits::uses_admin_virtqueues`] was
    /// negotiated.
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="admin_queue_index"
    pub struct FirstAdminQueueIndex(u16);

    #[register(offset = 62, mode = RO)]
    /// The number of supported administration virtqueues.
    ///
    /// The administration virtqueues have contiguous indices starting from
    /// [`FirstAdminQueueIndex`].
    ///
    /// Only valid if [`VirtioFeatureBits::uses_admin_virtqueues`] was
    /// negotiated.
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="admin_queue_num"
    pub struct AdminQueueCount(u16);
}

register_block! {
    // TODO(https://fxbug.dev/522840090): Figure out a more intuitive syntax for
    // the [`mmio::register_block!`] block. The current syntax suggests that
    // we're defining a struct with pub fields, which triggers the question of
    // whether `#[repr(C)]` would be appropriate.

    /// Device configuration common to all virtio devices.
    ///
    /// Covers the memory region identified by
    /// [`PciCapabilityType::COMMON_CONFIGURATION`].
    // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
    // @alias(virtio): theirs="virtio_pci_common_cfg"
    pub struct VirtioPciCommonConfiguration<M> {
        pub device_features_word_index: DeviceFeaturesWordIndex,
        pub device_features_word: DeviceFeaturesWord,
        pub driver_features_word_index: DriverFeaturesWordIndex,
        pub driver_features_word: DriverFeaturesWord,
        pub configuration_change_msix_vector: ConfigurationChangeMsixVector,
        pub queue_count: QueueCount,
        pub device_status: DeviceStatusReg,
        pub device_configation_version: DeviceConfigurationVersion,
        pub configured_queue_index: ConfiguredQueueIndex,
        pub configured_queue_capacity: ConfiguredQueueCapacity,
        pub configured_queue_msix_vector: ConfiguredQueueMsixVector,
        pub configured_queue_enabled: ConfiguredQueueEnabled,
        pub configured_queue_notification_offset: ConfiguredQueueNotificationOffset,
        pub configured_queue_descriptor_table_address_low: ConfiguredQueueDescriptorTableAddressLow,
        pub configured_queue_descriptor_table_address_high: ConfiguredQueueDescriptorTableAddressHigh,
        pub configured_queue_driver_area_address_low: ConfiguredQueueDriverAreaAddressLow,
        pub configured_queue_driver_area_address_high: ConfiguredQueueDriverAreaAddressHigh,
        pub configured_queue_device_area_address_low: ConfiguredQueueDeviceAreaAddressLow,
        pub configured_queue_device_area_address_high: ConfiguredQueueDeviceAreaAddressHigh,
        pub configured_queue_notification_config_data: ConfiguredQueueNotificationConfigData,
        pub configured_queue_reset: ConfiguredQueueReset,
        pub first_admin_queue_index: FirstAdminQueueIndex,
        pub admin_queue_count: AdminQueueCount,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    #[repr(C)]
    struct VirtioPciCommonConfigurationAbi {
        device_feature_select: u32,
        device_feature: u32,
        driver_feature_select: u32,
        driver_feature: u32,
        config_msix_vector: u16,
        num_queues: u16,
        device_status: u8,
        config_generation: u8,

        queue_select: u16,
        queue_size: u16,
        queue_msix_vector: u16,
        queue_enable: u16,
        queue_notify_off: u16,
        queue_desc_low: u32,
        queue_desc_high: u32,
        queue_driver_low: u32,
        queue_driver_high: u32,
        queue_device_low: u32,
        queue_device_high: u32,
        queue_notif_config_data: u16,
        queue_reset: u16,

        admin_queue_index: u16,
        admin_queue_num: u16,
    }

    #[fuchsia::test]
    fn test_virtio_pci_common_cfg_registers() {
        use mmio::Register;

        assert_eq!(
            <DeviceFeaturesWordIndex as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, device_feature_select)
        );

        assert_eq!(
            <DeviceFeaturesWord as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, device_feature)
        );

        assert_eq!(
            <DriverFeaturesWordIndex as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, driver_feature_select)
        );

        assert_eq!(
            <DriverFeaturesWord as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, driver_feature)
        );

        assert_eq!(
            <ConfigurationChangeMsixVector as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, config_msix_vector)
        );

        assert_eq!(
            <QueueCount as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, num_queues)
        );

        assert_eq!(
            <DeviceStatusReg as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, device_status)
        );

        assert_eq!(
            <DeviceConfigurationVersion as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, config_generation)
        );

        assert_eq!(
            <ConfiguredQueueIndex as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, queue_select)
        );

        assert_eq!(
            <ConfiguredQueueCapacity as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, queue_size)
        );

        assert_eq!(
            <ConfiguredQueueMsixVector as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, queue_msix_vector)
        );

        assert_eq!(
            <ConfiguredQueueEnabled as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, queue_enable)
        );

        assert_eq!(
            <ConfiguredQueueNotificationOffset as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, queue_notify_off)
        );

        assert_eq!(
            <ConfiguredQueueDescriptorTableAddressLow as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, queue_desc_low)
        );

        assert_eq!(
            <ConfiguredQueueDescriptorTableAddressHigh as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, queue_desc_high)
        );

        assert_eq!(
            <ConfiguredQueueDriverAreaAddressLow as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, queue_driver_low)
        );

        assert_eq!(
            <ConfiguredQueueDriverAreaAddressHigh as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, queue_driver_high)
        );

        assert_eq!(
            <ConfiguredQueueDeviceAreaAddressLow as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, queue_device_low)
        );

        assert_eq!(
            <ConfiguredQueueDeviceAreaAddressHigh as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, queue_device_high)
        );

        assert_eq!(
            <ConfiguredQueueNotificationConfigData as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, queue_notif_config_data)
        );

        assert_eq!(
            <ConfiguredQueueReset as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, queue_reset)
        );

        assert_eq!(
            <FirstAdminQueueIndex as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, admin_queue_index)
        );

        assert_eq!(
            <AdminQueueCount as Register>::OFFSET,
            offset_of!(VirtioPciCommonConfigurationAbi, admin_queue_num)
        );
    }
}
