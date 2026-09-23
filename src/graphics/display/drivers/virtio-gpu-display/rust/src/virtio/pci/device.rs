// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/504722357): Remove this in favor of more granular
// attributes when the Rust port is completed.
#![allow(dead_code)]

use fidl_next_fuchsia_hardware_pci as fidl_pci;
use log::{info, warn};
use mmio::region::MmioRegion;
use mmio::vmo::VmoMemory;
use std::num::NonZero;
use zx::Bti;

use super::capabilities::VirtioPciCapabilities;
use super::common_configuration::{VirtioPciCommonConfiguration, *};
use super::pci_notifications::VirtioPciNotifications;
use super::pci_queue::VirtioPciQueue;
use crate::virtio::{DeviceStatus, VirtioBufferRef, VirtioFeatureBits, VirtioMemoryRange};

/// Manages a fully initialized virtio device using the PCI transport.
///
/// [`VirtioPciDeviceBuilder`] implements virtio initialization and creates
/// instances.
pub struct VirtioPciDevice {
    /// FIDL client for PCI operations.
    ///
    /// May be necessary for future extensions, such as suspending.
    #[expect(dead_code)]
    pci: fidl_next::Client<fidl_pci::Device>,

    /// BTI that can pin VMOs in device-addressable memory.
    bti: zx::Bti,

    /// The feature bits negotiated with the virtio device.
    #[expect(dead_code)]
    feature_bits: VirtioFeatureBits,

    #[expect(dead_code)]
    configuration: VirtioPciCommonConfiguration<MmioRegion<VmoMemory>>,

    notifications: VirtioPciNotifications,

    /// The configured virtqueues for the device.
    queues: Box<[VirtioPciQueue]>,
}

impl VirtioPciDevice {
    /// Returns a BTI that can pin VMOs in device-addressable memory.
    pub fn bti(&self) -> &zx::Bti {
        &self.bti
    }

    /// Returns the feature bits negotiated with the virtio device.
    #[expect(dead_code)]
    pub fn feature_bits(&self) -> VirtioFeatureBits {
        self.feature_bits
    }

    /// Submits a buffer to the queue, and waits for the buffer to be returned.
    pub async fn submit_and_wait_for_buffer(
        &mut self,
        queue_index: u16,
        buffer: &[VirtioMemoryRange],
    ) -> Result<u32, zx::Status> {
        if queue_index as usize >= self.queues.len() {
            return Err(zx::Status::INVALID_ARGS);
        }
        let queue = &mut self.queues[queue_index as usize];

        let buffer_ref = VirtioBufferRef::new(&buffer);

        // SAFETY: The method keeps the ranges alive until it returns. The
        // method only returns after the buffers are returned.
        let submitted_buffer_id = unsafe { queue.submit_buffer(buffer_ref)? };

        self.notifications.trigger(queue.notification_data());

        let written_bytes = loop {
            if let Some(returned_buffer_info) = queue.take_returned_buffer() {
                assert!(
                    returned_buffer_info.submitted_buffer_id == submitted_buffer_id,
                    "Multiple concurrent buffer submissions not yet supported",
                );
                break returned_buffer_info.written_bytes;
            }

            // TODO(https://fxbug.dev/504722357): Integrate with MSI-X receiver.
            // let _ = self.interrupt_receiver.next().await;
        };

        Ok(written_bytes)
    }
}

/// Builder pattern instantiation for [`VirtioPciDevice`].
///
/// The builder implements virtio initialization. This is a multi-step process
/// because it mixes logic specific to each virtio device type with logic
/// applicable to all devices that use the PCI transport.
pub struct VirtioPciDeviceBuilder {
    pci: fidl_next::Client<fidl_pci::Device>,
    bti: Bti,

    configuration: VirtioPciCommonConfiguration<MmioRegion<VmoMemory>>,
    notifications: VirtioPciNotifications,

    /// Emptied by [`take_device_configuration()`].
    device_configuration: Option<MmioRegion<VmoMemory>>,

