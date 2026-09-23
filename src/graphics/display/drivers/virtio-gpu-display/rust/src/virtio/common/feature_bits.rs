// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(doc)]
use super::device_status::DeviceStatus;

#[cfg(doc)]
use super::queue_abi::VirtioMemoryRangeFlags;

use bitfield::bitfield;

bitfield! {
    /// Configures virtio device operation.
    ///
    /// Each bit represents an optional feature or an alternative mode for an
    /// aspect of the virtio device's operation.
    // @cite(virtio): sec="2.2" title="Feature Bits"
    // @cite(virtio): sec="6" title="Reserved Feature Bits"
    #[derive(Default, Copy, Clone, PartialEq, Eq)]
    pub struct VirtioFeatureBits(u128);
    impl Debug;

    /// The device notifies when a virtqueue has no submitted buffers.
    ///
    /// If this feature is negotiated, the device issues a virtqueue
    /// notification when it has completed all the commands submitted on the
    /// virtqueue.
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=24
    // @alias(virtio): theirs="VIRTIO_F_NOTIFY_ON_EMPTY"
    pub bool, notify_on_empty_queue, set_notify_on_empty_queue: 24;

    /// Remove virtio queue memory layout constraints in legacy mode.
    ///
    /// The feature is only meaningful if [`uses_virtio1_standard`] is not
    /// negotiated. Flexible memory layout is required by the virtio 1.0+
    /// standard.
    ///
    /// If this feature or [`uses_virtio1_standard`] is negotiated, the driver
    /// does not need to support memory layout limitations in early virtio
    /// device implementations.
    ///
    /// If neither feature is negotiated, the driver must meet the constraints
    /// specified in the "Legacy Interface: Framing Requirements" sections of
    /// the relevant device type specification.
    ///
    /// This driver only supports virtio 1.0+ devices, and does not attempt to
    /// negotiate this feature.
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=27
    // @alias(virtio): theirs="VIRTIO_F_ANY_LAYOUT"
    pub bool, supports_flexible_memory_layout, set_supports_flexible_memory_layout: 27;

    /// Indirect descriptors are supported.
    ///
    /// [`VirtioMemoryRangeFlags::is_indirect`] may be set to true iff
    /// this feature is negotiated.
    // @cite(virtio): sec="2.7.5.3" title="Indirect descriptors"
    // @cite(virtio): sec="2.8.7" title="Indirect Flag: Scatter-Gather Support"
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=28
    // @alias(virtio): theirs="VIRTIO_F_INDIRECT_DESC"
    pub bool, supports_indirect_descriptors, set_supports_indirect_descriptors: 28;

    /// Use selective virtqueue notifications.
    // @cite(virtio): sec="2.7.7" title="Used Buffer Notification Suppression"
    // @cite(virtio): sec="2.7.10" title="Available Buffer Notification Suppression"
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=29
    // @alias(virtio): theirs="VIRTIO_F_EVENT_IDX"
    pub bool, uses_virtqueue_notification_index, set_uses_virtqueue_notification_index: 29;

    /// Workaround for bug in early QEMU implementations.
    ///
    /// This driver does not attempt to negotiate this feature.
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=30
    // @alias(virtio): theirs="UNUSED"
    pub bool, unused_qemu_experimental, set_unused_qemu_experimental: 30;

    /// Use the virtio 1.0+ standard.
    ///
    /// Failing to negotiate this feature enables behaviors called "legacy" in
    /// the specification.
    ///
    /// This driver only supports virtio 1.0+ devices, and fails to initialize
    /// if the device does not offer this feature.
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=32
    // @cite(virtio): sec="6.1" title="Driver Requirements: Reserved Feature Bits"
    // @alias(virtio): theirs="VIRTIO_F_VERSION_1"
    pub bool, uses_virtio1_standard, set_uses_virtio1_standard: 32;

    /// Signals that the device's memory accesses are limited or translated.
    ///
    /// By default, the driver may assume that the virtio implementation can
    /// access any physical memory address provided by the driver. If this
    /// feature is negotiated, the device's memory accesses may be gated by an
    /// IOMMU.
    ///
    /// This driver does not currently support this feature.
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=33
    // @cite(virtio): sec="6.1" title="Driver Requirements: Reserved Feature Bits"
    // @cite(virtio): sec="6.2" title="Device Requirements: Reserved Feature Bits"
    // @alias(virtio): theirs="VIRTIO_F_ACCESS_PLATFORM"
    pub bool, has_limited_memory_access, set_has_limited_memory_access: 33;

    /// Use packed virtqueues, as opposed to split virtqueues.
    ///
    /// By default, virtqueues use split virtqueues. If this feature is negotiated,
    /// virtqueues use packed virtqueues.
    ///
    /// This driver currently only supports split virtqueues, and does not attempt
    /// to negotiate this feature.
    // @cite(virtio): sec="2.7" title="Split Virtqueues"
    // @cite(virtio): sec="2.8" title="Packed Virtqueues"
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=34
    // @cite(virtio): sec="6.1" title="Driver Requirements: Reserved Feature Bits"
    // @alias(virtio): theirs="VIRTIO_F_RING_PACKED"
    pub bool, uses_packed_virtqueues, set_uses_packed_virtqueues: 34;

    /// The device always returns virtqueue buffers in submission order.
    ///
    /// By default, virtio devices are free to return submitted virtqueue
    /// buffers in any order. If this feature is negotiated, devices will follow
    /// submission order when returning the buffers. So, negotiating this
    /// feature can simplify the driver's buffer management, at the cost of
    /// reducing performance optimization opportunities for the device.
    ///
    /// Only some devices offer this feature. For example, the virtio-gpu
    /// implementation in QEMU and crosvm does not offer this feature.
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=35
    // @alias(virtio): theirs="VIRTIO_F_IN_ORDER"
    pub bool, returns_buffers_in_submission_order, set_returns_buffers_in_submission_order: 35;

    /// Use the memory access ordering rules for device memory accesses.
    ///
    /// By default, the virtio device must behave as if it accesses memory
    /// directly from one of the CPU cores, which entails access to coherent
    /// caches.
    ///
    /// If this feature is negotiated, the virtio device may perform
    /// optimizations that assume the driver issues the memory access barriers
    /// needed for external hardware devices.
    ///
    /// This driver always issues the memory barriers required by the platform,
    /// even if the feature is not negotiated.
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=36
    // @cite(virtio): sec="6.1" title="Driver Requirements: Reserved Feature Bits"
    // @cite(virtio): sec="6.2" title="Device Requirements: Reserved Feature Bits"
    // @alias(virtio): theirs="VIRTIO_F_ORDER_PLATFORM"
    pub bool, assumes_hardware_memory_barriers, set_assumes_hardware_memory_barriers: 36;

    /// Enables device support for PCI Single Root I/O Virtualization (SR-IOV).
    ///
    /// If this feature is negotiated, the driver may use the PCI SR-IOV capability
    /// structure to enable virtual functions.
    ///
    /// This driver does not use SR-IOV.
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=37
    // @alias(virtio): theirs="VIRTIO_F_SR_IOV"
    pub bool, supports_io_virtualization, set_supports_io_virtualization: 37;

    /// Enables extra data in the driver's device notifications.
    ///
    /// By default, the notifications issued by the driver only identify a
    /// target virtqueue. If this feature is negotiated, the notifications include
    /// more data.
    ///
    /// This driver does not attempt to negotiate this feature. None of the
    /// virtio implementations targeted by this driver take advantage of this
    /// feature.
    // @cite(virtio): sec="2.9" title="Driver Notifications"
    // @cite(virtio): sec="4.1.5.2" title="Available Buffer Notifications"
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=38
    // @alias(virtio): theirs="VIRTIO_F_NOTIFICATION_DATA"
    pub bool, uses_extended_notification_data, set_uses_extended_notification_data: 38;

    /// Enables custom virtqueue IDs in driver-issued notifications.
    ///
    /// By default, the driver identifies virtqueues using their index. If this
    /// feature is negotiated, the notifications include a custom virtqueue ID
    /// provided by the device.
    ///
    /// This driver does not attempt to negotiate this feature. The feature is
    /// not implemented in any of the virtio implementations targeted by this
    /// driver.
    // @cite(virtio): sec="2.9" title="Driver Notifications"
    // @cite(virtio): sec="4.1.5.2" title="Available Buffer Notifications"
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=39
    // @alias(virtio): theirs="VIRTIO_F_NOTIF_CONFIG_DATA"
    pub bool, uses_custom_virtqueue_ids, set_uses_custom_virtqueue_ids: 39;

    /// Enables virtqueue-level reset.
    ///
    /// If this feature is not negotiated, the driver only has a device-level
    /// reset mechanism.
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=40
    // @alias(virtio): theirs="VIRTIO_F_RING_RESET"
    pub bool, supports_queue_reset, set_supports_queue_reset: 40;

    /// Enables driver usage of the device's administration virtqueues.
    ///
    /// If this feature is negotiated, the driver must configure the device's
    /// administration virtqueues during device initialization.
    ///
    /// This driver does not support administration virtqueues.
    // @cite(virtio): sec="2.13" title="Administration Virtqueues"
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=41
    // @alias(virtio): theirs="VIRTIO_F_ADMIN_VQ"
    pub bool, uses_admin_virtqueues, set_uses_admin_virtqueues: 41;

    /// Enables the device's suspend functionality.
    ///
    /// If this feature is enabled [`DeviceStatus::suspended`] may be set.
    // @cite(virtio): sec="6" title="Reserved Feature Bits" bits=43
    // @alias(virtio): theirs="VIRTIO_F_SUSPEND"
    pub bool, suspend_enabled, set_suspend_enabled: 43;
}
