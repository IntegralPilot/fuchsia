// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::string_table::StringTable;
use zerocopy::byteorder::little_endian::{U16, U64};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// SMBIOS Structure Type identifier.
#[repr(transparent)]
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    FromBytes,
    IntoBytes,
    Immutable,
    KnownLayout,
    Unaligned,
)]
pub struct StructType(pub u8);

impl StructType {
    pub const BIOS_INFO: Self = Self(0);
    pub const SYSTEM_INFO: Self = Self(1);
    pub const BASEBOARD: Self = Self(2);
    pub const SYSTEM_ENCLOSURE: Self = Self(3);
    pub const PROCESSOR: Self = Self(4);
    pub const MEMORY_CONTROLLER: Self = Self(5);
    pub const MEMORY_MODULE: Self = Self(6);
    pub const CACHE: Self = Self(7);
    pub const PORT_CONNECTOR: Self = Self(8);
    pub const SYSTEM_SLOTS: Self = Self(9);
    pub const ON_BOARD_DEVICES: Self = Self(10);
    pub const OEM_STRINGS: Self = Self(11);
    pub const SYSTEM_CONFIG_OPTIONS: Self = Self(12);
    pub const BIOS_LANGUAGE: Self = Self(13);

    pub const END_OF_TABLE: Self = Self(127);

    // PascalCase aliases matching C++ enum names.
    #[allow(non_upper_case_globals)]
    pub const BiosInfo: Self = Self::BIOS_INFO;
    #[allow(non_upper_case_globals)]
    pub const SystemInfo: Self = Self::SYSTEM_INFO;
    #[allow(non_upper_case_globals)]
    pub const Baseboard: Self = Self::BASEBOARD;
    #[allow(non_upper_case_globals)]
    pub const SystemEnclosure: Self = Self::SYSTEM_ENCLOSURE;
    #[allow(non_upper_case_globals)]
    pub const Processor: Self = Self::PROCESSOR;
    #[allow(non_upper_case_globals)]
    pub const MemoryController: Self = Self::MEMORY_CONTROLLER;
    #[allow(non_upper_case_globals)]
    pub const MemoryModule: Self = Self::MEMORY_MODULE;
    #[allow(non_upper_case_globals)]
    pub const Cache: Self = Self::CACHE;
    #[allow(non_upper_case_globals)]
    pub const PortConnector: Self = Self::PORT_CONNECTOR;
    #[allow(non_upper_case_globals)]
    pub const SystemSlots: Self = Self::SYSTEM_SLOTS;
    #[allow(non_upper_case_globals)]
    pub const OnBoardDevices: Self = Self::ON_BOARD_DEVICES;
    #[allow(non_upper_case_globals)]
    pub const OemStrings: Self = Self::OEM_STRINGS;
    #[allow(non_upper_case_globals)]
    pub const SystemConfigOptions: Self = Self::SYSTEM_CONFIG_OPTIONS;
    #[allow(non_upper_case_globals)]
    pub const BiosLanguage: Self = Self::BIOS_LANGUAGE;
    #[allow(non_upper_case_globals)]
    pub const EndOfTable: Self = Self::END_OF_TABLE;
}

/// SMBIOS common struct header.
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
pub struct Header {
    pub r#type: StructType,
    pub length: u8,
    pub handle: U16,
}

zr::static_assert!(core::mem::size_of::<Header>() == 4);
zr::static_assert!(core::mem::align_of::<Header>() == 1);

/// SMBIOS BIOS Information Struct v2.0.
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
pub struct BiosInformationStruct2_0 {
    pub hdr: Header,

    pub vendor_str_idx: u8,
    pub bios_version_str_idx: u8,
    pub bios_starting_address_segment: U16,
    pub bios_release_date_str_idx: u8,
    pub bios_rom_size: u8,
    pub bios_characteristics: U64,
}

zr::static_assert!(core::mem::size_of::<BiosInformationStruct2_0>() == 0x12);
zr::static_assert!(core::mem::align_of::<BiosInformationStruct2_0>() == 1);

