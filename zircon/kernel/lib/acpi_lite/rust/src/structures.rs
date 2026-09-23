// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::fmt;

// First byte and length of the x86 BIOS read-only area, [0xe0'000, 0xff'fff].
//
// Reference: ACPI v6.3, Section 5.2.5.1
pub const K_BIOS_READ_ONLY_AREA_START: usize = 0xe0_000;
pub const K_BIOS_READ_ONLY_AREA_LENGTH: usize = 0x20_000;

#[derive(
    Copy,
    Clone,
    Eq,
    PartialEq,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(transparent)]
// ACPI signature.
//
// Signatures are 4 byte ASCII strings. We represent them as an array of bytes.
pub struct AcpiSignature(pub [u8; AcpiSignature::K_ASCII_LENGTH]);

impl AcpiSignature {
    // Length of the signature when represented as ASCII.
    pub const K_ASCII_LENGTH: usize = 4;

    pub const fn new(name: &[u8; Self::K_ASCII_LENGTH]) -> Self {
        Self(*name)
    }

    // Write the signature into the given buffer.
    //
    // Buffer must have a length of at least 5.
    pub fn write_to_buffer(&self, buffer: &mut [u8]) {
        assert!(buffer.len() > Self::K_ASCII_LENGTH);
        buffer[..Self::K_ASCII_LENGTH].copy_from_slice(&self.0);
        buffer[Self::K_ASCII_LENGTH] = 0;
    }
}

impl fmt::Debug for AcpiSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(s) = core::str::from_utf8(&self.0) {
            write!(f, "AcpiSignature({})", s)
        } else {
            write!(f, "AcpiSignature({:?})", self.0)
        }
    }
}

pub trait VariableSized {
    fn size(&self) -> usize;
}

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// Root System Description Pointer (RSDP)
//
// Reference: ACPI v6.3 Section 5.2.5.3.
pub struct AcpiRsdp {
    pub sig1: AcpiSignature, // "RSD "
    pub sig2: AcpiSignature, // "PTR "
    pub checksum: u8,
    pub oemid: [u8; 6],
    pub revision: u8,
    pub rsdt_address: u32,
}

impl AcpiRsdp {
    pub const K_SIGNATURE1: AcpiSignature = AcpiSignature(*b"RSD ");
    pub const K_SIGNATURE2: AcpiSignature = AcpiSignature(*b"PTR ");
}

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
pub struct AcpiRsdpV2 {
    pub v1: AcpiRsdp,
    pub length: u32,
    pub xsdt_address: u64,
    pub extended_checksum: u8,
    pub reserved: [u8; 3],
}

impl VariableSized for AcpiRsdpV2 {
    fn size(&self) -> usize {
        self.length as usize
    }
}

#[derive(
    Copy,
    Clone,
    Debug,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// Standard system description table header, used as the header of
// multiple structures below.
//
// Reference: ACPI v6.3 Section 5.2.6.
pub struct AcpiSdtHeader {
    pub sig: AcpiSignature,
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oemid: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: u32,
    pub creator_revision: u32,
}

impl VariableSized for AcpiSdtHeader {
    fn size(&self) -> usize {
        self.length as usize
    }
}

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// Root System Description Table (RSDT)
//
// Reference: ACPI v6.3 Section 5.2.7.
pub struct AcpiRsdt {
    pub header: AcpiSdtHeader,
}

impl AcpiRsdt {
    pub const K_SIGNATURE: AcpiSignature = AcpiSignature(*b"RSDT");

    /// # Safety
    /// The caller must ensure that `index` is within the bounds of the table
    /// payload (which is determined by the table length in the header).
    pub unsafe fn get_entry(&self, index: usize) -> u32 {
        let self_size = core::mem::size_of::<AcpiSdtHeader>();
        // SAFETY: The caller guarantees `index` is within bounds. The pointer
        // arithmetic is safe as it stays within the allocated table memory.
        // We use read_unaligned because the entry might not be aligned.
        unsafe {
            let ptr = (self as *const Self as *const u8).add(self_size) as *const u32;
            let entry_ptr = ptr.add(index);
            core::ptr::read_unaligned(entry_ptr)
        }
    }
}

impl VariableSized for AcpiRsdt {
    fn size(&self) -> usize {
        self.header.size()
    }
}

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// Extended System Description Table (XSDT)
//
// Reference: ACPI v6.3 Section 5.2.8.
pub struct AcpiXsdt {
    pub header: AcpiSdtHeader,
}