    /// Populated by [`read_feature_bits()`].
    offered_features: Option<VirtioFeatureBits>,

    /// Populated by [`write_accepted_features()`].
    accepted_features: Option<VirtioFeatureBits>,

    /// Populated by [`initialize_virtqueues()`].
    queues: Vec<VirtioPciQueue>,
}

impl VirtioPciDeviceBuilder {
    /// Returns a builder ready for feature bits negotiation.
    ///
    /// See [`offered_features()`] for the next initialization step.
    pub async fn new(
        pci: fidl_next::ClientEnd<fidl_pci::Device>,
    ) -> Result<VirtioPciDeviceBuilder, zx::Status> {
        // Our implementation follows steps 1-3 and the part of step 4 that
        // covers reading offered feature bits.
        // @cite(virtio): sec="3.1" title="Device Initialization"

        let pci = pci.spawn();
        let bti = Self::get_pci_bti(&pci).await?;
        let capabilities = VirtioPciCapabilities::new(&pci).await?;

        let mut builder = Self {
            pci: pci.clone(),
            bti,

            configuration: capabilities.common_configuration,
            notifications: capabilities.notifications,
            device_configuration: capabilities.device_configuration,

            offered_features: None,
            accepted_features: None,
            queues: Vec::new(),
        };

        builder.reset_virtio();
        builder.read_offered_features();
        Ok(builder)
    }

    /// Returns the features offered by the device.
    ///
    /// Use the result to compute device type-specific accepted features, and
    /// then call [`accept_features()`].
    pub fn offered_features(&self) -> VirtioFeatureBits {
        // `unwrap()` will not panic because [`new()`] calls
        // [`read_offered_features()`].
        self.offered_features.unwrap()
    }

    /// Leaves the virtio device ready for device type-specific initialization.
    ///
    /// `accepted_features` must be a subset of the bits returned by
    /// [`offered_features()`].
    ///
    /// After this method succeeds, perform device-specific initialization and
    /// then call [`build()`].
    ///
    /// The directions above entail that the method completes feature
    /// negotiation, performs transport-specific initialization, and
    /// automatically configures all discovered virtqueues.
    ///
    /// On failure, sets [`DeviceStatus::driver_terminated`] to true, signaling
    /// that the driver will abandon this device.
    pub async fn accept_features(
        &mut self,
        mut accepted_features: VirtioFeatureBits,
    ) -> Result<(), zx::Status> {
        // Our implementation follows steps 5-6, the part of step 4 that covers
        // writing accepted feature bits, and the part of step 7 that does
        // per-bus setup and virtqueue discovery and configuration.
        // @cite(virtio): sec="3.1" title="Device Initialization"

        // `unwrap()` will not panic because [`new()`] calls
        // [`read_offered_features()`].
        let offered_features = self.offered_features.unwrap();

        if !offered_features.uses_virtio1_standard() {
            warn!("Refusing to operate device without virtio 1.0+ standard support");
            self.set_driver_terminated();
            return Err(zx::Status::NOT_SUPPORTED);
        }
        accepted_features.set_uses_virtio1_standard(true);

        self.write_accepted_features(accepted_features)?;
        self.initialize_virtqueues()?;

        Ok(())
    }

    /// Returns a BTI that can pin VMOs in device-addressable memory.
    ///
    /// Intended to be used during device type-specific initialization.
    pub fn bti(&self) -> &zx::Bti {
        &self.bti
    }

    /// Returns the memory area that holds device-specific configuration.
    ///
    /// The device must present at least one
    /// [`PciCapabilityType::DEVICE_CONFIGURATION`] capability for any device
    /// type that has a device type-specific configuration.
    ///
    /// Returns [`None`] if the device does not expose any device-specific
    /// configuration, or if the method is called more than once.
    // @cite(virtio): sec="2.5" title="Device Configuration Space"
    // @cite(virtio): sec="4.1.4.6" title="Device-specific configuration"
    pub fn take_device_configuration(&mut self) -> Option<MmioRegion<VmoMemory>> {
        self.device_configuration.take()
    }

