// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::string_table::StringTable;
use crate::structures::{Header, StructType};
use zerocopy::byteorder::little_endian::{U16, U32, U64};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};
use zx_status::Status;

pub const SMBIOS2_ANCHOR: &[u8; 4] = b"_SM_";
pub const SMBIOS2_INTERMEDIATE_ANCHOR: &[u8; 5] = b"_DMI_";
pub const SMBIOS3_ANCHOR: &[u8; 5] = b"_SM3_";

/// Computes the 8-bit checksum of a byte slice.
pub fn compute_checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |sum, &b| sum.wrapping_add(b))
}

/// Utility for comparing SMBIOS specification versions.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SpecVersion {
    pub major_ver: u8,
    pub minor_ver: u8,
    pub docrev_ver: u8,
}

impl SpecVersion {
    /// Creates a new `SpecVersion`.
    pub const fn new(major: u8, minor: u8, docrev: u8) -> Self {
        Self { major_ver: major, minor_ver: minor, docrev_ver: docrev }
    }

    /// Creates a new `SpecVersion` for SMBIOS 2.x (docrev defaults to 0).
    pub const fn new_v2(major: u8, minor: u8) -> Self {
        Self { major_ver: major, minor_ver: minor, docrev_ver: 0 }
    }

    /// Returns true if this version is at least the queried version.
    pub const fn includes_version(
        &self,
        spec_major_ver: u8,
        spec_minor_ver: u8,
        spec_docrev_ver: u8,
    ) -> bool {
        if self.major_ver > spec_major_ver {
            return true;
        }
        if self.major_ver < spec_major_ver {
            return false;
        }
        if self.minor_ver > spec_minor_ver {
            return true;
        }
        if self.minor_ver < spec_minor_ver {
            return false;
        }
        self.docrev_ver >= spec_docrev_ver
    }
}

/// SMBIOS EntryPoint version.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum EntryPointVersion {
    Unknown,
    V2_1,
    V3_0,
}

/// System structure identifying where SMBIOS v2.1 structs are in memory.
#[repr(C, packed)]
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Eq,
    PartialEq,
    FromBytes,
    IntoBytes,
    Immutable,
    KnownLayout,
    Unaligned,
)]
pub struct EntryPoint2_1 {
    pub anchor_string: [u8; 4], // _SM_
    pub checksum: u8,
    pub length: u8,

    // SMBIOS specification revision
    pub major_ver: u8,
    pub minor_ver: u8,

    pub max_struct_size: U16,

    pub ep_rev: u8,              // Should be 0x00 for version SMBIOS 2.1 entry point
    pub formatted_area: [u8; 5], // Should be all 0x00 for ver 2.1

    pub intermediate_anchor_string: [u8; 5], // _DMI_
    pub intermediate_checksum: u8,

    pub struct_table_length: U16,
    pub struct_table_phys: U32,
    pub struct_count: U16,

    pub bcd_rev: u8, // Should be 0x21
}

zr::static_assert!(core::mem::size_of::<EntryPoint2_1>() == 0x1f);
zr::static_assert!(core::mem::align_of::<EntryPoint2_1>() == 1);

impl EntryPoint2_1 {
    /// Validates the entry point structure checksums and magic strings.
    pub fn is_valid(&self) -> bool {
        if &self.anchor_string != SMBIOS2_ANCHOR {
            return false;
        }

        let real_length = if self.length == 0x1f {
            0x1f
        } else if self.length == 0x1e {
            // 0x1e is allowed due to errata in the SMBIOS 2.1 spec. It really means 0x1f.
            0x1f
        } else {
            return false;
        };

        let bytes = self.as_bytes();
        if compute_checksum(&bytes[..real_length as usize]) != 0 {
            return false;
        }

        if self.ep_rev != 0 {
            return false;
        }

        if &self.intermediate_anchor_string != SMBIOS2_INTERMEDIATE_ANCHOR {
            return false;
        }

        const INTERMEDIATE_OFFSET: usize = 0x10;
        if compute_checksum(&bytes[INTERMEDIATE_OFFSET..real_length as usize]) != 0 {
            return false;
        }

        let phys = self.struct_table_phys.get();
        let len = self.struct_table_length.get() as u32;
        !phys.checked_add(len).is_none()
    }

    /// Returns the specification version supported by this entry point.
    pub fn version(&self) -> SpecVersion {
        SpecVersion::new(self.major_ver, self.minor_ver, 0)
    }

    /// Formats a dump of the entry point to the provided writer.
    pub fn dump<W: core::fmt::Write>(&self, writer: &mut W) -> core::fmt::Result {
        writeln!(writer, "SMBIOS EntryPoint v2.1:")?;
        writeln!(writer, "  specification version: {}.{}", self.major_ver, self.minor_ver)?;
        writeln!(writer, "  max struct size: {}", self.max_struct_size.get())?;
        writeln!(
            writer,
            "  struct table: {} bytes @0x{:08x}, {} entries",
            self.struct_table_length.get(),
            self.struct_table_phys.get(),
            self.struct_count.get()
        )?;
        Ok(())
    }
}

