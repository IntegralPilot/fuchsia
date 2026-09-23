// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::structures::{
    AcpiDbg2Table, AcpiFacs, AcpiFadt, AcpiHpetTable, AcpiMadtTable, AcpiRsdp, AcpiRsdpV2,
    AcpiRsdt, AcpiSdtHeader, AcpiSignature, AcpiSratTable, AcpiXsdt, VariableSized,
};
use kprint::{kprint, kprintln};
use zx_status::Status;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use crate::structures::{K_BIOS_READ_ONLY_AREA_LENGTH, K_BIOS_READ_ONLY_AREA_START};

// A PhysMemReader translates physical addresses (such as those in the ACPI tables and the RSDT
// itself) into pointers directly readable by the acpi_lite library.
pub trait PhysMemReader: Sync {
    fn phys_to_slice(&self, phys: usize, length: usize) -> Result<&[u8], Status>;
}

// Abstract interface for reading ACPI tables.
pub trait AcpiParserInterface {
    // Get the number of tables.
    fn num_tables(&self) -> usize;

    // Return the i'th table. Return None if the index is out of range.
    //
    // If the return value is Some, it is guaranteed that the returned
    // pointer |p| points to memory at least |p.length| bytes long.
    fn get_table_at_index(&self, index: usize) -> Option<&AcpiSdtHeader>;
}

pub trait AcpiTable: VariableSized {
    const SIGNATURE: AcpiSignature;
}

impl AcpiTable for AcpiRsdt {
    const SIGNATURE: AcpiSignature = Self::K_SIGNATURE;
}
impl AcpiTable for AcpiXsdt {
    const SIGNATURE: AcpiSignature = Self::K_SIGNATURE;
}
impl AcpiTable for AcpiFadt {
    const SIGNATURE: AcpiSignature = Self::K_SIGNATURE;
}
impl AcpiTable for AcpiFacs {
    const SIGNATURE: AcpiSignature = Self::K_SIGNATURE;
}
impl AcpiTable for AcpiMadtTable {
    const SIGNATURE: AcpiSignature = Self::K_SIGNATURE;
}
impl AcpiTable for AcpiHpetTable {
    const SIGNATURE: AcpiSignature = Self::K_SIGNATURE;
}
impl AcpiTable for AcpiSratTable {
    const SIGNATURE: AcpiSignature = Self::K_SIGNATURE;
}
impl AcpiTable for AcpiDbg2Table {
    const SIGNATURE: AcpiSignature = Self::K_SIGNATURE;
}

// Get the first table matching the given signature. Return None if no table found.
pub fn get_table_by_signature(
    parser: &dyn AcpiParserInterface,
    sig: AcpiSignature,
) -> Option<&AcpiSdtHeader> {
    let num_tables = parser.num_tables();
    for i in 0..num_tables {
        let header = match parser.get_table_at_index(i) {
            Some(h) => h,
            None => continue,
        };
        if sig != header.sig {
            continue;
        }
        // SAFETY: header is a valid reference, and the parser guarantees it is
        // backed by at least `header.size()` bytes.
        let slice = unsafe {
            core::slice::from_raw_parts(header as *const AcpiSdtHeader as *const u8, header.size())
        };
        if !acpi_checksum_valid(slice) {
            continue;
        }
        return Some(header);
    }
    None
}

// Get the first table of the given type. Return None if no table found, or the
// table is invalid.
pub fn get_table_by_type<T>(parser: &dyn AcpiParserInterface) -> Option<&T>
where
    T: AcpiTable + 'static,
{
    let header = get_table_by_signature(parser, T::SIGNATURE)?;
    // TODO(https://fxbug.dev/42170568): Change this check so that tables with optional entries can be
    // found on platforms that do not have them
    if header.size() < core::mem::size_of::<T>() {
        return None;
    }
    // SAFETY: header is a valid reference, we verified the backing memory is
    // large enough for T, and T is packed (alignment 1).
    unsafe { Some(&*(header as *const AcpiSdtHeader as *const T)) }
}

// Calculate a checksum of the given range of memory.
pub fn acpi_checksum(buf: &[u8]) -> u8 {
    let mut c: u8 = 0;
    for &b in buf {
        c = c.wrapping_add(b);
    }
    c.wrapping_neg()
}

// Ensure the checksum of the given block of code is valid.
pub fn acpi_checksum_valid(buf: &[u8]) -> bool {
    #[cfg(fuzz)]
    {
        let _ = acpi_checksum(buf);
        true
    }
    #[cfg(not(fuzz))]
    {
        acpi_checksum(buf) == 0
    }
}