    /// Consumes the builder and produces an initialized device.
    ///
    /// Must be called after device-specific initialization is completed, which
    /// must follow a successful call to [`accept_features()`].
    ///
    /// On failure, sets [`DeviceStatus::driver_terminated`] to true, signaling
    /// that the driver will abandon this device.
    pub fn build(mut self) -> Result<VirtioPciDevice, zx::Status> {
        // Step 8 in the specification.
        // @cite(virtio): sec="3.1" title="Device Initialization"
        self.finish_virtio_initialization()?;

        Ok(VirtioPciDevice {
            pci: self.pci,
            bti: self.bti,
            feature_bits: self.accepted_features.expect("accept_features() not called"),

            configuration: self.configuration,
            notifications: self.notifications,

            queues: self.queues.into_boxed_slice(),
        })
    }

    /// Retrieves a BTI from the PCI device.
    ///
    /// The returned BTI is suitable for pinning pages in physical memory
    /// addressable by the PCI device.
    async fn get_pci_bti(pci: &fidl_next::Client<fidl_pci::Device>) -> Result<Bti, zx::Status> {
        /// [`fuchsia.hardware.pci/Device.GetBti()`] argument referencing a BTI
        /// that produces physical addresses in the PCI device's addressable
        /// space.
        const PCI_DEVICE_ADDRESSABLE_BTI_ID: u32 = 0;

        let get_bti_response = pci
            .get_bti(PCI_DEVICE_ADDRESSABLE_BTI_ID)
            .await
            .map_err(|_| zx::Status::INTERNAL)?
            .map_err(|_| zx::Status::INTERNAL)?;
        let bti = get_bti_response.bti;
        debug_assert!(!bti.is_invalid(), "GetBti() returned invalid BTI");
        Ok(bti)
    }

    /// Drives a virtio device through reset and early initialization.
    ///
    /// Currently, must be called at most once. This precondition can be easily
    /// relaxed if a driver wants to support resetting the virtio device on
    /// failures.
    ///
    /// Returns when the virtio device is ready to perform feature negotiation.
    fn reset_virtio(&mut self) {
        // In the future, we may want to support resetting a virtio device after it encounters
        // an error. If we go down that path, we should reset [`offered_features`] and
        // [`accepted_features`] here.
        debug_assert!(
            self.offered_features.is_none(),
            "Reset not currently supported after feature negotiation started"
        );
        debug_assert!(
            self.accepted_features.is_none(),
            "Reset not currently supported after feature negotiation"
        );

        // Step 1 in the specification.
        // @cite(virtio): sec="3.1" title="Device Initialization"
        let mut device_status_register = self.configuration.device_status_mut();
        device_status_register.write(DeviceStatusReg(DeviceStatus::RESET.0));

        while device_status_register.read().value() != DeviceStatus::RESET.0 {}

        // Step 2 in the specification.
        // @cite(virtio): sec="3.1" title="Device Initialization"
        let mut device_status = DeviceStatus::RESET;
        device_status.set_virtio_device_detected(true);
        device_status_register.write(DeviceStatusReg(device_status.0));

        // Step 3 in the specification.
        // @cite(virtio): sec="3.1" title="Device Initialization"
        device_status.set_driver_found(true);
        device_status_register.write(DeviceStatusReg(device_status.0));
    }

    /// Reads feature bits offered by the virtio device into a field.
    fn read_offered_features(&mut self) {
        debug_assert!(self.offered_features.is_none(), "Offered features already read");

        // The read part of step 4 in the specification.
        // @cite(virtio): sec="3.1" title="Device Initialization"
        let mut raw_feature_bits: u128 = 0;
        for word_index in 0..4 {
            self.configuration
                .device_features_word_index_mut()
                .write(DeviceFeaturesWordIndex(word_index));
            let raw_feature_word: u32 = self.configuration.device_features_word().read().value();
            raw_feature_bits |= (raw_feature_word as u128) << (word_index * 32);
        }

        self.offered_features = Some(VirtioFeatureBits(raw_feature_bits));
    }

