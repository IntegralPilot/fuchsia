// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::utils::LogError;
use crate::{address_space, mem, regs, utils};
use bitfield::bitfield;
use core::ops::Range;
use std::io::{BufRead, Cursor, Read};

pub const SHARED_REGION_START: usize = 0x04_000_000;

// Helper trait for better cursor functions.
trait CursorExt {
    fn bytes_left(&self) -> usize;
    fn new_range(&self, size: usize) -> Self;
    fn read_u8(&mut self) -> Result<u8, zx::Status>;
    fn read_u16(&mut self) -> Result<u16, zx::Status>;
    fn read_u32(&mut self) -> Result<u32, zx::Status>;
    // Read a range that is [start: u32, end: u32].
    // These are expanded to usize so it can be used more easily.
    fn read_u32_range(&mut self) -> Result<Range<usize>, zx::Status>;
}

impl CursorExt for Cursor<&[u8]> {
    fn bytes_left(&self) -> usize {
        self.get_ref().len() - self.position() as usize
    }

    fn new_range(&self, size: usize) -> Self {
        let start = self.position() as usize;
        let end = start + size;
        Cursor::new(&self.get_ref()[start..end])
    }

    fn read_u8(&mut self) -> Result<u8, zx::Status> {
        let mut bytes = [0];
        self.read_exact(&mut bytes).map_err(|_| zx::Status::INTERNAL)?;
        Ok(bytes[0])
    }

