// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! virtio PCI capability parsing.

use super::bar_map::{PciDeviceBarMap, PciDeviceBarMapBuilder};
use super::capability_type::PciCapabilityType;
use super::common_configuration::VirtioPciCommonConfiguration;
use super::pci_notifications::VirtioPciNotifications;
use fidl_next_fuchsia_hardware_pci as fidl_pci;
use log::{info, warn};
use mmio::region::MmioRegion;
use mmio::vmo::{VmoMapping, VmoMemory};
use std::mem::{offset_of, size_of};

/// Data region information stored by virtio in a PCI capability.
///
/// Defines PCI capabilities omitting the header and containing additional data.
// @cite(pci): sec="6.7" title="Capabilities list"
// @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
// @cite(virtio): sec="4.1.4.4" title="Notification structure layout"
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PciCapabilityData {
    /// The type of virtio data declared by this capability.
    // @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
    // @alias(virtio): theirs="cfg_type"
    type_: PciCapabilityType,

    /// The index of the BAR region that contains the data.
    ///
    /// Values 0x0 to 0x5 specify a Base Address register (BAR) belonging to the
    /// function located beginning at 10h in PCI Configuration Space and used to
    /// map the structure into Memory or I/O Space.
    ///
    /// See `BaseAddressRegion` for accessing the memory area pointed by a Base
    /// Address Register (BAR).
    // @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
    // @alias(virtio): theirs="bar"
    bar_index: u8,

    /// Distinguishes between multiple instances of the same virtio data type.
    ///
    /// Used by some device types to uniquely identify multiple capabilities of a
    /// certain type.
    // @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
    // @alias(virtio): theirs="id"
    capability_index: u8,

    /// The offset of the virtio data in the BAR region.
    ///
    /// Indicates where the structure begins relative to the base address associated
    /// with the BAR.
    ///
    /// See `BaseAddressRegion` for accessing the memory area pointed by a Base
    /// Address Register (BAR).
    // @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
    // @alias(virtio): theirs="offset"
    // @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
    // @alias(virtio): theirs="offset_hi"
    data_offset: u64,

    /// The length of the virtio data in the BAR region.
    ///
    /// The reported length may exceed the length of the corresponding structure
    /// in the virtio specification, because it may include padding and
    /// non-standard data.
    // @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
    // @alias(virtio): theirs="length"
    // @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
    // @alias(virtio): theirs="length_hi"
    data_length: u64,

    /// The number of bytes between two notification structures.
    ///
    /// Set (not [`None`]) iff the capability type is
    /// [`PciCapabilityType::NOTIFICATIONS`].
    ///
    /// The specification explicitly states that the multiplier can be zero,
    /// meaning that all the virtqueues use the same multiplier.
    // @cite(virtio): sec="4.1.4.4" title="Notification structure layout"
    // @alias(virtio): theirs="notify_off_multiplier"
    notification_stride: Option<u32>,
}

/// Memory layout for a 32-bit PCI capability parsed by [`PciCapabilityData`].
///
/// Used for computing field offsets in the PCI configuration space.
// @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
// @alias(virtio): theirs="virtio_pci_cap"
#[repr(C)]
struct VirtioPciCapability32 {
    cap_vndr: u8,
    cap_next: u8,
    cap_len: u8,
    cfg_type: u8,
    bar: u8,
    id: u8,
    padding: [u8; 2],
    offset: u32,
    length: u32,
}

/// Memory layout for a 64-bit PCI capability parsed by [`PciCapabilityData`].
///
/// Used for computing field offsets in the PCI configuration space.
// @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
// @alias(virtio): theirs="virtio_pci_cap64"
#[repr(C)]
struct VirtioPciCapability64 {
    cap: VirtioPciCapability32,
    offset_hi: u32,
    length_hi: u32,
}

/// Memory layout for a notification PCI capability parsed by
/// [`PciCapabilityData`].
///
/// Used for computing field offsets in the PCI configuration space.
// @cite(virtio): sec="4.1.4.4" title="Notification structure layout"
// @alias(virtio): theirs="virtio_pci_notify_cap"
#[repr(C)]
struct VirtioPciNotifyCapability {
    cap: VirtioPciCapability32,
    notify_off_multiplier: u32,
}