    /// Completes virtio initialization that is not specific to a device type.
    ///
    /// `feature_bits` are the feature bits accepted by the driver.
    /// `feature_bits` must be a subset of the feature bits offered by the
    /// device, which can be obtained by calling [`read_device_features()`].
    ///
    /// Must be called at most once during feature negotiation.
    ///
    /// Returns when the virtio device has completed feature negotiation.
    ///
    /// On failure, sets [`DeviceStatus::driver_terminated`] to true, signaling
    /// that the driver will abandon this device.
    fn write_accepted_features(
        &mut self,
        feature_bits: VirtioFeatureBits,
    ) -> Result<(), zx::Status> {
        debug_assert!(
            !self.offered_features.is_none(),
            "Accepted features are not based on offered features"
        );
        debug_assert!(self.accepted_features.is_none(), "Accepted features already written");
        debug_assert!(
            feature_bits.0 & self.offered_features.unwrap().0 == feature_bits.0,
            "Accepted features are not a subset of the offered features",
        );

        let mut device_status = DeviceStatus(self.configuration.device_status().read().value());

        let mut expected_device_status = DeviceStatus::default();
        expected_device_status.set_virtio_device_detected(true);
        expected_device_status.set_driver_found(true);

        if expected_device_status != device_status {
            warn!(
                "Unexpected virtio device status during feature negotiation: {:?}",
                device_status
            );

            device_status.set_driver_terminated(true);
            self.configuration.device_status_mut().write(DeviceStatusReg(device_status.0));
            return Err(zx::Status::IO);
        }

        // The write part of step 4 in the specification.
        // @cite(virtio): sec="3.1" title="Device Initialization"
        let mut raw_feature_bits: u128 = feature_bits.0;
        for word_index in 0..4 {
            let raw_feature_word: u32 = raw_feature_bits as u32;
            raw_feature_bits >>= 32;

            // Skip over words that wouldn't set any bits.
            //
            // This avoids using higher word index values on devices that may
            // not be able to handle them.
            if raw_feature_word == 0 {
                continue;
            }

            self.configuration
                .driver_features_word_index_mut()
                .write(DriverFeaturesWordIndex(word_index));
            self.configuration
                .driver_features_word_mut()
                .write(DriverFeaturesWord(raw_feature_word));
        }

        // Step 5 in the specification.
        // @cite(virtio): sec="3.1" title="Device Initialization"
        device_status.set_feature_negotiation_complete(true);
        self.configuration.device_status_mut().write(DeviceStatusReg(device_status.0));

        // Step 6 in the specification.
        // @cite(virtio): sec="3.1" title="Device Initialization"
        device_status = DeviceStatus(self.configuration.device_status().read().value());
        if !device_status.feature_negotiation_complete() {
            warn!(
                "virtio device does not support offered features {:#x} {:?}",
                feature_bits.0, feature_bits
            );

            device_status.set_driver_terminated(true);
            self.configuration.device_status_mut().write(DeviceStatusReg(device_status.0));
            return Err(zx::Status::IO);
        }

        self.accepted_features = Some(feature_bits);
        Ok(())
    }