    fn read_u16(&mut self) -> Result<u16, zx::Status> {
        let mut bytes = [0; 2];
        self.read_exact(&mut bytes).map_err(|_| zx::Status::INTERNAL)?;
        Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn read_u32(&mut self) -> Result<u32, zx::Status> {
        let mut bytes = [0; 4];
        self.read_exact(&mut bytes).map_err(|_| zx::Status::INTERNAL)?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn read_u32_range(&mut self) -> Result<Range<usize>, zx::Status> {
        let start = self.read_u32()?;
        let end = self.read_u32()?;
        if start > end {
            log::error!("Read bad range: {}-{}", start, end);
            return Err(zx::Status::INTERNAL);
        }
        Ok((start as usize)..(end as usize))
    }
}

// Things in this module are read directly out of the firmware binary.
mod binary {
    use super::*;

    bitfield! {
        pub struct Flags(u32);
        impl Debug;
        pub read, set_read: 0, 0;
        pub write, set_write: 1, 1;
        pub exec, set_exec: 2, 2;
        pub cache_mode_cached, set_cache_mode_cached: 3, 3;
        pub cache_mode_coherent, set_cache_mode_coherent: 4, 4;
        pub protected, set_protected: 5, 5;
        pub shared, set_shared: 30, 30;
        pub zero, set_zero: 31, 31;
    }

    impl Flags {
        pub fn into_address_space_flags(&self) -> address_space::AccessFlags {
            let mut flags = address_space::AccessFlags(0);
            flags.set_access_flag_read(self.read() as u64);
            flags.set_access_flag_no_write(1 - self.write() as u64);
            flags.set_access_flag_no_exec(1 - self.exec() as u64);

            // TODO(https://fxbug.dev/498573259): We should be setting this based on:
            //  `cache_mode_cached`, `cached_mode_coherent` and `shared`.
            flags.set_attribute_slot(regs::MemoryAttributeSlot::NonCacheable as u64);

            flags
        }

        pub fn into_bti_options(&self) -> zx::BtiOptions {
            let mut options = zx::BtiOptions::empty();
            if self.read() == 1 {
                options |= zx::BtiOptions::PERM_READ
            }
            if self.write() == 1 {
                options |= zx::BtiOptions::PERM_WRITE
            }
            options
        }
    }

    #[derive(Debug)]
    pub struct FirmwareHeader {
        pub minor: u8,
        pub major: u8,
        pub version_hash: u32,
        pub size: u32,
    }

    impl FirmwareHeader {
        const MAGIC: u32 = 0xc3f13a6e;

        pub fn read(cursor: &mut Cursor<&[u8]>) -> Result<Self, zx::Status> {
            let magic = cursor.read_u32()?;
            if magic != Self::MAGIC {
                log::error!("Bad firmware header: bad magic {} != {}", magic, Self::MAGIC);
                return Err(zx::Status::INVALID_ARGS);
            }

            let minor = cursor.read_u8()?;
            let major = cursor.read_u8()?;
            let padding1 = cursor.read_u16()?;
            let version_hash = cursor.read_u32()?;
            let padding2 = cursor.read_u32()?;
            let size = cursor.read_u32()?;

            if padding1 != 0 || padding2 != 0 {
                log::error!("Bad firmware header: non-zero padding");
                return Err(zx::Status::INVALID_ARGS);
            }

            Ok(Self { minor, major, version_hash, size })
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub enum EntryType {
        Interface = 0,
        Config = 1,
        UnitTest = 2,
        TraceBuffer = 3,
        TimelineData = 4,
        BuildInfo = 6,
    }

    impl TryFrom<u8> for EntryType {
        type Error = u8;

        fn try_from(value: u8) -> Result<Self, Self::Error> {
            match value {
                0 => Ok(EntryType::Interface),
                1 => Ok(EntryType::Config),
                2 => Ok(EntryType::UnitTest),
                3 => Ok(EntryType::TraceBuffer),
                4 => Ok(EntryType::TimelineData),
                6 => Ok(EntryType::BuildInfo),
                _ => Err(value),
            }
        }
    }

    #[derive(Debug)]
    pub struct SectionHeader {
        pub data: Range<usize>,
        pub virtual_address: Range<usize>,
        pub flags: Flags,
    }

    impl SectionHeader {
        pub fn read(cursor: &mut Cursor<&[u8]>) -> Result<Self, zx::Status> {
            let flags = Flags(cursor.read_u32()?);
            let virtual_address = cursor.read_u32_range()?;
            let data = cursor.read_u32_range()?;
            Ok(Self { flags, virtual_address, data })
        }
    }

    bitfield! {
        pub struct EntryHeader(u32);
        impl Debug;
        pub entry, set_entry: 7, 0;
        pub size, set_size: 15, 8;
        pub optional, set_optional: 31, 31;
    }

    impl EntryHeader {
        pub fn read(cursor: &mut Cursor<&[u8]>) -> Result<Self, zx::Status> {
            Ok(Self(cursor.read_u32()?))
        }

        pub fn entry_type(&self) -> Result<EntryType, u8> {
            EntryType::try_from(self.entry() as u8)
        }
    }
}

pub struct Section {
    pub name: String,
    pub data: mem::Buffer,
    pub virtual_address: Range<usize>,
    pub flags: binary::Flags,
}

impl std::fmt::Debug for Section {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Section")
            .field("name", &self.name)
            .field("data size", &self.data.size())
            .field("virtual_address", &self.virtual_address)
            .field("flags", &self.flags)
            .finish()
    }
}

/// Parses the firmware sections from the binary.
pub fn parse_firmware(firmware: &[u8]) -> Result<Vec<Section>, zx::Status> {
    let mut cursor = Cursor::new(firmware);

    let header = binary::FirmwareHeader::read(&mut cursor).log_err("Bad header")?;
    log::info!(
        "Firmware version {}.{} hash: {:x}",
        header.major,
        header.minor,
        header.version_hash
    );

    if header.size > cursor.bytes_left() as u32 {
        log::error!("Bad header size! size: {} bytes_left: {}", header.size, cursor.bytes_left());
        return Err(zx::Status::INVALID_ARGS);
    }

    let mut sections = Vec::new();
    while (cursor.position() as u32) < header.size {
        let section = read_section(&mut cursor, firmware).log_err("Failed to read section")?;
        if let Some(section) = section {
            sections.push(section);
        }
    }

    Ok(sections)
}

fn read_section(
    cursor: &mut Cursor<&[u8]>,
    firmware: &[u8],
) -> Result<Option<Section>, zx::Status> {
    let header = binary::EntryHeader::read(cursor)?;
    let section_size = header.size() as usize - size_of::<binary::EntryHeader>();

    let mut entry_cursor = cursor.new_range(section_size);
    cursor.consume(section_size);

    let entry_type = match header.entry_type() {
        Ok(t) => t,
        Err(e) => {
            if header.optional() == 1 {
                log::info!("Unexpected optional firmware type: {}", e);
                return Ok(None);
            } else {
                log::error!("Failed to handle firmware type: {}", e);
                return Err(zx::Status::INVALID_ARGS);
            }
        }
    };

    let section = match entry_type {
        binary::EntryType::Interface => {
            read_interface(&mut entry_cursor, firmware).log_err("Failed to read section")?
        }
        binary::EntryType::BuildInfo => read_build_info(&mut entry_cursor, firmware)?,
        _ => None,
    };
    Ok(section)
}

fn read_build_info(
    cursor: &mut Cursor<&[u8]>,
    firmware: &[u8],
) -> Result<Option<Section>, zx::Status> {
    let start = cursor.read_u32()? as usize;
    let end = start + (cursor.read_u32()? as usize);
    let info = std::ffi::CStr::from_bytes_until_nul(&firmware[start..end])
        .log_err("Bad name conversion")
        .map_err(|_| zx::Status::INTERNAL)?;
    log::info!("Build info: {:?}", info);
    Ok(None)
}

fn read_interface(
    cursor: &mut Cursor<&[u8]>,
    firmware: &[u8],
) -> Result<Option<Section>, zx::Status> {
    let header = binary::SectionHeader::read(cursor)?;

    if header.data.end as usize > firmware.len() {
        log::error!(
            "Header data points past firmware! data_end:{:#x} firmware_size:{:#x}",
            header.data.end,
            firmware.len()
        );
        return Err(zx::Status::INVALID_ARGS);
    }

    if header.flags.protected() != 0 {
        log::warn!("Ignoring firmware protected mode entry");
        return Ok(None);
    }

    if header.virtual_address.start == SHARED_REGION_START && header.flags.shared() == 0 {
        log::error!("The CSF shared region is not marked shared");
        return Err(zx::Status::INVALID_ARGS);
    }
    if header.virtual_address.start != SHARED_REGION_START && header.flags.shared() == 1 {
        log::error!("Shared region not CSF");
        return Err(zx::Status::INVALID_ARGS);
    }

    let mut name = Vec::new();
    cursor.read_to_end(&mut name).map_err(|_| zx::Status::INTERNAL)?;
    let c_name = std::ffi::CStr::from_bytes_until_nul(&name)
        .log_err("Bad name conversion")
        .map_err(|_| zx::Status::INTERNAL)?;
    let name = c_name.to_string_lossy().to_string();

    let buffer_size = header.virtual_address.end - header.virtual_address.start;
    debug_assert!(
        buffer_size % (utils::PAGE_SIZE as usize) == 0,
        "Buffer size not page multiple {}",
        buffer_size
    );
    let buffer = mem::Buffer::new(
        header.virtual_address.len(),
        if header.flags.shared() == 1 {
            // TODO(https://fxbug.dev/498573259): Outside of a VM this should be WriteCombine.
            zx::CachePolicy::Cached
        } else {
            zx::CachePolicy::Cached
        },
    )
    .log_err("Failed to create buffer")?;

    let mut mapped = buffer
        .map(0, buffer.size(), zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE)
        .log_err("Failed to map buffer")?;
    mapped.write_bytes(0, &firmware[header.data]);
    mapped.flush_cache();

    Ok(Some(Section {
        flags: header.flags,
        name,
        data: buffer,
        virtual_address: header.virtual_address,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use binary::Flags;

    #[fuchsia::test]
    fn test_header() {
        let Ok(file) = std::fs::File::open("/pkg/data/firmware.bin") else {
            log::warn!("Skipping test because no firmware file");
            return;
        };
        let vmo = fdio::get_vmo_copy_from_file(&file).unwrap();
        let firmware_map = crate::mem::MappedMemory::new(
            &vmo,
            0,
            vmo.get_size().unwrap() as usize,
            zx::VmarFlags::PERM_READ,
        )
        .unwrap();
        let mut cursor = Cursor::new(firmware_map.as_u8());
        let header = binary::FirmwareHeader::read(&mut cursor).unwrap();
        assert_eq!(header.minor, 3);
        assert_eq!(header.major, 0);
        assert_eq!(header.version_hash, 0x2080000);
        assert_eq!(header.size, 0x318);
    }

    #[fuchsia::test]
    fn test_firmware() {
        let Ok(file) = std::fs::File::open("/pkg/data/firmware.bin") else {
            log::warn!("Skipping test because no firmware file");
            return;
        };
        let vmo = fdio::get_vmo_copy_from_file(&file).unwrap();
        let firmware_map = crate::mem::MappedMemory::new(
            &vmo,
            0,
            vmo.get_size().unwrap() as usize,
            zx::VmarFlags::PERM_READ,
        )
        .unwrap();
        let firmware = parse_firmware(firmware_map.as_u8()).unwrap();
        let expected_firmware = vec![
            Section {
                name: "".into(),
                data: mem::Buffer::new(0, zx::CachePolicy::Cached).unwrap(),
                virtual_address: 4194304..4259840,
                flags: Flags(9),
            },
            Section {
                name: "".into(),
                data: mem::Buffer::new(0, zx::CachePolicy::Cached).unwrap(),
                virtual_address: 4259840..4325376,
                flags: Flags(9),
            },
            Section {
                name: "".into(),
                data: mem::Buffer::new(0, zx::CachePolicy::Cached).unwrap(),
                virtual_address: 0..65536,
                flags: Flags(9),
            },
            Section {
                name: "".into(),
                data: mem::Buffer::new(0, zx::CachePolicy::Cached).unwrap(),
                virtual_address: 8388608..8519680,
                flags: Flags(13),
            },
            Section {
                name: "".into(),
                data: mem::Buffer::new(0, zx::CachePolicy::Cached).unwrap(),
                virtual_address: 33554432..33816576,
                flags: Flags(2147483659),
            },
            Section {
                name: "".into(),
                data: mem::Buffer::new(0, zx::CachePolicy::Cached).unwrap(),
                virtual_address: 16777216..17039360,
                flags: Flags(2147483657),
            },
            Section {
                name: "".into(),
                data: mem::Buffer::new(0, zx::CachePolicy::Cached).unwrap(),
                virtual_address: 67108864..67174400,
                flags: Flags(3221225499),
            },
        ];

        assert_eq!(expected_firmware.len(), firmware.len());
        for i in 0..firmware.len() {
            assert_eq!(expected_firmware[i].name, firmware[i].name);
            assert_eq!(expected_firmware[i].virtual_address, firmware[i].virtual_address);
            assert_eq!(expected_firmware[i].flags.0, firmware[i].flags.0);
            assert_eq!(firmware[i].virtual_address.len() % 0x1000, 0);
        }
    }
}
