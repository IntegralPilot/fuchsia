// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Manages Device notifications over the PCI transport.
//!
//! At the highest level, device notifications are issued by the device (the
//! virtualized hardware), and driver notifications are issued by the driver.
//!
//! Device notifications inform the driver that either the virtualized hardware
//! configuration changed (configuration change notifications), or that the
//! device completed some work and returned a previously submitted buffer (used
//! buffer notification).

#[cfg(doc)]
use super::capability_type::PciCapabilityType;

use mmio::Mmio;
use mmio::region::MmioRegion;
use mmio::vmo::VmoMemory;

use super::common_configuration::ConfiguredQueueNotificationOffset;

/// Information for triggering a driver-issued notification.
///
/// The driver is responsible for issuing a driver notification when it submits
/// buffers to a virtqueue. The buffers generally convey work that must be done
/// by the device.
// @cite(virtio): sec="2.3" title="Notifications"
// @cite(virtio): sec="2.9" title="Driver Notifications"
// @cite(virtio): sec="4.1.5.2" title="Available Buffer Notifications"
pub struct VirtioPciNotificationData {
    /// Points to the notification structure's first byte.
    ///
    /// Relative to [`mmio_region`].
    mmio_offset: usize,

    /// Identifies the virtqueue in a driver-issued notification.
    virtio_queue_id: u16,
}

/// Manages a virtio PCI device's notifications area.
///
/// The notifications area is exposed in a PCI capability of the type
/// [`PciCapabilityType::NOTIFICATIONS`].
pub struct VirtioPciNotifications {
    mmio_region: MmioRegion<VmoMemory>,

    /// Number of bytes between two consecutive notification structures.
    ///
    /// The notifications area is conceptually an array of notification
    /// structures. The size of each notification structure (the array's array
    /// stride) is decided at runtime by the virtio device implementation.
    ///
    /// The remainder of the structures is an internal detail specific to virtio
    /// implementations.
    // @cite(virtio): sec="4.1.5.2" title="Available Buffer Notifications"
    stride: u32,
}

impl VirtioPciNotifications {
    pub fn new(mmio_region: MmioRegion<VmoMemory>, stride: u32) -> Self {
        Self { mmio_region, stride }
    }

    /// Returns a description of a virtqueue's notification structure.
    pub fn data_for_queue(
        &mut self,
        virtio_queue_id: u16,
        notification_offset: ConfiguredQueueNotificationOffset,
    ) -> VirtioPciNotificationData {
        // @cite(virtio): sec="4.1.4.4" title="Notification structure layout"
        let mmio_offset = (self.stride as usize) * (notification_offset.value() as usize);

        debug_assert!(
            mmio_offset + std::mem::size_of::<u16>() <= self.mmio_region.len(),
            "Notification offset too large: {:?}",
            notification_offset
        );

        VirtioPciNotificationData { mmio_offset, virtio_queue_id }
    }

    /// Issues a driver notification.
    // @cite(virtio): sec="4.1.5.2" title="Available Buffer Notifications"
    pub fn trigger(&mut self, queue_data: &VirtioPciNotificationData) {
        // [`VirtoFeatureBits::uses_extended_notification_data`] is currently
        // unsupported. If we need to add support, a straightforward path is to
        // add a `next_submitted_ring_index` argument here and plumb the data
        // from the virtqueue implementation.

        // Ensure available ring and descriptor table updates are committed
        // before writing to the MMIO doorbell register.
        self.mmio_region.write_barrier();
        self.mmio_region.store16(queue_data.mmio_offset, queue_data.virtio_queue_id);
    }
}
