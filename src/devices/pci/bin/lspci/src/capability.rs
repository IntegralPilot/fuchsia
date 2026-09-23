// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::util::is_set;
use bitfield::bitfield;
use fidl_fuchsia_hardware_pci::{
    Capability as FidlCapability, ExtendedCapability as FidlExtCapability,
};
use std::fmt;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Ref};

// Capability types are documented in PCI Local Bus Specification v3.0 Appendix H
enum CapabilityType {
    Null,
    PciPowerManagement,
    Agp,
    VitalProductData,
    SlotIdentification,
    Msi,
    CompactPciHotSwap,
    PciX,
    HyperTransport,
    Vendor,
    DebugPort,
    CompactPciCrc,
    PciHotplug,
    PciBridgeSubsystemVendorId,
    Agp8x,
    SecureDevice,
    PciExpress,
    MsiX,
    SataDataNdxCfg,
    AdvancedFeatures,
    EnhancedAllocation,
    FlatteningPortalBridge,
    Unknown(u8),
}

impl From<u8> for CapabilityType {
    fn from(value: u8) -> Self {
        match value {
            0x00 => CapabilityType::Null,
            0x01 => CapabilityType::PciPowerManagement,
            0x02 => CapabilityType::Agp,
            0x03 => CapabilityType::VitalProductData,
            0x04 => CapabilityType::SlotIdentification,
            0x05 => CapabilityType::Msi,
            0x06 => CapabilityType::CompactPciHotSwap,
            0x07 => CapabilityType::PciX,
            0x08 => CapabilityType::HyperTransport,
            0x09 => CapabilityType::Vendor,
            0x0a => CapabilityType::DebugPort,
            0x0b => CapabilityType::CompactPciCrc,
            0x0c => CapabilityType::PciHotplug,
            0x0d => CapabilityType::PciBridgeSubsystemVendorId,
            0x0e => CapabilityType::Agp8x,
            0x0f => CapabilityType::SecureDevice,
            0x10 => CapabilityType::PciExpress,
            0x11 => CapabilityType::MsiX,
            0x12 => CapabilityType::SataDataNdxCfg,
            0x13 => CapabilityType::AdvancedFeatures,
            0x14 => CapabilityType::EnhancedAllocation,
            0x15 => CapabilityType::FlatteningPortalBridge,
            _ => CapabilityType::Unknown(value),
        }
    }
}

pub struct Capability<'a> {
    offset: usize,
    config: &'a [u8],
    cap_type: CapabilityType,
}

impl<'a> fmt::Display for Capability<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Capabilities: [{:#2x}] ", self.offset)?;
        match self.cap_type {
            CapabilityType::Null => write!(f, "Null"),
            CapabilityType::PciPowerManagement => write!(f, "PCI Power Management"),
            CapabilityType::Agp => write!(f, "AGP"),
            CapabilityType::VitalProductData => write!(f, "Vital Product Data"),
            CapabilityType::SlotIdentification => write!(f, "Slot Identification"),
            CapabilityType::Msi => self.msi(f),
            CapabilityType::CompactPciHotSwap => write!(f, "CompactPCI Hotswap"),
            CapabilityType::PciX => write!(f, "PCI-X"),
            CapabilityType::HyperTransport => write!(f, "HyperTransport"),
            CapabilityType::Vendor => self.vendor(f),
            CapabilityType::DebugPort => write!(f, "Debug Port"),
            CapabilityType::CompactPciCrc => write!(f, "CompactPCI CRC"),
            CapabilityType::PciHotplug => write!(f, "PCI Hotplug"),
            CapabilityType::PciBridgeSubsystemVendorId => write!(f, "PCI Bridge Subsystem VID"),
            CapabilityType::Agp8x => write!(f, "AGP 8x"),
            CapabilityType::SecureDevice => write!(f, "Secure Device"),
            CapabilityType::PciExpress => self.pci_express(f),
            CapabilityType::MsiX => self.msi_x(f),
            CapabilityType::SataDataNdxCfg => write!(f, "SATA Data Ndx Config"),
            CapabilityType::AdvancedFeatures => write!(f, "Advanced Features"),
            CapabilityType::EnhancedAllocation => write!(f, "Enhanced Allocations"),
            CapabilityType::FlatteningPortalBridge => write!(f, "Flattening Portal Bridge"),
            CapabilityType::Unknown(id) => write!(f, "Unknown Capability (id = {:#2x})", id),
        }
    }
}

impl<'a> Capability<'a> {
    pub fn new(capability: &'a FidlCapability, config: &'a [u8]) -> Self {
        Capability {
            offset: capability.offset as usize,
            config,
            cap_type: CapabilityType::from(capability.id),
        }
    }