/// System structure identifying where SMBIOS v3.0 structs are in memory.
#[repr(C, packed)]
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Eq,
    PartialEq,
    FromBytes,
    IntoBytes,
    Immutable,
    KnownLayout,
    Unaligned,
)]
pub struct EntryPoint3_0 {
    pub anchor_string: [u8; 5], // _SM3_
    pub checksum: u8,
    pub length: u8,

    // SMBIOS specification revision
    pub major_ver: u8,
    pub minor_ver: u8,
    pub docrev_ver: u8,

    pub ep_rev: u8, // Should be 0x01 for SMBIOS 3.0 entry point.
    pub reserved: u8,

    pub max_struct_size: U32,
    pub struct_table_phys: U64,
}

zr::static_assert!(core::mem::size_of::<EntryPoint3_0>() == 0x18);
zr::static_assert!(core::mem::align_of::<EntryPoint3_0>() == 1);

impl EntryPoint3_0 {
    /// Validates the entry point structure checksum and magic string.
    pub fn is_valid(&self) -> bool {
        if &self.anchor_string != SMBIOS3_ANCHOR {
            return false;
        }

        if self.length as usize != core::mem::size_of::<Self>() {
            return false;
        }

        if compute_checksum(self.as_bytes()) != 0 {
            return false;
        }

        true
    }

    /// Returns the specification version supported by this entry point.
    pub fn version(&self) -> SpecVersion {
        SpecVersion::new(self.major_ver, self.minor_ver, self.docrev_ver)
    }
}

/// Type representing the underlying entry point version reference.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum EntryPointType<'a> {
    V2_1(&'a EntryPoint2_1),
    V3_0(&'a EntryPoint3_0),
}

/// Unified abstraction over SMBIOS 2.1 and 3.0 entry points.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EntryPoint<'a> {
    inner: EntryPointType<'a>,
}

impl<'a> From<&'a EntryPoint2_1> for EntryPoint<'a> {
    fn from(ep: &'a EntryPoint2_1) -> Self {
        Self { inner: EntryPointType::V2_1(ep) }
    }
}

impl<'a> From<&'a EntryPoint3_0> for EntryPoint<'a> {
    fn from(ep: &'a EntryPoint3_0) -> Self {
        Self { inner: EntryPointType::V3_0(ep) }
    }
}

impl<'a> EntryPoint<'a> {
    /// Creates an `EntryPoint` wrapper around an already-validated entry point type.
    pub fn new(inner: EntryPointType<'a>) -> Self {
        Self { inner }
    }

    /// Attempts to parse and validate an `EntryPoint` from a byte slice.
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self, Status> {
        if let Ok((v2, _)) = EntryPoint2_1::ref_from_prefix(bytes) {
            if v2.is_valid() {
                return Ok(Self { inner: EntryPointType::V2_1(v2) });
            }
        }
        if let Ok((v3, _)) = EntryPoint3_0::ref_from_prefix(bytes) {
            if v3.is_valid() {
                return Ok(Self { inner: EntryPointType::V3_0(v3) });
            }
        }
        Err(Status::IO_DATA_INTEGRITY)
    }

    /// Creates an `EntryPoint` from a raw memory address.
    ///
    /// # Safety
    ///
    /// `ep_start` must point to valid mapped memory containing at least `size_of::<EntryPoint2_1>()`
    /// (0x1f) bytes or `size_of::<EntryPoint3_0>()` (0x18) bytes.
    pub unsafe fn create(ep_start: usize) -> Result<EntryPoint<'static>, Status> {
        if ep_start == 0 {
            return Err(Status::INVALID_ARGS);
        }

        // SAFETY: Caller guarantees ep_start points to valid memory.
        let v2_ptr = core::ptr::with_exposed_provenance::<EntryPoint2_1>(ep_start);
        let v2_ref = unsafe { &*v2_ptr };
        if v2_ref.is_valid() {
            return Ok(EntryPoint { inner: EntryPointType::V2_1(v2_ref) });
        }

        // SAFETY: Caller guarantees ep_start points to valid memory.
        let v3_ptr = core::ptr::with_exposed_provenance::<EntryPoint3_0>(ep_start);
        let v3_ref = unsafe { &*v3_ptr };
        if v3_ref.is_valid() {
            return Ok(EntryPoint { inner: EntryPointType::V3_0(v3_ref) });
        }

