// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(doc)]
use super::capabilities::PciCapabilityData;

#[cfg(doc)]
use super::common_configuration::VirtioPciCommonConfiguration;

/// Identifies the type of data stored by virtio in a PCI capability.
///
/// The driver may only modify PCI capability structures with the type
/// [`PCI_CONFIGURATION`]. All other PCI capability structures are read-only.
// @cite(virtio): sec="4.1.4.1" title="Driver Requirements: Virtio Structure PCI Capabilities"
// @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
// @alias(virtio): theirs="cfg_type"
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PciCapabilityType(pub u8);

impl PciCapabilityType {
    /// Points to data managed by [`VirtioPciCommonConfiguration`].
    // @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
    // @alias(virtio): theirs="VIRTIO_PCI_CAP_COMMON_CFG"
    pub const COMMON_CONFIGURATION: Self = PciCapabilityType(1);

    /// Points to data managed by [`VirtioPciNotifications`].
    // @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
    // @alias(virtio): theirs="VIRTIO_PCI_CAP_NOTIFY_CFG"
    pub const NOTIFICATIONS: Self = PciCapabilityType(2);

    /// Points to the device's Interrupt Status Register (ISR).
    ///
    /// This driver does not access the ISR, because it only supports the PCI
    /// transport with Message Signalled Interrupts-Extended (MSI-X)
    /// capabilities. Mandates that the ISR is not accessed on devices where MSI-X
    /// is enabled.
    // @cite(virtio): sec="4.1.4.5" title="ISR status capability"
    // @cite(virtio): sec="4.1.4.5.2" title="Driver Requirements: ISR status capability"
    // @alias(virtio): theirs="VIRTIO_PCI_CAP_ISR_CFG"
    pub const INTERRUPT_STATUS_REGISTER: Self = PciCapabilityType(3);

    /// Points to a configuration area with data specific to each device type.
    ///
    /// The memory area described by the capability is exposed by
    /// [`VirtioPciDeviceBuilder::take_device_configuration()`].
    // @cite(virtio): sec="4.1.4" title="Virtio Structure PCI Capabilities"
    // @alias(virtio): theirs="VIRTIO_PCI_CAP_DEVICE_CFG"
    pub const DEVICE_CONFIGURATION: Self = PciCapabilityType(4);

    /// Alternative path for accessing configuration data.
    ///
    /// Not supported by this driver.
    // @cite(virtio): sec="4.1.4.9" title="PCI configuration access capability"
    // @alias(virtio): theirs="VIRTIO_PCI_CAP_PCI_CFG"
    pub const PCI_CONFIGURATION: Self = PciCapabilityType(5);

    /// Points to data continuously shared between the device and the driver.
    ///
    /// Not supported by this driver.
    // @cite(virtio): sec="4.1.4.7" title="Shared memory capability"
    // @cite(virtio): sec="2.10" title="Shared Memory Regions"
    // @alias(virtio): theirs="VIRTIO_PCI_CAP_SHARED_MEMORY_CFG"
    pub const SHARED_MEMORY: Self = PciCapabilityType(8);

    /// Vendor-specific information stored inline in the capability.
    ///
    /// Intended for vendor-specific data that facilitates debugging and reporting,
    /// and does not conflict with the functionality standardized in virtio. Not
    /// supported by this driver.
    // @cite(virtio): sec="4.1.4.8" title="Vendor data capability"
    // @alias(virtio): theirs="VIRTIO_PCI_CAP_VENDOR_CFG"
    pub const VENDOR_SPECIFIC: Self = PciCapabilityType(9);
}

impl PciCapabilityType {
    /// True iff [`PciCapabilityData`] can represent the capability's data.
    pub fn has_bar_data(self) -> bool {
        match self {
            // @cite(virtio): sec="4.1.4.3" title="Common configuration structure layout"
            Self::COMMON_CONFIGURATION => true,

            // @cite(virtio): sec="4.1.4.4" title="Notification structure layout"
            Self::NOTIFICATIONS => true,

            // @cite(virtio): sec="4.1.4.5" title="ISR status capability"
            Self::INTERRUPT_STATUS_REGISTER => true,

            // @cite(virtio): sec="4.1.4.6" title="Device-specific configuration"
            Self::DEVICE_CONFIGURATION => true,

            // The capability's data is written by the driver, so we shouldn't
            // try to parse it here.
            //
            // @cite(virtio): sec="4.1.4.9" title="PCI configuration access capability"
            Self::PCI_CONFIGURATION => false,

            // @cite(virtio): sec="4.1.4.7" title="Shared memory capability"
            Self::SHARED_MEMORY => true,

            // @cite(virtio): sec="4.1.4.8" title="Vendor data capability"
            // @alias(virtio): theirs="virtio_pci_vndr_data"
            Self::VENDOR_SPECIFIC => false,

            // Don't attempt to parse capabilities with unknown types.
            _ => false,
        }
    }
}

impl std::fmt::Debug for PciCapabilityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::COMMON_CONFIGURATION => write!(f, "COMMON_CONFIGURATION"),
            Self::NOTIFICATIONS => write!(f, "NOTIFICATIONS"),
            Self::INTERRUPT_STATUS_REGISTER => write!(f, "INTERRUPT_STATUS_REGISTER"),
            Self::DEVICE_CONFIGURATION => write!(f, "DEVICE_CONFIGURATION"),
            Self::PCI_CONFIGURATION => write!(f, "PCI_CONFIGURATION"),
            Self::SHARED_MEMORY => write!(f, "SHARED_MEMORY"),
            Self::VENDOR_SPECIFIC => write!(f, "VENDOR_SPECIFIC"),
            _ => write!(f, "UnknownPciCapabilityType({})", self.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pci_capability_type_debug() {
        assert_eq!(format!("{:?}", PciCapabilityType::PCI_CONFIGURATION), "PCI_CONFIGURATION");
        assert_eq!(format!("{:?}", PciCapabilityType(100)), "UnknownPciCapabilityType(100)");
    }
}