impl BiosInformationStruct2_0 {
    /// Returns extended BIOS characteristics bytes if present in the formatted portion.
    pub fn bios_characteristics_ext<'a>(&self, raw_struct_bytes: &'a [u8]) -> &'a [u8] {
        let len = self.hdr.length as usize;
        let base_size = core::mem::size_of::<Self>();
        if len > base_size && raw_struct_bytes.len() >= len {
            &raw_struct_bytes[base_size..len]
        } else {
            &[]
        }
    }

    /// Formats a dump of the BIOS Information Struct v2.0 to the provided writer.
    pub fn dump<W: core::fmt::Write>(
        &self,
        writer: &mut W,
        st: &StringTable<'_>,
        raw_struct_bytes: &[u8],
    ) -> core::fmt::Result {
        writeln!(writer, "SMBIOS BIOS Information Struct v2.0:")?;
        writeln!(writer, "  vendor: {}", st.get_string_or_default(self.vendor_str_idx as usize))?;
        writeln!(
            writer,
            "  BIOS version: {}",
            st.get_string_or_default(self.bios_version_str_idx as usize)
        )?;
        writeln!(
            writer,
            "  BIOS starting address segment: 0x{:04x}",
            self.bios_starting_address_segment.get()
        )?;
        writeln!(
            writer,
            "  BIOS release date: {}",
            st.get_string_or_default(self.bios_release_date_str_idx as usize)
        )?;
        writeln!(writer, "  BIOS ROM size: 0x{:02x}", self.bios_rom_size)?;
        writeln!(writer, "  BIOS characteristics: 0x{:016x}", self.bios_characteristics.get())?;
        let ext = self.bios_characteristics_ext(raw_struct_bytes);
        for &byte in ext {
            writeln!(writer, "  BIOS characteristics extended: 0x{:02x}", byte)?;
        }
        Ok(())
    }
}

/// SMBIOS BIOS Information Struct v2.4.
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
pub struct BiosInformationStruct2_4 {
    pub hdr: Header,

    pub vendor_str_idx: u8,
    pub bios_version_str_idx: u8,
    pub bios_starting_address_segment: U16,
    pub bios_release_date_str_idx: u8,
    pub bios_rom_size: u8,
    pub bios_characteristics: U64,
    pub bios_characteristics_ext: U16,

    pub bios_major_release: u8,
    pub bios_minor_release: u8,
    pub ec_major_release: u8,
    pub ec_minor_release: u8,
}

zr::static_assert!(core::mem::size_of::<BiosInformationStruct2_4>() == 0x18);
zr::static_assert!(core::mem::align_of::<BiosInformationStruct2_4>() == 1);

impl BiosInformationStruct2_4 {
    /// Formats a dump of the BIOS Information Struct v2.4 to the provided writer.
    pub fn dump<W: core::fmt::Write>(
        &self,
        writer: &mut W,
        st: &StringTable<'_>,
    ) -> core::fmt::Result {
        writeln!(writer, "SMBIOS BIOS Information Struct v2.4:")?;
        writeln!(writer, "  vendor: {}", st.get_string_or_default(self.vendor_str_idx as usize))?;
        writeln!(
            writer,
            "  BIOS version: {}",
            st.get_string_or_default(self.bios_version_str_idx as usize)
        )?;
        writeln!(
            writer,
            "  BIOS starting address segment: 0x{:04x}",
            self.bios_starting_address_segment.get()
        )?;
        writeln!(
            writer,
            "  BIOS release date: {}",
            st.get_string_or_default(self.bios_release_date_str_idx as usize)
        )?;
        writeln!(writer, "  BIOS ROM size: 0x{:02x}", self.bios_rom_size)?;
        writeln!(writer, "  BIOS characteristics: 0x{:016x}", self.bios_characteristics.get())?;
        writeln!(
            writer,
            "  BIOS characteristics extended: 0x{:04x}",
            self.bios_characteristics_ext.get()
        )?;
        writeln!(
            writer,
            "  BIOS version number: {}.{}",
            self.bios_major_release, self.bios_minor_release
        )?;
        writeln!(
            writer,
            "  EC version number: {}.{}",
            self.ec_major_release, self.ec_minor_release
        )?;
        let base_size = core::mem::size_of::<Self>();
        if self.hdr.length as usize > base_size {
            writeln!(
                writer,
                "  {} bytes of unknown trailing contents",
                self.hdr.length as usize - base_size
            )?;
        }
        Ok(())
    }
}

/// SMBIOS System Information Struct v2.0.
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
pub struct SystemInformationStruct2_0 {
    pub hdr: Header,

    pub manufacturer_str_idx: u8,
    pub product_name_str_idx: u8,
    pub version_str_idx: u8,
    pub serial_number_str_idx: u8,
}

zr::static_assert!(core::mem::size_of::<SystemInformationStruct2_0>() == 0x8);
zr::static_assert!(core::mem::align_of::<SystemInformationStruct2_0>() == 1);