    fn msi(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // MSI: Enable+ Count=1/1 Maskable- 64bit+
        // Address: 00000000fee00698  Data: 0000
        let control = MsiControl(
            ((self.config[self.offset + 3] as u16) << 8) | self.config[self.offset + 2] as u16,
        );
        write!(
            f,
            "MSI: Enable{} Count={}/{} Maskable{} 64bit{}\n",
            is_set(control.enable()),
            msi_mms_to_value(control.mms_enabled()),
            msi_mms_to_value(control.mms_capable()),
            is_set(control.pvm_capable()),
            is_set(control.can_be_64bit())
        )?;

        if control.can_be_64bit() {
            let (msi, _) = Ref::<_, Msi64Capability>::from_prefix(
                &self.config[self.offset..self.config.len()],
            )
            .unwrap();
            write!(
                f,
                "\t\tAddress: {:#010x} {:#08x} Data: {:#06x}",
                { msi.address_upper },
                { msi.address },
                { msi.data }
            )
        } else {
            let (msi, _) = Ref::<_, Msi32Capability>::from_prefix(
                &self.config[self.offset..self.config.len()],
            )
            .unwrap();
            write!(f, "\t\tAddress: {:#010x} Data: {:#06x}", { msi.address }, { msi.data })
        }
    }

    fn msi_x(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (msix, _) =
            Ref::<_, MsixCapability>::from_prefix(&self.config[self.offset..self.config.len()])
                .unwrap();
        let control = MsixControl(msix.control);
        let table = MsixBarField(msix.table);
        let pba = MsixBarField(msix.pba);
        write!(
            f,
            "MSI-X: Enable{} Count={} Masked{} TBIR={} TOff={:#x} PBIR={} POff={:#x}",
            is_set(control.enable()),
            control.table_size() + 1,
            is_set(control.function_mask()),
            table.bir(),
            table.offset() << 3,
            pba.bir(),
            pba.offset() << 3,
        )
    }

    fn pci_express(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (pcie, _) = Ref::<_, PciExpressCapability>::from_prefix(
            &self.config[self.offset..self.config.len()],
        )
        .unwrap();
        // PCIe Base Specification v4 section 7.5.3
        let pcie_capabilities = PcieCapabilitiesField(pcie.pcie_capabilities);
        let dev_type = PcieDevicePortType::from(pcie_capabilities.device_type());
        let slot = match dev_type {
            PcieDevicePortType::PCIEEP
            | PcieDevicePortType::LPCIEEP
            | PcieDevicePortType::RCIEP
            | PcieDevicePortType::RCEC => String::from(""),
            _ => format!(" (Slot{})", is_set(pcie_capabilities.slot_implemented())),
        };
        write!(
            f,
            "Express (v{}) {}{}, MSI {:#02x}",
            pcie_capabilities.version(),
            dev_type,
            slot,
            pcie_capabilities.irq_number()
        )
    }