    /// Discovers and configures all the device's virtqueues.
    ///
    /// The device must have completed PCI transport initialization.
    ///
    /// On failure, sets [`DeviceStatus::driver_terminated`] to true, signaling
    /// that the driver will abandon this device.
    fn initialize_virtqueues(&mut self) -> Result<(), zx::Status> {
        debug_assert!(self.queues.is_empty(), "The device's virtqueues are already set up");

        let queue_count = self.configuration.queue_count().read().value();

        for queue_index in 0..queue_count {
            // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
            self.configuration
                .configured_queue_index_mut()
                .write(ConfiguredQueueIndex(queue_index));

            let queue_capacity_raw = self.configuration.configured_queue_capacity().read().value();
            let Some(queue_capacity) = NonZero::<u16>::new(queue_capacity_raw) else {
                warn!("virtqueue {} is disabled (capacity set to 0)", queue_index);
                self.set_driver_terminated();
                return Err(zx::Status::IO_DATA_LOSS);
            };

            let notification_offset =
                self.configuration.configured_queue_notification_offset().read();

            // The ID needs to be read from the configuration area if
            // [`VirtioFeatureBits::uses_custom_virtqueue_ids`] was negotiated.
            let queue_id = queue_index;

            let notification_data =
                self.notifications.data_for_queue(queue_id, notification_offset);

            let (pci_queue, queue_memory_layout) =
                VirtioPciQueue::new(&self.bti, queue_capacity, notification_data)?;

            // 64-bit fields must be treated and accessed as two 32-bit fields,
            // with the low 32-bit part followed by the high 32-bit part.
            // @cite(virtio): sec="4.1.3.1" title="Driver Requirements: PCI Device Layout"
            // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
            let desc_addr = queue_memory_layout.descriptor_table_physical_address;
            self.configuration
                .configured_queue_descriptor_table_address_low_mut()
                .write(ConfiguredQueueDescriptorTableAddressLow(desc_addr as u32));
            self.configuration
                .configured_queue_descriptor_table_address_high_mut()
                .write(ConfiguredQueueDescriptorTableAddressHigh((desc_addr >> 32) as u32));

            let driver_addr = queue_memory_layout.submitted_ring_physical_address;
            self.configuration
                .configured_queue_driver_area_address_low_mut()
                .write(ConfiguredQueueDriverAreaAddressLow(driver_addr as u32));
            self.configuration
                .configured_queue_driver_area_address_high_mut()
                .write(ConfiguredQueueDriverAreaAddressHigh((driver_addr >> 32) as u32));

            let device_addr = queue_memory_layout.returned_ring_physical_address;
            self.configuration
                .configured_queue_device_area_address_low_mut()
                .write(ConfiguredQueueDeviceAreaAddressLow(device_addr as u32));
            self.configuration
                .configured_queue_device_area_address_high_mut()
                .write(ConfiguredQueueDeviceAreaAddressHigh((device_addr >> 32) as u32));

            self.configuration.configured_queue_enabled_mut().write(ConfiguredQueueEnabled(1));
            self.queues.push(pci_queue);
        }

        Ok(())
    }

    /// Completes virtio device initialization.
    ///
    /// Must be called after device-specific initialization is complete.
    ///
    /// On failure, sets [`DeviceStatus::driver_terminated`] to true, signaling
    /// that the driver will abandon this device.
    fn finish_virtio_initialization(&mut self) -> Result<(), zx::Status> {
        let mut device_status = DeviceStatus(self.configuration.device_status().read().value());

        let mut expected_device_status = DeviceStatus::default();
        expected_device_status.set_virtio_device_detected(true);
        expected_device_status.set_driver_found(true);
        expected_device_status.set_feature_negotiation_complete(true);

        if expected_device_status != device_status {
            warn!(
                "Unexpected virtio device status after device-specific initialization: {:?}",
                device_status
            );

            device_status.set_driver_terminated(true);
            self.configuration.device_status_mut().write(DeviceStatusReg(device_status.0));
            return Err(zx::Status::IO);
        }

        // Step 8 in the specification.
        // @cite(virtio): sec="3.1" title="Device Initialization"
        device_status.set_driver_initialized(true);
        self.configuration.device_status_mut().write(DeviceStatusReg(device_status.0));

        Ok(())
    }

    /// Signals that the driver will abandon this device.
    ///
    /// Sets [`DeviceStatus::driver_terminated`] to true (1).
    // @cite(virtio): sec="2.1" title="Device Status Field"
    // @cite(virtio): sec="3.1.1" title="Driver Requirements: Device Initialization"
    fn set_driver_terminated(&mut self) {
        let mut device_status = DeviceStatus(self.configuration.device_status().read().value());
        device_status.set_driver_terminated(true);
        self.configuration.device_status_mut().write(DeviceStatusReg(device_status.0));
    }
}

// TODO(https://fxbug.dev/522425080): Add unit tests for MMIO access sequences.