impl PciCapabilityData {
    /// Parses the information in a virtio PCI device capability.
    ///
    /// `pci` is used to retrieve PCI configuration data, and must be valid.
    ///
    /// `offset` is relative to the start of the PCI configuration space, and
    /// must point to the beginning of a vendor-specific (type 9) PCI capability
    /// structure.
    ///
    /// Returns [`None`] if the capability must be ignored by the driver, based
    /// on the rules in the virtio specification.
    pub async fn new(
        pci: &fidl_next::Client<fidl_pci::Device>,
        offset: u8,
    ) -> Result<Option<PciCapabilityData>, zx::Status> {
        // TODO(https://fxbug.dev/523333960): Refine error handling for PCI FIDL calls.

        let pci_capability_type_id = pci
            .read_config8(offset as u16 + offset_of!(VirtioPciCapability32, cap_vndr) as u16)
            .await
            .map_err(|_| zx::Status::INTERNAL)?
            .map_err(|_| zx::Status::INTERNAL)?
            .value;
        assert!(
            fidl_pci::CapabilityId::from(pci_capability_type_id) == fidl_pci::CapabilityId::Vendor,
            "Invalid PCI capability type: {:?}",
            fidl_pci::CapabilityId::from(pci_capability_type_id)
        );

        let capability_length = pci
            .read_config8(offset as u16 + offset_of!(VirtioPciCapability32, cap_len) as u16)
            .await
            .map_err(|_| zx::Status::INTERNAL)?
            .map_err(|_| zx::Status::INTERNAL)?
            .value;

        if usize::from(capability_length) <= offset_of!(VirtioPciCapability32, cfg_type) {
            warn!("Ignoring vendor PCI capability too small to contain a virtio type");
            return Ok(None);
        }

        let virtio_capability_type = PciCapabilityType(
            pci.read_config8(offset as u16 + offset_of!(VirtioPciCapability32, cfg_type) as u16)
                .await
                .map_err(|_| zx::Status::INTERNAL)?
                .map_err(|_| zx::Status::INTERNAL)?
                .value,
        );

        if !virtio_capability_type.has_bar_data() {
            return Ok(None);
        }

        // Capability length must include all data fields and drivers may check that the length is large enough.
        // @cite(virtio): sec="4.1.4.2" title="Device Requirements: Virtio Structure PCI Capabilities"
        // @cite(virtio): sec="4.1.4.1" title="Driver Requirements: Virtio Structure PCI Capabilities"
        if capability_length < size_of::<VirtioPciCapability32>() as u8 {
            warn!(
                "virtio PCI capability {:?} has length {:?}, too small to encode a BAR pointer",
                virtio_capability_type, capability_length
            );
            return Err(zx::Status::IO_DATA_INTEGRITY);
        }

        let bar_index = pci
            .read_config8(offset as u16 + offset_of!(VirtioPciCapability32, bar) as u16)
            .await
            .map_err(|_| zx::Status::INTERNAL)?
            .map_err(|_| zx::Status::INTERNAL)?
            .value;

        // Invalid BAR indices are designated as "reserved", and drivers must ignore PCI capabilities that use reserved values.
        // @cite(virtio): sec="4.1.4.1" title="Driver Requirements: Virtio Structure PCI Capabilities"
        if !PciDeviceBarMap::is_valid_bar_index(bar_index) {
            warn!("Ignoring capability with reserved BAR index: {}", bar_index);
            return Ok(None);
        }

        let capability_index = pci
            .read_config8(offset as u16 + offset_of!(VirtioPciCapability32, id) as u16)
            .await
            .map_err(|_| zx::Status::INTERNAL)?
            .map_err(|_| zx::Status::INTERNAL)?
            .value;

        let mut data_offset = pci
            .read_config32(offset as u16 + offset_of!(VirtioPciCapability32, offset) as u16)
            .await
            .map_err(|_| zx::Status::INTERNAL)?
            .map_err(|_| zx::Status::INTERNAL)?
            .value as u64;
        let mut data_length = pci
            .read_config32(offset as u16 + offset_of!(VirtioPciCapability32, length) as u16)
            .await
            .map_err(|_| zx::Status::INTERNAL)?
            .map_err(|_| zx::Status::INTERNAL)?
            .value as u64;

        let notification_stride;
        if virtio_capability_type == PciCapabilityType::NOTIFICATIONS {
            // Special case for notification structure layout.
            // @cite(virtio): sec="4.1.4.4" title="Notification structure layout"

            if capability_length < size_of::<VirtioPciNotifyCapability>() as u8 {
                warn!(
                    "virtio PCI capability {:?} has length {:?}, too small to encode a BAR pointer",
                    virtio_capability_type, capability_length
                );
                return Err(zx::Status::IO_DATA_INTEGRITY);
            }
            notification_stride = Some(
                pci.read_config32(
                    offset as u16
                        + offset_of!(VirtioPciNotifyCapability, notify_off_multiplier) as u16,
                )
                .await
                .map_err(|_| zx::Status::INTERNAL)?
                .map_err(|_| zx::Status::INTERNAL)?
                .value,
            );

            // The section defines a virtio_pci_notify_cap structure, which
            // includes virtio_pci_cap, implying that notifications capabilities
            // always use 32-bit memory region offsets and lengths.
        } else {
            notification_stride = None;

            // We assume that any capability large enough to fit a 64-bit
            // capability header (struct virtio_pci_cap64) starts with a 64-bit
            // capability header, as opposed to storing a 32-bit capability
            // header followed by non-standard data.
            //
            // Shared memory capabilities use the 64-bit header. The specification is
            // silent about the other capability types.
            // @cite(virtio): sec="4.1.4.7" title="Shared memory capability"
            if capability_length >= size_of::<VirtioPciCapability64>() as u8 {
                let offset_bits63_32 = pci
                    .read_config32(
                        offset as u16 + offset_of!(VirtioPciCapability64, offset_hi) as u16,
                    )
                    .await
                    .map_err(|_| zx::Status::INTERNAL)?
                    .map_err(|_| zx::Status::INTERNAL)?
                    .value as u64;
                let length_bits63_32 = pci
                    .read_config32(
                        offset as u16 + offset_of!(VirtioPciCapability64, length_hi) as u16,
                    )
                    .await
                    .map_err(|_| zx::Status::INTERNAL)?
                    .map_err(|_| zx::Status::INTERNAL)?
                    .value as u64;
                data_offset |= offset_bits63_32 << 32;
                data_length |= length_bits63_32 << 32;
            }
        }

        Ok(Some(PciCapabilityData {
            type_: virtio_capability_type,
            bar_index,
            capability_index,
            data_offset,
            data_length,
            notification_stride,
        }))
    }