    fn vendor(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Vendor Specific Information: Len={:#2x}", self.config[self.offset + 2])
    }
}

fn msi_mms_to_value(mms: u16) -> u8 {
    match mms {
        0b000 => 1,
        0b001 => 2,
        0b010 => 4,
        0b011 => 8,
        0b100 => 16,
        0b101 => 32,
        _ => 0,
    }
}

bitfield! {
    struct MsiControl(u16);
    enable, _: 0;
    mms_capable, _: 3, 1;
    mms_enabled, _: 6, 4;
    can_be_64bit, _: 7;
    pvm_capable, _: 8;
    _reserved, _: 15, 9;
}

#[derive(IntoBytes, KnownLayout, FromBytes, Immutable)]
#[repr(C, packed)]
struct Msi32Capability {
    id: u8,
    next: u8,
    control: u16,
    address: u32,
    data: u16,
}

#[derive(IntoBytes, KnownLayout, FromBytes, Immutable)]
#[repr(C, packed)]
struct Msi64Capability {
    id: u8,
    next: u8,
    control: u16,
    address: u32,
    address_upper: u32,
    data: u16,
}

#[derive(IntoBytes, KnownLayout, FromBytes, Immutable)]
#[repr(C, packed)]
struct MsixCapability {
    id: u8,
    next: u8,
    control: u16,
    table: u32,
    pba: u32,
}

bitfield! {
    pub struct MsixControl(u16);
    table_size, _: 10, 0;
    _reserved, _: 13, 11;
    function_mask, _: 14;
    enable, _: 15;
}

bitfield! {
    pub struct MsixBarField(u32);
    bir, _: 2, 0;
    offset, _: 31, 3;
}

bitfield! {
    pub struct PcieCapabilitiesField(u16);
    version, _: 3, 0;
    device_type, _: 7, 4;
    slot_implemented, _: 8;
    irq_number, _: 13, 9;
    _reserved, _: 15, 14;
}

// PCIe Base Specification Table 7-17: PCI Express Capabilities Register
enum PcieDevicePortType {
    PCIEEP,
    LPCIEEP,
    RCIEP,
    RCEC,
    RPPCIERC,
    UPPCIES,
    DPPCIES,
    PCIE2PCIB,
    PCI2PCIEB,
    Unknown(u16),
}

impl From<u16> for PcieDevicePortType {
    fn from(value: u16) -> Self {
        match value {
            0b0000 => PcieDevicePortType::PCIEEP,
            0b0001 => PcieDevicePortType::LPCIEEP,
            0b1001 => PcieDevicePortType::RCIEP,
            0b1010 => PcieDevicePortType::RCEC,
            0b0100 => PcieDevicePortType::RPPCIERC,
            0b0101 => PcieDevicePortType::UPPCIES,
            0b0110 => PcieDevicePortType::DPPCIES,
            0b0111 => PcieDevicePortType::PCIE2PCIB,
            0b1000 => PcieDevicePortType::PCI2PCIEB,
            _ => PcieDevicePortType::Unknown(value),
        }
    }
}

impl fmt::Display for PcieDevicePortType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                PcieDevicePortType::PCIEEP => "PCI Express Endpoint",
                PcieDevicePortType::LPCIEEP => "Legacy PCI Express Endpoint",
                PcieDevicePortType::RCIEP => "Root Complex Integrated Endpoint",
                PcieDevicePortType::RCEC => "Root Complex Event Collector",
                PcieDevicePortType::RPPCIERC => "Root Port of PCI Express Root Complex",
                PcieDevicePortType::UPPCIES => "Upstream Port of PCI Express Switch",
                PcieDevicePortType::DPPCIES => "Downstream Port of PCI Express Switch",
                PcieDevicePortType::PCIE2PCIB => "PCI Express to PCI/PCI-X Bridge",
                PcieDevicePortType::PCI2PCIEB => "PCI/PCI-X to PCI Express Bridge",
                PcieDevicePortType::Unknown(x) => return write!(f, "Unknown PCIe Type ({:#x})", x),
            }
        )
    }
}

#[derive(IntoBytes, KnownLayout, FromBytes, Immutable)]
#[repr(C, packed)]
struct PciExpressCapability {
    id: u8,
    next: u8,
    pcie_capabilities: u16,
    device_capabilities: u32,
    device_control: u16,
    device_status: u16,
}

// PCIe Extended Capability IDs.
// PCIe Base Specification rev4, chapter 7.6.
enum ExtendedCapabilityType {
    Null,
    AdvancedErrorReporting,
    VirtualChannelNoMfvc,
    DeviceSerialNumber,
    PowerBudgeting,
    RootComplexLinkDeclaration,
    RootComplexInternalLinkControl,
    RootComplexEventCollectorEndpointAssociation,
    MultiFunctionVirtualChannel,
    VirtualChannel,
    Rcrb,
    Vendor,
    Cac,
    Acs,
    Ari,
    Ats,
    SrIov,
    MrIov,
    Multicast,
    Pri,
    EnhancedAllocation,
    ResizableBar,
    DynamicPowerAllocation,
    Tph,
    LatencyToleranceReporting,
    SecondaryPciExpress,
    Pmux,
    Pasid,
    Lnr,
    Dpc,
    L1pmSubstates,
    PrecisionTimeMeasurement,
    Mpcie,
    FrsQueueing,
    ReadinessTimeReporting,
    DesignatedVendor,
    VfResizableBar,
    DataLinkFeature,
    PhysicalLayer16,
    LaneMarginingAtReceiver,
    HierarchyId,
    NativePcieEnclosure,
    PhysicalLayer32,
    AlternateProtocol,
    SystemFirmwareIntermediary,
    Unknown(u16),
}