impl AcpiXsdt {
    pub const K_SIGNATURE: AcpiSignature = AcpiSignature(*b"XSDT");

    /// # Safety
    /// The caller must ensure that `index` is within the bounds of the table
    /// payload (which is determined by the table length in the header).
    pub unsafe fn get_entry(&self, index: usize) -> u64 {
        let self_size = core::mem::size_of::<AcpiSdtHeader>();
        // SAFETY: The caller guarantees `index` is within bounds. The pointer
        // arithmetic is safe as it stays within the allocated table memory.
        // We use read_unaligned because the entry might not be aligned.
        unsafe {
            let ptr = (self as *const Self as *const u8).add(self_size) as *const u64;
            let entry_ptr = ptr.add(index);
            core::ptr::read_unaligned(entry_ptr)
        }
    }
}

impl VariableSized for AcpiXsdt {
    fn size(&self) -> usize {
        self.header.size()
    }
}

#[derive(
    Copy,
    Clone,
    Debug,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// ACPI Generic Address
//
// Reference: ACPI v6.3 Section 5.2.3.2
pub struct AcpiGenericAddress {
    pub address_space_id: u8,
    pub register_bit_width: u8,
    pub register_bit_offset: u8,
    pub access_size: u8,
    pub address: u64,
}

pub const ACPI_ADDR_SPACE_MEMORY: u8 = 0;
pub const ACPI_ADDR_SPACE_IO: u8 = 1;

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// Fixed ACPI Description Table
//
// Reference: ACPI v6.3 Section 5.2.9.
pub struct AcpiFadt {
    pub header: AcpiSdtHeader,
    pub firmware_ctrl: u32,
    pub dsdt: u32,
    pub _reserved: u8,
    pub preferred_pm_profile: u8,
    pub sci_int: u16,
    pub smi_cmd: u32,
    pub acpi_enable: u8,
    pub acpi_disable: u8,
    pub s4bios_req: u8,
    pub pstate_cnt: u8,
    pub pm1a_evt_blk: u32,
    pub pm1b_evt_blk: u32,
    pub pm1a_cnt_blk: u32,
    pub pm1b_cnt_blk: u32,
    pub pm2_cnt_blk: u32,
    pub pm_tmr_blk: u32,
    pub gpe0_blk: u32,
    pub gpe1_blk: u32,
    pub pm1_evt_len: u8,
    pub pm1_cnt_len: u8,
    pub pm2_cnt_len: u8,
    pub pm_tmr_len: u8,
    pub gpe0_blk_len: u8,
    pub gpe1_blk_len: u8,
    pub gpe1_base: u8,
    pub cst_cnt: u8,
    pub p_lvl2_lat: u16,
    pub p_lvl3_lat: u16,
    pub flush_size: u16,
    pub flush_stride: u16,
    pub duty_offset: u8,
    pub duty_width: u8,
    pub day_alrm: u8,
    pub mon_alrm: u8,
    pub century: u8,
    pub iapc_boot_arch: u16,
    pub _reserved2: u8,
    pub flags: u32,
    pub reset_reg: AcpiGenericAddress,
    pub reset_value: u8,
    pub arm_boot_arch: u16,
    pub fadt_minor_version: u8,
    pub x_firmware_ctrl: u64,
    pub x_dsdt: u64,
    pub x_pm1a_evt_blk: AcpiGenericAddress,
    pub x_pm1b_evt_blk: AcpiGenericAddress,
    pub x_pm1a_cnt_blk: AcpiGenericAddress,
    pub x_pm1b_cnt_blk: AcpiGenericAddress,
    pub x_pm2_cnt_blk: AcpiGenericAddress,
    pub x_pm_tmr_blk: AcpiGenericAddress,
    pub x_gpe0_blk: AcpiGenericAddress,
    pub x_gpe1_blk: AcpiGenericAddress,
}

impl AcpiFadt {
    pub const K_SIGNATURE: AcpiSignature = AcpiSignature(*b"FACP");
}

impl VariableSized for AcpiFadt {
    fn size(&self) -> usize {
        self.header.size()
    }
}

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// Firmware ACPI Control Structure
//
// Reference: ACPI v6.3 Section 5.2.10.
pub struct AcpiFacs {
    pub sig: AcpiSignature,
    pub length: u32,
    pub hardware_signature: u32,
    pub firmware_waking_vector: u32,
    pub global_lock: u32,
    pub flags: u32,
    pub x_firmware_waking_vector: u64,
    pub version: u8,
    pub _reserved: [u8; 3],
    pub ospm_flags: u32,
    pub _reserved2: [u8; 24],
}