impl SystemInformationStruct2_0 {
    /// Formats a dump of the System Information Struct v2.0 to the provided writer.
    pub fn dump<W: core::fmt::Write>(
        &self,
        writer: &mut W,
        st: &StringTable<'_>,
    ) -> core::fmt::Result {
        writeln!(writer, "SMBIOS System Information Struct v2.0:")?;
        writeln!(
            writer,
            "  manufacturer: {}",
            st.get_string_or_default(self.manufacturer_str_idx as usize)
        )?;
        writeln!(
            writer,
            "  product: {}",
            st.get_string_or_default(self.product_name_str_idx as usize)
        )?;
        writeln!(writer, "  version: {}", st.get_string_or_default(self.version_str_idx as usize))?;
        let base_size = core::mem::size_of::<Self>();
        if self.hdr.length as usize > base_size {
            writeln!(
                writer,
                "  {} bytes of unknown trailing contents",
                self.hdr.length as usize - base_size
            )?;
        }
        Ok(())
    }
}

/// SMBIOS System Information Struct v2.1.
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
pub struct SystemInformationStruct2_1 {
    pub hdr: Header,

    pub manufacturer_str_idx: u8,
    pub product_name_str_idx: u8,
    pub version_str_idx: u8,
    pub serial_number_str_idx: u8,

    pub uuid: [u8; 16],
    pub wakeup_type: u8,
}

zr::static_assert!(core::mem::size_of::<SystemInformationStruct2_1>() == 0x19);
zr::static_assert!(core::mem::align_of::<SystemInformationStruct2_1>() == 1);

impl SystemInformationStruct2_1 {
    /// Formats a dump of the System Information Struct v2.1 to the provided writer.
    pub fn dump<W: core::fmt::Write>(
        &self,
        writer: &mut W,
        st: &StringTable<'_>,
    ) -> core::fmt::Result {
        writeln!(writer, "SMBIOS System Information Struct v2.1:")?;
        writeln!(
            writer,
            "  manufacturer: {}",
            st.get_string_or_default(self.manufacturer_str_idx as usize)
        )?;
        writeln!(
            writer,
            "  product: {}",
            st.get_string_or_default(self.product_name_str_idx as usize)
        )?;
        writeln!(writer, "  version: {}", st.get_string_or_default(self.version_str_idx as usize))?;
        writeln!(writer, "  wakeup_type: 0x{:x}", self.wakeup_type)?;
        let base_size = core::mem::size_of::<Self>();
        if self.hdr.length as usize > base_size {
            writeln!(
                writer,
                "  {} bytes of unknown trailing contents",
                self.hdr.length as usize - base_size
            )?;
        }
        Ok(())
    }
}

/// SMBIOS System Information Struct v2.4.
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
pub struct SystemInformationStruct2_4 {
    pub hdr: Header,

    pub manufacturer_str_idx: u8,
    pub product_name_str_idx: u8,
    pub version_str_idx: u8,
    pub serial_number_str_idx: u8,

    pub uuid: [u8; 16],
    pub wakeup_type: u8,

    pub sku_number_str_idx: u8,
    pub family_str_idx: u8,
}

zr::static_assert!(core::mem::size_of::<SystemInformationStruct2_4>() == 0x1b);
zr::static_assert!(core::mem::align_of::<SystemInformationStruct2_4>() == 1);

impl SystemInformationStruct2_4 {
    /// Formats a dump of the System Information Struct v2.4 to the provided writer.
    pub fn dump<W: core::fmt::Write>(
        &self,
        writer: &mut W,
        st: &StringTable<'_>,
    ) -> core::fmt::Result {
        writeln!(writer, "SMBIOS System Information Struct v2.4:")?;
        writeln!(
            writer,
            "  manufacturer: {}",
            st.get_string_or_default(self.manufacturer_str_idx as usize)
        )?;
        writeln!(
            writer,
            "  product: {}",
            st.get_string_or_default(self.product_name_str_idx as usize)
        )?;
        writeln!(writer, "  version: {}", st.get_string_or_default(self.version_str_idx as usize))?;
        writeln!(writer, "  wakeup_type: 0x{:x}", self.wakeup_type)?;
        writeln!(writer, "  SKU: {}", st.get_string_or_default(self.sku_number_str_idx as usize))?;
        writeln!(writer, "  family: {}", st.get_string_or_default(self.family_str_idx as usize))?;
        let base_size = core::mem::size_of::<Self>();
        if self.hdr.length as usize > base_size {
            writeln!(
                writer,
                "  {} bytes of unknown trailing contents",
                self.hdr.length as usize - base_size
            )?;
        }
        Ok(())
    }
}