// Map a variable-length structure into memory.
//
// Perform a two-phase PhysToPtr conversion:
//
//   1. We first read a fixed-sized header.
//   2. We next determine the length of the structure by reading the fields.
//   3. We finally map in the full size of the structure.
//
// This allows us to handle the common use-case where the number of bytes that need
// to be accessed at a particular address cannot be determined until we first read
// a header at that address.
/// # Safety
/// The caller must ensure that `phys` points to a valid ACPI structure of type `T`
/// in physical memory, and that the memory remains valid for `'a`.
fn map_structure<T>(reader: &dyn PhysMemReader, phys: usize) -> Result<&T, Status>
where
    T: VariableSized + zerocopy::FromBytes + zerocopy::Immutable + zerocopy::KnownLayout,
{
    let bytes = reader.phys_to_slice(phys, core::mem::size_of::<T>())?;
    let r = zerocopy::Ref::<_, T>::from_bytes(bytes).map_err(|_| Status::IO_DATA_INTEGRITY)?;
    let header = zerocopy::Ref::into_ref(r);
    let size = header.size();
    if size < core::mem::size_of::<T>() {
        return Err(Status::IO_DATA_INTEGRITY);
    }
    let bytes = reader.phys_to_slice(phys, size)?;
    let prefix = &bytes[..core::mem::size_of::<T>()];
    let r = zerocopy::Ref::<_, T>::from_bytes(prefix).map_err(|_| Status::IO_DATA_INTEGRITY)?;
    Ok(zerocopy::Ref::into_ref(r))
}

// Verify the RSDP signature and validate the checksum on the V1 header.
fn validate_rsdp(rsdp: &AcpiRsdp) -> bool {
    if rsdp.sig1 != AcpiRsdp::K_SIGNATURE1 || rsdp.sig2 != AcpiRsdp::K_SIGNATURE2 {
        return false;
    }
    let slice = zerocopy::IntoBytes::as_bytes(rsdp);
    acpi_checksum_valid(slice)
}

struct RootSystemTableDetails {
    rsdp_address: usize,
    rsdt_address: u32,
    xsdt_address: u64,
}