impl AcpiFacs {
    pub const K_SIGNATURE: AcpiSignature = AcpiSignature(*b"FACS");
}

impl VariableSized for AcpiFacs {
    fn size(&self) -> usize {
        self.length as usize
    }
}

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// Multiple APIC Description Table
//
// The table is followed by interrupt control structures, each with
// a "AcpiSubTableHeader" header.
//
// Reference: ACPI v6.3 5.2.12.
pub struct AcpiMadtTable {
    pub header: AcpiSdtHeader,
    pub local_int_controller_address: u32,
    pub flags: u32,
}

impl AcpiMadtTable {
    pub const K_SIGNATURE: AcpiSignature = AcpiSignature(*b"APIC");
}

impl VariableSized for AcpiMadtTable {
    fn size(&self) -> usize {
        self.header.size()
    }
}

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
pub struct AcpiSubTableHeader {
    pub r#type: u8,
    pub length: u8,
}

impl VariableSized for AcpiSubTableHeader {
    fn size(&self) -> usize {
        self.length as usize
    }
}

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// High Precision Event Timer Table
//
// Reference: IA-PC HPET (High Precision Event Timers) v1.0a, Section 3.2.4.
pub struct AcpiHpetTable {
    pub header: AcpiSdtHeader,
    pub id: u32,
    pub address: AcpiGenericAddress,
    pub sequence: u8,
    pub minimum_tick: u16,
    pub flags: u8,
}

impl AcpiHpetTable {
    pub const K_SIGNATURE: AcpiSignature = AcpiSignature(*b"HPET");
}

impl VariableSized for AcpiHpetTable {
    fn size(&self) -> usize {
        self.header.size()
    }
}

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// SRAT table and descriptors.
//
// Reference: ACPI v6.3 Section 5.2.16.
pub struct AcpiSratTable {
    pub header: AcpiSdtHeader,
    pub _reserved: u32,
    pub _reserved2: u64,
}

impl AcpiSratTable {
    pub const K_SIGNATURE: AcpiSignature = AcpiSignature(*b"SRAT");
}

impl VariableSized for AcpiSratTable {
    fn size(&self) -> usize {
        self.header.size()
    }
}

pub const ACPI_SRAT_TYPE_PROCESSOR_AFFINITY: u8 = 0;

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// Type 0: processor local apic/sapic affinity structure
//
// Reference: ACPI v6.3 Section 5.2.16.1.
pub struct AcpiSratProcessorAffinityEntry {
    pub header: AcpiSubTableHeader,
    pub proximity_domain_low: u8,
    pub apic_id: u8,
    pub flags: u32,
    pub sapic_eid: u8,
    pub proximity_domain_high: [u8; 3],
    pub clock_domain: u32,
}

impl AcpiSratProcessorAffinityEntry {
    pub fn proximity_domain(&self) -> u32 {
        let low = self.proximity_domain_low as u32;
        let high0 = self.proximity_domain_high[0] as u32;
        let high1 = self.proximity_domain_high[1] as u32;
        let high2 = self.proximity_domain_high[2] as u32;
        low | (high0 << 8) | (high1 << 16) | (high2 << 24)
    }
}

impl VariableSized for AcpiSratProcessorAffinityEntry {
    fn size(&self) -> usize {
        self.header.size()
    }
}

pub const ACPI_SRAT_FLAG_ENABLED: u32 = 1;

pub const ACPI_SRAT_TYPE_MEMORY_AFFINITY: u8 = 1;

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// Type 1: memory affinity structure
//
// Reference: ACPI v6.3 Section 5.2.16.2.
pub struct AcpiSratMemoryAffinityEntry {
    pub header: AcpiSubTableHeader,
    pub proximity_domain: u32,
    pub _reserved: u16,
    pub base_address_low: u32,
    pub base_address_high: u32,
    pub length_low: u32,
    pub length_high: u32,
    pub _reserved2: u32,
    pub flags: u32,
    pub _reserved3: u32,
    pub _reserved4: u32,
}

impl VariableSized for AcpiSratMemoryAffinityEntry {
    fn size(&self) -> usize {
        self.header.size()
    }
}