/// SMBIOS Baseboard Information Struct.
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
pub struct BaseboardInformationStruct {
    pub hdr: Header,
    pub manufacturer_str_idx: u8,
    pub product_name_str_idx: u8,
    pub version_str_idx: u8,
    pub serial_number_str_idx: u8,

    pub unsafe_asset_tag_str_idx: u8,
    pub unsafe_feature_flags: u8,
    pub unsafe_location_in_chassis_str_idx: u8,
    pub unsafe_chassis_handle: U16,

    pub unsafe_board_type: u8,
    pub unsafe_contained_object_handles_count: u8,
}

zr::static_assert!(core::mem::size_of::<BaseboardInformationStruct>() == 0xf);
zr::static_assert!(core::mem::align_of::<BaseboardInformationStruct>() == 1);

impl BaseboardInformationStruct {
    /// Returns the asset tag string index if present within the struct length.
    pub fn asset_tag_str_idx(&self) -> Option<u8> {
        if self.hdr.length > 8 { Some(self.unsafe_asset_tag_str_idx) } else { None }
    }

    /// Returns the feature flags if present within the struct length.
    pub fn feature_flags(&self) -> Option<u8> {
        if self.hdr.length > 9 { Some(self.unsafe_feature_flags) } else { None }
    }

    /// Returns the location in chassis string index if present within the struct length.
    pub fn location_in_chassis_str_idx(&self) -> Option<u8> {
        if self.hdr.length > 10 { Some(self.unsafe_location_in_chassis_str_idx) } else { None }
    }

    /// Returns the chassis handle if present within the struct length.
    pub fn chassis_handle(&self) -> Option<u16> {
        if self.hdr.length >= 13 { Some(self.unsafe_chassis_handle.get()) } else { None }
    }

    /// Returns the board type if present within the struct length.
    pub fn board_type(&self) -> Option<u8> {
        if self.hdr.length > 13 { Some(self.unsafe_board_type) } else { None }
    }

    /// Returns the contained object handles count if present within the struct length.
    pub fn contained_object_handles_count(&self) -> Option<u8> {
        if self.hdr.length > 14 { Some(self.unsafe_contained_object_handles_count) } else { None }
    }

    /// Returns the slice of contained object handles if present and valid within the struct length.
    pub fn contained_object_handles<'a>(&self, raw_struct_bytes: &'a [u8]) -> Option<&'a [U16]> {
        let count = self.contained_object_handles_count()? as usize;
        let start_offset = core::mem::size_of::<Self>();
        let end_offset = start_offset + count * 2;
        if (self.hdr.length as usize) < end_offset || raw_struct_bytes.len() < end_offset {
            return None;
        }
        let handles_bytes = &raw_struct_bytes[start_offset..end_offset];
        <[U16]>::ref_from_bytes(handles_bytes).ok()
    }

    /// Formats a dump of the Baseboard Information Struct to the provided writer.
    pub fn dump<W: core::fmt::Write>(
        &self,
        writer: &mut W,
        st: &StringTable<'_>,
    ) -> core::fmt::Result {
        writeln!(writer, "SMBIOS Baseboard Information Struct:")?;
        writeln!(
            writer,
            "  manufacturer: {}",
            st.get_string_or_default(self.manufacturer_str_idx as usize)
        )?;
        writeln!(
            writer,
            "  product: {}",
            st.get_string_or_default(self.product_name_str_idx as usize)
        )?;
        writeln!(writer, "  version: {}", st.get_string_or_default(self.version_str_idx as usize))?;
        let ff_present = if self.feature_flags().is_some() { "" } else { "not " };
        writeln!(
            writer,
            "  feature flags ({}present): 0x{:02x}",
            ff_present,
            self.feature_flags().unwrap_or(0)
        )?;
        writeln!(
            writer,
            "  location: {}",
            st.get_string_or_default(self.location_in_chassis_str_idx().unwrap_or(0) as usize)
        )?;
        let bt_present = if self.board_type().is_some() { "" } else { "not " };
        writeln!(
            writer,
            "  board_type ({}present): 0x{:02x}",
            bt_present,
            self.board_type().unwrap_or(0)
        )?;
        Ok(())
    }
}