impl From<u16> for ExtendedCapabilityType {
    fn from(value: u16) -> Self {
        match value {
            0x00 => ExtendedCapabilityType::Null,
            0x01 => ExtendedCapabilityType::AdvancedErrorReporting,
            0x02 => ExtendedCapabilityType::VirtualChannelNoMfvc,
            0x03 => ExtendedCapabilityType::DeviceSerialNumber,
            0x04 => ExtendedCapabilityType::PowerBudgeting,
            0x05 => ExtendedCapabilityType::RootComplexLinkDeclaration,
            0x06 => ExtendedCapabilityType::RootComplexInternalLinkControl,
            0x07 => ExtendedCapabilityType::RootComplexEventCollectorEndpointAssociation,
            0x08 => ExtendedCapabilityType::MultiFunctionVirtualChannel,
            0x09 => ExtendedCapabilityType::VirtualChannel,
            0x0a => ExtendedCapabilityType::Rcrb,
            0x0b => ExtendedCapabilityType::Vendor,
            0x0c => ExtendedCapabilityType::Cac,
            0x0d => ExtendedCapabilityType::Acs,
            0x0e => ExtendedCapabilityType::Ari,
            0x0f => ExtendedCapabilityType::Ats,
            0x10 => ExtendedCapabilityType::SrIov,
            0x11 => ExtendedCapabilityType::MrIov,
            0x12 => ExtendedCapabilityType::Multicast,
            0x13 => ExtendedCapabilityType::Pri,
            0x14 => ExtendedCapabilityType::EnhancedAllocation,
            0x15 => ExtendedCapabilityType::ResizableBar,
            0x16 => ExtendedCapabilityType::DynamicPowerAllocation,
            0x17 => ExtendedCapabilityType::Tph,
            0x18 => ExtendedCapabilityType::LatencyToleranceReporting,
            0x19 => ExtendedCapabilityType::SecondaryPciExpress,
            0x1a => ExtendedCapabilityType::Pmux,
            0x1b => ExtendedCapabilityType::Pasid,
            0x1c => ExtendedCapabilityType::Lnr,
            0x1d => ExtendedCapabilityType::Dpc,
            0x1e => ExtendedCapabilityType::L1pmSubstates,
            0x1f => ExtendedCapabilityType::PrecisionTimeMeasurement,
            0x20 => ExtendedCapabilityType::Mpcie,
            0x21 => ExtendedCapabilityType::FrsQueueing,
            0x22 => ExtendedCapabilityType::ReadinessTimeReporting,
            0x23 => ExtendedCapabilityType::DesignatedVendor,
            0x24 => ExtendedCapabilityType::VfResizableBar,
            0x25 => ExtendedCapabilityType::DataLinkFeature,
            0x26 => ExtendedCapabilityType::PhysicalLayer16,
            0x27 => ExtendedCapabilityType::LaneMarginingAtReceiver,
            0x28 => ExtendedCapabilityType::HierarchyId,
            0x29 => ExtendedCapabilityType::NativePcieEnclosure,
            0x2a => ExtendedCapabilityType::PhysicalLayer32,
            0x2b => ExtendedCapabilityType::AlternateProtocol,
            0x2c => ExtendedCapabilityType::SystemFirmwareIntermediary,
            _ => ExtendedCapabilityType::Unknown(value),
        }
    }
}

pub struct ExtendedCapability {
    offset: usize,
    cap_type: ExtendedCapabilityType,
}

impl ExtendedCapability {
    pub fn new(capability: &FidlExtCapability) -> Self {
        ExtendedCapability {
            offset: capability.offset as usize,
            cap_type: ExtendedCapabilityType::from(capability.id),
        }
    }
}