pub const ACPI_SRAT_TYPE_PROCESSOR_X2APIC_AFFINITY: u8 = 2;

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// Type 2: processor x2apic affinity structure
//
// Reference: ACPI v6.3 Section 5.2.16.3.
pub struct AcpiSratProcessorX2ApicAffinityEntry {
    pub header: AcpiSubTableHeader,
    pub _reserved: u16,
    pub proximity_domain: u32,
    pub x2apic_id: u32,
    pub flags: u32,
    pub clock_domain: u32,
    pub _reserved2: u32,
}

impl VariableSized for AcpiSratProcessorX2ApicAffinityEntry {
    fn size(&self) -> usize {
        self.header.size()
    }
}

pub const ACPI_MADT_TYPE_LOCAL_APIC: u8 = 0;

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// MADT entry type 0: Processor Local APIC (ACPI v6.3 Section 5.2.12.2)
pub struct AcpiMadtLocalApicEntry {
    pub header: AcpiSubTableHeader,
    pub processor_id: u8,
    pub apic_id: u8,
    pub flags: u32,
}

impl VariableSized for AcpiMadtLocalApicEntry {
    fn size(&self) -> usize {
        self.header.size()
    }
}

pub const ACPI_MADT_FLAG_ENABLED: u32 = 0x1;

pub const ACPI_MADT_TYPE_IO_APIC: u8 = 1;

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// MADT entry type 1: I/O APIC (ACPI v6.3 Section 5.2.12.3)
pub struct AcpiMadtIoApicEntry {
    pub header: AcpiSubTableHeader,
    pub io_apic_id: u8,
    pub reserved: u8,
    pub io_apic_address: u32,
    pub global_system_interrupt_base: u32,
}

impl VariableSized for AcpiMadtIoApicEntry {
    fn size(&self) -> usize {
        self.header.size()
    }
}

pub const ACPI_MADT_TYPE_INT_SOURCE_OVERRIDE: u8 = 2;

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// MADT entry type 2: Interrupt Source Override (ACPI v6.3 Section 5.2.12.5)
pub struct AcpiMadtIntSourceOverrideEntry {
    pub header: AcpiSubTableHeader,
    pub bus: u8,
    pub source: u8,
    pub global_sys_interrupt: u32,
    pub flags: u16,
}

impl VariableSized for AcpiMadtIntSourceOverrideEntry {
    fn size(&self) -> usize {
        self.header.size()
    }
}

pub const ACPI_MADT_FLAG_POLARITY_CONFORMS: u16 = 0b00;
pub const ACPI_MADT_FLAG_POLARITY_HIGH: u16 = 0b01;
pub const ACPI_MADT_FLAG_POLARITY_LOW: u16 = 0b11;
pub const ACPI_MADT_FLAG_POLARITY_MASK: u16 = 0b11;

pub const ACPI_MADT_FLAG_TRIGGER_CONFORMS: u16 = 0b0000;
pub const ACPI_MADT_FLAG_TRIGGER_EDGE: u16 = 0b0100;
pub const ACPI_MADT_FLAG_TRIGGER_LEVEL: u16 = 0b1100;
pub const ACPI_MADT_FLAG_TRIGGER_MASK: u16 = 0b1100;

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// DBG2 table
pub struct AcpiDbg2Table {
    pub header: AcpiSdtHeader,
    pub offset: u32,
    pub num_entries: u32,
}

impl AcpiDbg2Table {
    pub const K_SIGNATURE: AcpiSignature = AcpiSignature(*b"DBG2");
}

impl VariableSized for AcpiDbg2Table {
    fn size(&self) -> usize {
        self.header.size()
    }
}

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    zerocopy::IntoBytes,
)]
#[repr(C, packed)]
// DBG2 device information
pub struct AcpiDbg2Device {
    pub revision: u8,
    pub length: u16,
    pub register_count: u8,
    pub namepath_length: u16,
    pub namepath_offset: u16,
    pub oem_data_length: u16,
    pub oem_data_offset: u16,
    pub port_type: u16,
    pub port_subtype: u16,
    pub reserved: u16,
    pub base_address_offset: u16,
    pub address_size_offset: u16,
}

impl VariableSized for AcpiDbg2Device {
    fn size(&self) -> usize {
        self.length as usize
    }
}

