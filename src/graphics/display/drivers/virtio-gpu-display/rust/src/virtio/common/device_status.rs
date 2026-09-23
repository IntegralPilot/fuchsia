// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitfield::bitfield;

bitfield! {
    /// Modified by both the driver and the device during lifecycle transitions.
    ///
    /// The bits are intended to represent stages in the device + driver
    /// lifecycle. After a device is reset, the value is guaranteed to be
    /// [`DeviceStatus::RESET`].
    ///
    /// With the exception of resetting the device, the driver must not clear
    /// (write 0 to) a bit that is set (to 1).
    // @cite(virtio): sec="2.1" title="Device Status Field"
    // @cite(virtio): sec="2.4" title="Device Reset"
    // @cite(virtio): sec="3" title="General Initialization And Device Operation"
    #[derive(Default, Copy, Clone, PartialEq, Eq)]
    #[repr(transparent)]
    pub struct DeviceStatus(u8);
    impl Debug;

    /// True iff the driver recognized the hardware as a virtio device.
    ///
    /// Set to true (1) by the driver in step 2 of device initialization.
    ///
    /// The specification recommends that the virtualized operating system sets
    /// this bit. Fuchsia does not special-case virtio devices, so this bit is
    /// set by Fuchsia drivers.
    // @cite(virtio): sec="2.1" title="Device Status Field" bits=0
    // @cite(virtio): sec="3.1.1" title="Driver Requirements: Device Initialization"
    // @alias(virtio): theirs="ACKNOWLEDGE"
    pub bool, virtio_device_detected, set_virtio_device_detected: 0;

    /// True iff the guest OS has found a driver for the device.
    ///
    /// Set to true (1) by the driver in step 3 of device initialization. Fuchsia
    /// does not special-case virtio devices, so the driver must perform this
    /// step.
    ///
    /// The driver may only read the feature bits offered by the device after
    /// setting this bit to true.
    // @cite(virtio): sec="2.1" title="Device Status Field" bits=1
    // @cite(virtio): sec="3.1.1" title="Driver Requirements: Device Initialization"
    // @alias(virtio): theirs="DRIVER"
    pub bool, driver_found, set_driver_found: 1;

    /// True iff the driver is sufficiently initialized to drive the device.
    ///
    /// Set to true (1) by the driver in step 8 of device initialization.
    ///
    /// The driver must only set this bit to true after it has completed the
    /// device-specific initialization process.
    ///
    /// The driver may only operate the device's virtqueues while this bit is
    /// set to true (1).
    // @cite(virtio): sec="2.1" title="Device Status Field" bits=2
    // @cite(virtio): sec="3.1.1" title="Driver Requirements: Device Initialization"
    // @alias(virtio): theirs="DRIVER_OK"
    pub bool, driver_initialized, set_driver_initialized: 2;

    /// True iff the feature negotiation is complete.
    ///
    /// Set to true (1) by the driver in step 5 of device initialization.
    ///
    /// The driver must only set this bit to true after it has acknowledged all
    /// the features it understands.
    // @cite(virtio): sec="2.1" title="Device Status Field" bits=3
    // @cite(virtio): sec="3.1.1" title="Driver Requirements: Device Initialization"
    // @alias(virtio): theirs="FEATURES_OK"
    pub bool, feature_negotiation_complete, set_feature_negotiation_complete: 3;

    /// True iff the device has been suspended by the driver.
    ///
    /// The driver triggers device suspension by setting this bit to true (1).
    /// The device sets the [`driver_initialized`] bit to false (0) after the
    /// device completes suspending.
    ///
    /// The driver triggers resuming a suspended device by setting the
    /// [`driver_initialized`] bit to true (1). The device sets this bit to
    /// false (0) after the device completes resuming.
    ///
    /// The driver is only allowed to set this bit if
    /// [`VirtioFeatureBits::suspend_enabled`] is negotiated.
    // @cite(virtio): sec="2.1" title="Device Status Field" bits=4
    // @cite(virtio): sec="3.4" title="Device Suspend"
    // @alias(virtio): theirs="SUSPEND"
    pub bool, suspended, set_suspended: 4;

    /// True iff the device has experienced an unrecoverable error.
    ///
    /// The device sends a configuration change notification when setting this
    /// bit to true (1). The driver cannot assume that virtqueue operations will
    /// complete (or, conversely, that the operations will be dropped) while
    /// this bit is set.
    // @cite(virtio): sec="2.1" title="Device Status Field" bits=6
    // @alias(virtio): theirs="DEVICE_NEEDS_RESET"
    pub bool, device_needs_reset, set_device_needs_reset: 6;

    /// True iff the driver has experienced an unrecoverable error.
    ///
    /// The driver sets this bit when it gives up on driving the device.
    // @cite(virtio): sec="2.1" title="Device Status Field" bits=7
    // @alias(virtio): theirs="FAILED"
    pub bool, driver_terminated, set_driver_terminated: 7;
}

impl DeviceStatus {
    /// The status value that indicates the device has been reset.
    // @cite(virtio): sec="2.4" title="Device Reset"
    pub const RESET: DeviceStatus = DeviceStatus(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_device_status() {
        let mut status = DeviceStatus(0);
        assert!(!status.virtio_device_detected());
        status.set_virtio_device_detected(true);
        assert!(status.virtio_device_detected());
        assert_eq!(status.0, 1);
    }

    #[fuchsia::test]
    fn test_device_status_debug() {
        let mut status = DeviceStatus(0);
        status.set_virtio_device_detected(true);
        let debug_str = format!("{:?}", status);
        assert_eq!(
            debug_str,
            "DeviceStatus { .0: 1, virtio_device_detected: true, driver_found: false, driver_initialized: false, feature_negotiation_complete: false, suspended: false, device_needs_reset: false, driver_terminated: false }"
        );

        let mut status3 = DeviceStatus(0);
        status3.set_virtio_device_detected(true);
        status3.set_driver_found(true);
        status3.set_driver_initialized(true);
        let debug_str3 = format!("{:?}", status3);
        assert_eq!(
            debug_str3,
            "DeviceStatus { .0: 7, virtio_device_detected: true, driver_found: true, driver_initialized: true, feature_negotiation_complete: false, suspended: false, device_needs_reset: false, driver_terminated: false }"
        );
    }
}