impl fmt::Display for ExtendedCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Capabilities: [0x{:03x}] ", self.offset)?;
        match self.cap_type {
            ExtendedCapabilityType::Null => write!(f, "Null"),
            ExtendedCapabilityType::AdvancedErrorReporting => write!(f, "Advanced Error Reporting"),
            ExtendedCapabilityType::VirtualChannelNoMfvc => write!(f, "Virtual Channel (no MFVC)"),
            ExtendedCapabilityType::DeviceSerialNumber => write!(f, "Device Serial Number"),
            ExtendedCapabilityType::PowerBudgeting => write!(f, "Power Budgeting"),
            ExtendedCapabilityType::RootComplexLinkDeclaration => {
                write!(f, "Root Complex Link Declaration")
            }
            ExtendedCapabilityType::RootComplexInternalLinkControl => {
                write!(f, "Root Complex Internal Link Control")
            }
            ExtendedCapabilityType::RootComplexEventCollectorEndpointAssociation => {
                write!(f, "Root Complex Event Collector Endpoint Association")
            }
            ExtendedCapabilityType::MultiFunctionVirtualChannel => {
                write!(f, "Multi-Function Virtual Channel")
            }
            ExtendedCapabilityType::VirtualChannel => write!(f, "Virtual Channel"),
            ExtendedCapabilityType::Rcrb => write!(f, "RCRB"),
            ExtendedCapabilityType::Vendor => write!(f, "Vendor Specific Option"),
            ExtendedCapabilityType::Cac => write!(f, "CAC"),
            ExtendedCapabilityType::Acs => write!(f, "Access Control Services"),
            ExtendedCapabilityType::Ari => write!(f, "Alternative Routing-ID Interpretation (ARI)"),
            ExtendedCapabilityType::Ats => write!(f, "Address Translation Services (ATS)"),
            ExtendedCapabilityType::SrIov => write!(f, "Single Root I/O Virtualization (SR-IOV)"),
            ExtendedCapabilityType::MrIov => write!(f, "Multi-Root I/O Virtualization (MR-IOV)"),
            ExtendedCapabilityType::Multicast => write!(f, "Multicast"),
            ExtendedCapabilityType::Pri => write!(f, "Page Request Interface (PRI)"),
            ExtendedCapabilityType::EnhancedAllocation => write!(f, "Enhanced Allocation"),
            ExtendedCapabilityType::ResizableBar => write!(f, "Resizable BAR"),
            ExtendedCapabilityType::DynamicPowerAllocation => write!(f, "Dynamic Power Allocation"),
            ExtendedCapabilityType::Tph => write!(f, "TLP Processing Hints (TPH)"),
            ExtendedCapabilityType::LatencyToleranceReporting => {
                write!(f, "Latency Tolerance Reporting")
            }
            ExtendedCapabilityType::SecondaryPciExpress => write!(f, "Secondary PCI Express"),
            ExtendedCapabilityType::Pmux => write!(f, "Protocol Multiplexing (PMUX)"),
            ExtendedCapabilityType::Pasid => write!(f, "Process Address Space ID (PASID)"),
            ExtendedCapabilityType::Lnr => write!(f, "LN Requester (LNR)"),
            ExtendedCapabilityType::Dpc => write!(f, "Downstream Port Containment (DPC)"),
            ExtendedCapabilityType::L1pmSubstates => write!(f, "L1 PM Substates"),
            ExtendedCapabilityType::PrecisionTimeMeasurement => {
                write!(f, "Precision Time Measurement (PTM)")
            }
            ExtendedCapabilityType::Mpcie => write!(f, "M-PCIe"),
            ExtendedCapabilityType::FrsQueueing => write!(f, "FRS Queueing"),
            ExtendedCapabilityType::ReadinessTimeReporting => write!(f, "Readiness Time Reporting"),
            ExtendedCapabilityType::DesignatedVendor => write!(f, "Designated Vendor-Specific"),
            ExtendedCapabilityType::VfResizableBar => write!(f, "VF Resizable BAR"),
            ExtendedCapabilityType::DataLinkFeature => write!(f, "Data Link Feature"),
            ExtendedCapabilityType::PhysicalLayer16 => write!(f, "Physical Layer 16.0 GT/s"),
            ExtendedCapabilityType::LaneMarginingAtReceiver => {
                write!(f, "Lane Margining at Receiver")
            }
            ExtendedCapabilityType::HierarchyId => write!(f, "Hierarchy ID"),
            ExtendedCapabilityType::NativePcieEnclosure => write!(f, "Native PCIe Enclosure"),
            ExtendedCapabilityType::PhysicalLayer32 => write!(f, "Physical Layer 32.0 GT/s"),
            ExtendedCapabilityType::AlternateProtocol => write!(f, "Alternate Protocol"),
            ExtendedCapabilityType::SystemFirmwareIntermediary => {
                write!(f, "System Firmware Intermediary")
            }
            ExtendedCapabilityType::Unknown(id) => {
                write!(f, "Unknown Extended Capability (id = {:#04x})", id)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extended_capability_display() {
        let cap = FidlExtCapability { id: 0x0001, offset: 0x0100 };
        let ext_cap = ExtendedCapability::new(&cap);
        assert_eq!(format!("{}", ext_cap), "Capabilities: [0x100] Advanced Error Reporting");

        let cap_zero = FidlExtCapability { id: 0x0001, offset: 0x0 };
        let ext_cap_zero = ExtendedCapability::new(&cap_zero);
        assert_eq!(format!("{}", ext_cap_zero), "Capabilities: [0x000] Advanced Error Reporting");

        let cap_unknown = FidlExtCapability { id: 0xabcd, offset: 0x0200 };
        let ext_cap_unknown = ExtendedCapability::new(&cap_unknown);
        assert_eq!(
            format!("{}", ext_cap_unknown),
            "Capabilities: [0x200] Unknown Extended Capability (id = 0xabcd)"
        );
    }
}