        Err(Status::IO_DATA_INTEGRITY)
    }

    /// Returns the physical address of the structure table.
    pub fn struct_table_phys(&self) -> u64 {
        match self.inner {
            EntryPointType::V2_1(ep) => ep.struct_table_phys.get() as u64,
            EntryPointType::V3_0(ep) => ep.struct_table_phys.get(),
        }
    }

    /// Returns the length of the structure table in bytes.
    ///
    /// Note that for SMBIOS 3.0, this returns `max_struct_size`, and the structures should be
    /// checked for the End-of-Table type (`StructType::END_OF_TABLE`).
    pub fn struct_table_length(&self) -> u32 {
        match self.inner {
            EntryPointType::V2_1(ep) => ep.struct_table_length.get() as u32,
            EntryPointType::V3_0(ep) => ep.max_struct_size.get(),
        }
    }

    /// Returns the maximum structure size in bytes.
    pub fn max_struct_size(&self) -> u32 {
        match self.inner {
            EntryPointType::V2_1(ep) => ep.max_struct_size.get() as u32,
            EntryPointType::V3_0(ep) => ep.max_struct_size.get(),
        }
    }

    /// Returns the specification version supported by this entry point.
    pub fn version(&self) -> SpecVersion {
        match self.inner {
            EntryPointType::V2_1(ep) => ep.version(),
            EntryPointType::V3_0(ep) => ep.version(),
        }
    }

    /// Returns true if the entry point specifies a structure count (SMBIOS 2.1).
    pub fn has_struct_count(&self) -> bool {
        matches!(self.inner, EntryPointType::V2_1(_))
    }

    /// Returns the number of structures (SMBIOS 2.1 only).
    ///
    /// # Panics
    ///
    /// Panics if called on an SMBIOS 3.0 entry point where structure count is not present.
    pub fn struct_count(&self) -> u16 {
        match self.inner {
            EntryPointType::V2_1(ep) => ep.struct_count.get(),
            EntryPointType::V3_0(_) => panic!("struct_count() called on SMBIOS 3.0 entry point"),
        }
    }

    /// Walks the known SMBIOS structures within the provided `struct_table` slice.
    /// The callback will be called once for each structure found.
    ///
    /// Return values from the callback:
    /// - `Err(Status::STOP)`: aborted and returns `Ok(())`
    /// - `Ok(())` or `Err(Status::NEXT)`: walk continues to the next structure
    /// - Any other error: walk is aborted and returns the error
    pub fn walk_structs<F>(&self, struct_table: &[u8], mut cb: F) -> Result<(), Status>
    where
        F: FnMut(SpecVersion, &Header, &StringTable<'_>) -> Result<(), Status>,
    {
        let mut idx = 0usize;
        let mut curr = 0usize;
        let table_len = core::cmp::min(self.struct_table_length() as usize, struct_table.len());

        while curr + core::mem::size_of::<Header>() < table_len {
            let (hdr, _) = Header::ref_from_prefix(&struct_table[curr..])
                .map_err(|_| Status::IO_DATA_INTEGRITY)?;

            let hdr_len = hdr.length as usize;
            if hdr_len < core::mem::size_of::<Header>() || curr + hdr_len > table_len {
                return Err(Status::IO_DATA_INTEGRITY);
            }

            if hdr.r#type == StructType::END_OF_TABLE {
                return Ok(());
            }

            let max_struct_len = core::cmp::max(table_len - curr, self.max_struct_size() as usize);

            let st = StringTable::init(hdr, max_struct_len, &struct_table[curr..])?;

            let status = cb(self.version(), hdr, &st);
            match status {
                Ok(()) => {}
                Err(Status::STOP) => return Ok(()),
                Err(Status::NEXT) => {}
                Err(err) => return Err(err),
            }

            idx += 1;
            if self.has_struct_count() && self.struct_count() as usize == idx {
                return Ok(());
            }

            // Skip over the formatted portion and embedded strings
            curr += hdr_len + st.length();
        }

        Err(Status::IO_DATA_INTEGRITY)
    }

    /// Walks the known SMBIOS structures mapped at virtual address `struct_table_virt`.
    ///
    /// # Safety
    ///
    /// `struct_table_virt` must point to valid mapped memory containing at least
    /// `struct_table_length()` bytes.
    pub unsafe fn walk_structs_raw<F>(&self, struct_table_virt: usize, cb: F) -> Result<(), Status>
    where
        F: FnMut(SpecVersion, &Header, &StringTable<'_>) -> Result<(), Status>,
    {
        let len = self.struct_table_length() as usize;
        if struct_table_virt == 0 || len == 0 {
            return Err(Status::INVALID_ARGS);
        }

        // SAFETY: Caller guarantees struct_table_virt is mapped with at least len bytes.
        let slice = unsafe {
            let ptr = core::ptr::with_exposed_provenance::<u8>(struct_table_virt);
            core::slice::from_raw_parts(ptr, len)
        };
        self.walk_structs(slice, cb)
    }
}