// debug port types
pub const ACPI_DBG2_TYPE_SERIAL_PORT: u16 = 0x8000;
pub const ACPI_DBG2_TYPE_1394_PORT: u16 = 0x8001;
pub const ACPI_DBG2_TYPE_USB_PORT: u16 = 0x8002;
pub const ACPI_DBG2_TYPE_NET_PORT: u16 = 0x8003;

// debug port subtypes
pub const ACPI_DBG2_SUBTYPE_16550_COMPATIBLE: u16 = 0x0000;
pub const ACPI_DBG2_SUBTYPE_16550_SUBSET: u16 = 0x0001;
pub const ACPI_DBG2_SUBTYPE_1394_STANDARD: u16 = 0x0000;
pub const ACPI_DBG2_SUBTYPE_USB_XHCI: u16 = 0x0000;
pub const ACPI_DBG2_SUBTYPE_USB_EHCI: u16 = 0x0001;

const _: () = {
    assert!(core::mem::size_of::<AcpiSignature>() == 4);
    assert!(core::mem::align_of::<AcpiSignature>() == 1);

    assert!(core::mem::size_of::<AcpiRsdp>() == 20);
    assert!(core::mem::align_of::<AcpiRsdp>() == 1);

    assert!(core::mem::size_of::<AcpiRsdpV2>() == 36);
    assert!(core::mem::align_of::<AcpiRsdpV2>() == 1);

    assert!(core::mem::size_of::<AcpiSdtHeader>() == 36);
    assert!(core::mem::align_of::<AcpiSdtHeader>() == 1);

    assert!(core::mem::size_of::<AcpiRsdt>() == 36);
    assert!(core::mem::align_of::<AcpiRsdt>() == 1);

    assert!(core::mem::size_of::<AcpiXsdt>() == 36);
    assert!(core::mem::align_of::<AcpiXsdt>() == 1);

    assert!(core::mem::size_of::<AcpiGenericAddress>() == 12);
    assert!(core::mem::align_of::<AcpiGenericAddress>() == 1);

    assert!(core::mem::size_of::<AcpiFadt>() == 244);
    assert!(core::mem::align_of::<AcpiFadt>() == 1);

    assert!(core::mem::size_of::<AcpiFacs>() == 64);
    assert!(core::mem::align_of::<AcpiFacs>() == 1);

    assert!(core::mem::size_of::<AcpiMadtTable>() == 44);
    assert!(core::mem::align_of::<AcpiMadtTable>() == 1);

    assert!(core::mem::size_of::<AcpiSubTableHeader>() == 2);
    assert!(core::mem::align_of::<AcpiSubTableHeader>() == 1);

    assert!(core::mem::size_of::<AcpiHpetTable>() == 56);
    assert!(core::mem::align_of::<AcpiHpetTable>() == 1);

    assert!(core::mem::size_of::<AcpiSratTable>() == 48);
    assert!(core::mem::align_of::<AcpiSratTable>() == 1);

    assert!(core::mem::size_of::<AcpiSratProcessorAffinityEntry>() == 16);
    assert!(core::mem::align_of::<AcpiSratProcessorAffinityEntry>() == 1);

    assert!(core::mem::size_of::<AcpiSratMemoryAffinityEntry>() == 40);
    assert!(core::mem::align_of::<AcpiSratMemoryAffinityEntry>() == 1);

    assert!(core::mem::size_of::<AcpiSratProcessorX2ApicAffinityEntry>() == 24);
    assert!(core::mem::align_of::<AcpiSratProcessorX2ApicAffinityEntry>() == 1);

    assert!(core::mem::size_of::<AcpiMadtLocalApicEntry>() == 8);
    assert!(core::mem::align_of::<AcpiMadtLocalApicEntry>() == 1);

    assert!(core::mem::size_of::<AcpiMadtIoApicEntry>() == 12);
    assert!(core::mem::align_of::<AcpiMadtIoApicEntry>() == 1);

    assert!(core::mem::size_of::<AcpiMadtIntSourceOverrideEntry>() == 10);
    assert!(core::mem::align_of::<AcpiMadtIntSourceOverrideEntry>() == 1);

    assert!(core::mem::size_of::<AcpiDbg2Table>() == 44);
    assert!(core::mem::align_of::<AcpiDbg2Table>() == 1);

    assert!(core::mem::size_of::<AcpiDbg2Device>() == 22);
    assert!(core::mem::align_of::<AcpiDbg2Device>() == 1);
};