    /// Maps the BAR memory region described by the capability.
    pub fn map_memory_region(
        self,
        bar_map: &PciDeviceBarMap,
    ) -> Result<MmioRegion<VmoMemory>, zx::Status> {
        let vmo = bar_map.get_vmo(self.bar_index);

        // [`VmoMapping::map()`] wants to own the VMO handle.
        let vmo = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?;

        let mmio = VmoMapping::map(self.data_offset as usize, self.data_length as usize, vmo)?;
        Ok(mmio)
    }
}

/// PCI capabilities parsed for a virtio device.
// @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
pub struct VirtioPciCapabilities {
    pub common_configuration: VirtioPciCommonConfiguration<MmioRegion<VmoMemory>>,

    pub notifications: VirtioPciNotifications,

    /// Configuration data specific to the virtio device type.
    ///
    /// [`None`] if the device does not have a device configuration capability.
    // @cite(virtio): sec="2.5" title="Device Configuration Space"
    pub device_configuration: Option<MmioRegion<VmoMemory>>,
}

impl VirtioPciCapabilities {
    pub async fn new(pci: &fidl_next::Client<fidl_pci::Device>) -> Result<Self, zx::Status> {
        let mut common_configuration: Option<PciCapabilityData> = None;
        let mut notifications: Option<PciCapabilityData> = None;
        let mut device_configuration: Option<PciCapabilityData> = None;

        let capabilities = pci
            .get_capabilities(fidl_pci::CapabilityId::Vendor)
            .await
            .map_err(|_| zx::Status::INTERNAL)?;

        let mut bar_map_builder = PciDeviceBarMapBuilder::new(pci);
        for offset in capabilities.offsets {
            let Some(capability_data) = PciCapabilityData::new(pci, offset).await? else {
                continue;
            };

            info!("PCI capability with virtio data: {:?}", capability_data);

            bar_map_builder.ensure_bar_memory_region_is_mapped(capability_data.bar_index).await?;

            // Keep the first instance of each structure type. The instances are
            // ordered from optimal (first) to least-optimal (last).
            // @cite(virtio): sec="4.1.4.1" title="Driver Requirements: Virtio Structure PCI Capabilities" q="The driver SHOULD use the first instance of each virtio structure type they can support."
            // @cite(virtio): sec="4.1.4.2" title="Virtio Structure PCI Capabilities"
            match capability_data.type_ {
                PciCapabilityType::COMMON_CONFIGURATION => {
                    if common_configuration.is_none() {
                        common_configuration = Some(capability_data);
                    }
                }
                PciCapabilityType::NOTIFICATIONS => {
                    if notifications.is_none() {
                        notifications = Some(capability_data);
                    }
                }
                PciCapabilityType::DEVICE_CONFIGURATION => {
                    if device_configuration.is_none() {
                        device_configuration = Some(capability_data);
                    }
                }
                _ => {}
            }
        }
        let bar_map = bar_map_builder.build();

        // The device must present at least one common configuration capability.
        // @cite(virtio): sec="4.1.4.3.1" title="Device Requirements: Common configuration structure layout"
        let common_configuration = common_configuration.ok_or_else(|| {
            warn!("virtio device missing required PCI capability: common configuration");
            zx::Status::IO_DATA_LOSS
        })?;

        // The device must present at least one notification capability.
        // @cite(virtio): sec="4.1.4.4.1" title="Device Requirements: Notification capability"
        let notifications = notifications.ok_or_else(|| {
            warn!("virtio device missing required PCI capability: notification");
            zx::Status::IO_DATA_LOSS
        })?;

        // `unwrap()` will not panic because [`PciCapabilitiesData`] of type
        // [`PciCapabilityType::notifications`] is guaranteed to have the
        // field set to [`Some`].
        let notification_stride = notifications.notification_stride.unwrap();

        let common_configuration = common_configuration.map_memory_region(&bar_map)?;
        let notifications = notifications.map_memory_region(&bar_map)?;
        let device_configuration = match device_configuration {
            Some(pci_capability) => Some(pci_capability.map_memory_region(&bar_map)?),
            None => None,
        };

        Ok(VirtioPciCapabilities {
            common_configuration: VirtioPciCommonConfiguration::new(common_configuration),
            notifications: VirtioPciNotifications::new(notifications, notification_stride),
            device_configuration,
        })
    }
}