fn parse_rsdp(
    reader: &dyn PhysMemReader,
    rsdp_pa: usize,
) -> Result<RootSystemTableDetails, Status> {
    // Read the header.
    let maybe_rsdp_v1 = reader.phys_to_slice(rsdp_pa, core::mem::size_of::<AcpiRsdp>())?;
    let r = zerocopy::Ref::<_, AcpiRsdp>::from_bytes(maybe_rsdp_v1)
        .map_err(|_| Status::IO_DATA_INTEGRITY)?;
    let rsdp_v1 = zerocopy::Ref::into_ref(r);

    // Verify the V1 header details.
    if !validate_rsdp(rsdp_v1) {
        return Err(Status::NOT_FOUND);
    }

    // If this is just a V1 RSDP, parse it and finish up.
    let revision = rsdp_v1.revision;
    let rsdt_address = rsdp_v1.rsdt_address;
    if revision < 2 {
        return Ok(RootSystemTableDetails { rsdp_address: rsdp_pa, rsdt_address, xsdt_address: 0 });
    }

    // Try and map the larger V2 structure.
    let rsdp_v2 = map_structure::<AcpiRsdpV2>(reader, rsdp_pa)?;
    let rsdp_v2_slice = reader.phys_to_slice(rsdp_pa, rsdp_v2.size())?;
    // Validate the checksum of the larger structure.
    if !acpi_checksum_valid(rsdp_v2_slice) {
        return Err(Status::NOT_FOUND);
    }

    let rsdt_address = rsdp_v2.v1.rsdt_address;
    let xsdt_address = rsdp_v2.xsdt_address;
    Ok(RootSystemTableDetails { rsdp_address: rsdp_pa, rsdt_address, xsdt_address })
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
// Search for a valid RSDP in the BIOS read-only memory space in [0xe0000..0xfffff],
// on 16 byte boundaries.
//
// Return 0 if no RSDP found.
//
// Reference: ACPI v6.3, Section 5.2.5.1
fn find_rsdp_pc(reader: &dyn PhysMemReader) -> Result<usize, Status> {
    let bios_section =
        reader.phys_to_slice(K_BIOS_READ_ONLY_AREA_START, K_BIOS_READ_ONLY_AREA_LENGTH)?;
    let rsdp_size = core::mem::size_of::<AcpiRsdp>();
    if bios_section.len() < rsdp_size {
        return Err(Status::NOT_FOUND);
    }
    for offset in (0..=K_BIOS_READ_ONLY_AREA_LENGTH - rsdp_size).step_by(16) {
        let slice = &bios_section[offset..offset + rsdp_size];
        let r = zerocopy::Ref::<_, AcpiRsdp>::from_bytes(slice).map_err(|_| Status::NOT_FOUND)?;
        let rsdp = zerocopy::Ref::into_ref(r);
        if validate_rsdp(rsdp) {
            return Ok(K_BIOS_READ_ONLY_AREA_START + offset);
        }
    }
    Err(Status::NOT_FOUND)
}

fn find_root_tables(
    physmem_reader: &dyn PhysMemReader,
    rsdp_pa: usize,
) -> Result<RootSystemTableDetails, Status> {
    // If the user gave us an explicit RSDP, just use that directly.
    if rsdp_pa != 0 {
        return parse_rsdp(physmem_reader, rsdp_pa);
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if let Ok(addr) = find_rsdp_pc(physmem_reader) {
            kprintln!("ACPI LITE: Found RSDP at physical address 0x{:x}.", addr);
            return parse_rsdp(physmem_reader, addr);
        }
        kprintln!("ACPI LITE: Couldn't find ACPI RSDP in BIOS area");
    }

    Err(Status::NOT_FOUND)
}

// Validate the RSDT table.
pub fn validate_rsdt(
    reader: &dyn PhysMemReader,
    rsdt_pa: usize,
) -> Result<(&AcpiRsdt, usize), Status> {
    // Map in the RSDT.
    let rsdt = map_structure::<AcpiRsdt>(reader, rsdt_pa)?;
    // Ensure we have an RSDT signature.
    if rsdt.header.sig != AcpiRsdt::K_SIGNATURE {
        return Err(Status::NOT_FOUND);
    }
    let length = rsdt.header.size();
    let slice = reader.phys_to_slice(rsdt_pa, length)?;
    // Validate checksum.
    if !acpi_checksum_valid(slice) {
        return Err(Status::IO_DATA_INTEGRITY);
    }
    // Ensure this is a revision we understand.
    if rsdt.header.revision != 1 {
        return Err(Status::NOT_SUPPORTED);
    }
    let num_tables = (length - core::mem::size_of::<AcpiSdtHeader>()) / 4;
    Ok((rsdt, num_tables))
}

// Validate the XSDT table.
pub fn validate_xsdt(
    reader: &dyn PhysMemReader,
    xsdt_pa: usize,
) -> Result<(&AcpiXsdt, usize), Status> {
    // Map in the XSDT.
    let xsdt = map_structure::<AcpiXsdt>(reader, xsdt_pa)?;
    // Ensure we have an XSDT signature.
    if xsdt.header.sig != AcpiXsdt::K_SIGNATURE {
        return Err(Status::NOT_FOUND);
    }
    let length = xsdt.header.size();
    let slice = reader.phys_to_slice(xsdt_pa, length)?;
    // Validate checksum.
    if !acpi_checksum_valid(slice) {
        return Err(Status::IO_DATA_INTEGRITY);
    }
    // Ensure this is a revision we understand.
    if xsdt.header.revision != 1 {
        return Err(Status::NOT_SUPPORTED);
    }
    let num_tables = (length - core::mem::size_of::<AcpiSdtHeader>()) / 8;
    Ok((xsdt, num_tables))
}

// Functionality for reading ACPI tables.
pub struct AcpiParser<'a> {
    reader: &'a dyn PhysMemReader,
    rsdt: Option<&'a AcpiRsdt>,
    xsdt: Option<&'a AcpiXsdt>,
    num_tables: usize,
    #[allow(dead_code)]
    root_table_addr: usize,
    rsdp_addr: usize,
}

impl<'a> AcpiParser<'a> {
    // Create a new AcpiParser, using the given PhysMemReader object.
    //
    // PhysMemReader must outlive this object. Caller retains ownership of the PhysMemReader.
    pub fn init(physmem_reader: &'a dyn PhysMemReader, rsdp_pa: usize) -> Result<Self, Status> {
        let root_tables = find_root_tables(physmem_reader, rsdp_pa)?;

        // If an XSDT table exists, try using it first.
        if root_tables.xsdt_address != 0 {
            match validate_xsdt(physmem_reader, root_tables.xsdt_address as usize) {
                Ok((xsdt, count)) => {
                    kprintln!(
                        "ACPI LITE: Found valid XSDT table at physical address 0x{:x}",
                        root_tables.xsdt_address,
                    );
                    return Ok(AcpiParser {
                        reader: physmem_reader,
                        rsdt: None,
                        xsdt: Some(xsdt),
                        num_tables: count,
                        root_table_addr: root_tables.xsdt_address as usize,
                        rsdp_addr: root_tables.rsdp_address,
                    });
                }
                Err(_) => {
                    kprintln!(
                        "ACPI LITE: Invalid XSDT table at physical address 0x{:x}",
                        root_tables.xsdt_address,
                    );
                }
            }
        }

        // Otherwise, try using the RSDT.
        if root_tables.rsdt_address != 0 {
            match validate_rsdt(physmem_reader, root_tables.rsdt_address as usize) {
                Ok((rsdt, count)) => {
                    kprintln!(
                        "ACPI LITE: Found valid RSDT table at physical address 0x{:x}",
                        root_tables.rsdt_address,
                    );
                    return Ok(AcpiParser {
                        reader: physmem_reader,
                        rsdt: Some(rsdt),
                        xsdt: None,
                        num_tables: count,
                        root_table_addr: root_tables.rsdt_address as usize,
                        rsdp_addr: root_tables.rsdp_address,
                    });
                }
                Err(_) => {
                    kprintln!(
                        "ACPI LITE: Invalid RSDT table at physical address 0x{:x}",
                        root_tables.rsdt_address,
                    );
                }
            }
        }

        Err(Status::NOT_FOUND)
    }

    pub fn rsdp_pa(&self) -> usize {
        self.rsdp_addr
    }

    // Get the physical address of the given table, or return 0 if the table does not exist.
    fn get_table_phys_addr(&self, index: usize) -> usize {
        if index >= self.num_tables {
            return 0;
        }
        if let Some(xsdt) = self.xsdt {
            // SAFETY: index is within bounds of the validated XSDT.
            unsafe { xsdt.get_entry(index) as usize }
        } else if let Some(rsdt) = self.rsdt {
            // SAFETY: index is within bounds of the validated RSDT.
            unsafe { rsdt.get_entry(index) as usize }
        } else {
            0
        }
    }

    // Print tables to debug output.
    pub fn dump_tables(&self) {
        struct StdoutWriter;

        impl core::fmt::Write for StdoutWriter {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                kprint!("{:s}", s);
                Ok(())
            }
        }
        let mut writer = StdoutWriter;
        kprintln!("root table at paddr 0x{:x}:", self.root_table_addr);
        if let Some(xsdt) = self.xsdt {
            // SAFETY: xsdt is a valid reference. xsdt.size() returns the size of the table.
            let slice = unsafe {
                core::slice::from_raw_parts(xsdt as *const AcpiXsdt as *const u8, xsdt.size())
            };
            let _ = pretty::hexdump_very_ex_rs(&mut writer, slice, xsdt as *const _ as u64);
        } else if let Some(rsdt) = self.rsdt {
            // SAFETY: rsdt is a valid reference. rsdt.size() returns the size of the table.
            let slice = unsafe {
                core::slice::from_raw_parts(rsdt as *const AcpiRsdt as *const u8, rsdt.size())
            };
            let _ = pretty::hexdump_very_ex_rs(&mut writer, slice, rsdt as *const _ as u64);
        }

        for i in 0..self.num_tables {
            if let Some(header) = self.get_table_at_index(i) {
                let mut name = [0u8; 5];
                header.sig.write_to_buffer(&mut name);
                let name_str = core::str::from_utf8(&name[..4]).unwrap_or("????");
                kprintln!(
                    "table {:x}: '{:s}' at paddr 0x{:x}, len {:x}",
                    i,
                    name_str,
                    self.get_table_phys_addr(i),
                    header.size(),
                );
                // SAFETY: header is a valid reference. header.size() returns the size of the table.
                let slice = unsafe {
                    core::slice::from_raw_parts(
                        header as *const AcpiSdtHeader as *const u8,
                        header.size(),
                    )
                };
                let _ = pretty::hexdump_very_ex_rs(&mut writer, slice, header as *const _ as u64);
            }
        }
    }
}

impl<'a> AcpiParserInterface for AcpiParser<'a> {
    fn num_tables(&self) -> usize {
        self.num_tables
    }

    fn get_table_at_index(&self, index: usize) -> Option<&AcpiSdtHeader> {
        let paddr = self.get_table_phys_addr(index);
        if paddr == 0 {
            return None;
        }
        map_structure::<AcpiSdtHeader>(self.reader, paddr).ok()
    }
}
