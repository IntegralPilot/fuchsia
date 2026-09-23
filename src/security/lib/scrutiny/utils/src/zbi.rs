// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::zstd;
use anyhow::{Error, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use log::info;
use std::io::{Cursor, Read, Seek, SeekFrom};
use thiserror::Error;

/// ZBI header size in bytes.
const ZBI_HEADER_SIZE: u64 = size_of::<zbi::Header>() as u64;

/// Set in the flags if the section is compressed. We assume all compression
/// is ZSTD. If this flag is set ZbiHeader.extra will be the uncompressed
/// size of the image.
const ZBI_FLAGS_STORAGE_COMPRESSED: zbi::Flags = zbi::Flags::from_bits_retain(1);

/// Magic number for the vboot structure which can encase a ZBI.
const VBOOT_MAGIC: u64 = 0x534f454d4f524843;

/// Magic number for the Android Boot Image which can encase a ZBI.
const ANDROID_BOOT_IMAGE_MAGIC: u64 = 0x2144494f52444e41;

/// ZbiSection holder that contains the type and an uncompressed buffer
/// containing the data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZbiSection {
    pub section_type: zbi::Type,
    pub buffer: Vec<u8>,
}

fn read_header(cursor: &mut Cursor<Vec<u8>>) -> Result<zbi::Header> {
    let mut buf = [0u8; ZBI_HEADER_SIZE as usize];
    cursor.read_exact(&mut buf)?;
    Ok(unsafe { std::mem::transmute(buf) })
}

fn header_bytes(ty: zbi::Type, length: u32, extra: u32, flags: zbi::Flags) -> Vec<u8> {
    let header = zbi::Header {
        r#type: ty,
        length,
        extra,
        flags,
        reserved0: 0,
        reserved1: 0,
        magic: zbi::ITEM_MAGIC,
        crc32: 0,
    };
    let bytes: [u8; 32] = unsafe { std::mem::transmute(header) };
    bytes.to_vec()
}

#[derive(Error, Debug)]
pub enum ZbiError {
    #[error("Invalid ZBI container header type: {0:?}")]
    InvalidContainerHeader(zbi::Type),
    #[error("ZBI container header magic value {0} doesn't match expected value")]
    InvalidContainerMagic(u32),
    #[error("ZBI item header magic value doesn't match expected value")]
    InvalidItemMagic,
}

/// Responsible for extracting the zbi from the package and reading the zbi
/// data from it.
pub struct ZbiReader {
    cursor: Cursor<Vec<u8>>,
}

impl ZbiReader {
    pub fn new(zbi_buffer: Vec<u8>) -> Self {
        Self { cursor: Cursor::new(zbi_buffer) }
    }

    pub fn parse(&mut self) -> Result<Vec<ZbiSection>> {
        // The ZBI can be wrapped inside a vboot or android boot image file.
        // If this is the case, before parsing the ZBI, seek out the partition
        // holding the ZBI inside it.
        let magic = self.cursor.read_u64::<LittleEndian>()?;
        self.cursor.set_position(0);
        if magic == VBOOT_MAGIC || magic == ANDROID_BOOT_IMAGE_MAGIC {
            ZbiSeeker::seek_to_partition(&mut self.cursor)?;
        }

        // Parse the header and validate it is a ZBI.
        let container_header = read_header(&mut self.cursor)?;
        if container_header.r#type != zbi::Type::Container {
            return Err(Error::new(ZbiError::InvalidContainerHeader(container_header.r#type)));
        }
        if container_header.magic != zbi::ITEM_MAGIC {
            return Err(Error::new(ZbiError::InvalidContainerMagic(container_header.magic)));
        }
        let container_end = self.cursor.position() + (container_header.length as u64);

        let mut zbi_sections = vec![];

        if container_end == self.cursor.position() {
            return Ok(zbi_sections);
        }

        // Iterate until we cannot parse section headers anymore or reach
        // the end.
        while let Ok(section_header) = read_header(&mut self.cursor) {
            if section_header.magic != zbi::ITEM_MAGIC {
                return Err(Error::new(ZbiError::InvalidItemMagic {}));
            }

            let section_type = section_header.r#type;
            let data_len = usize::try_from(section_header.length)?;
            let mut section_data = vec![0; data_len];
            self.cursor.read_exact(&mut section_data)?;

            // Decompress the block.
            if section_header.flags.contains(ZBI_FLAGS_STORAGE_COMPRESSED) {
                let decompressed_data = zstd::decompress(&section_data, section_header.extra)?;
                zbi_sections.push(ZbiSection { section_type, buffer: decompressed_data });
            } else {
                zbi_sections.push(ZbiSection { section_type, buffer: section_data });
            }

            // All items are 8 byte aligned, skip if the end of the block isn't.
            let position: u64 = self.cursor.position();
            if position % 8 != 0 {
                let padding: u64 = 8 - (position % 8);
                self.cursor.seek(SeekFrom::Current(padding.try_into().unwrap()))?;
            }

            // Exit if we have arrived at the end of the container length.
            if self.cursor.position() >= container_end {
                break;
            }
        }
        Ok(zbi_sections)
    }
}

struct ZbiSeeker {}

impl ZbiSeeker {
    /// Seeks from the start of an unknown image scanning for the ZBI header that
    /// is wrapped within it. This doesn't attempt to understand the underlying
    /// format it just attempts to find the inner ZBI partition.
    pub fn seek_to_partition(cursor: &mut Cursor<Vec<u8>>) -> Result<()> {
        const SEEK_ALIGNMENT: u64 = 4;
        let mut header = read_header(cursor)?;
        let mut cur_pos = cursor.position();
        while header.r#type != zbi::Type::Container || header.magic != zbi::ITEM_MAGIC {
            cur_pos += SEEK_ALIGNMENT;
            cursor.set_position(cur_pos);
            header = read_header(cursor)?;
        }
        cursor.set_position(cursor.position() - ZBI_HEADER_SIZE);
        info!(position:% = cursor.position(); "Found ZBI inside another container");
        Ok(())
    }
}

/// Test helpers for pre-computed zbi images.
pub mod test {
    use super::*;
    use crate::bootfs::test::*;

    /// Returns raw bytes for a zbi container that has no sections.
    pub fn empty_zbi_bytes() -> Vec<u8> {
        header_bytes(zbi::Type::Container, 0, 0, zbi::Flags::empty())
    }

    /// Returns raw bytes for a zbi container has exactly one section: a bootfs section containing no file entries.
    pub fn zbi_with_empty_bootfs_bytes() -> Vec<u8> {
        let section_data = empty_bootfs_bytes();
        let mut section_bytes = header_bytes(
            zbi::Type::StorageBootfs,
            section_data.len() as u32,
            0,
            zbi::Flags::empty(),
        );
        section_bytes.extend(&section_data);

        let mut zbi_bytes =
            header_bytes(zbi::Type::Container, section_bytes.len() as u32, 0, zbi::Flags::empty());
        zbi_bytes.extend(&section_bytes);
        zbi_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::test::*;
    use super::*;
    use crate::bootfs::test::*;

    #[test]
    fn test_zbi_empty_container() {
        let zbi_bytes = empty_zbi_bytes();
        let mut reader = ZbiReader::new(zbi_bytes);
        let sections = reader.parse().unwrap();
        assert_eq!(sections.len(), 0);
    }

    #[test]
    fn test_zbi_with_empty_bootfs() {
        let zbi_bytes = zbi_with_empty_bootfs_bytes();
        let mut reader = ZbiReader::new(zbi_bytes);
        let sections = reader.parse().unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].section_type, zbi::Type::StorageBootfs);
        assert_eq!(sections[0].buffer, empty_bootfs_bytes());
    }

    #[test]
    fn test_zbi_sections() {
        let section_data: Vec<u8> = vec![0; 10];
        let mut section_bytes = header_bytes(zbi::Type::Discard, 10, 10, zbi::Flags::empty());
        section_bytes.extend(&section_data);

        let mut zbi_bytes = header_bytes(
            zbi::Type::Container,
            u32::try_from(section_bytes.len()).unwrap(),
            0,
            zbi::Flags::empty(),
        );
        zbi_bytes.extend(&section_bytes);

        let mut reader = ZbiReader::new(zbi_bytes);
        let sections = reader.parse().unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].buffer.len(), 10);
    }

    #[test]
    fn test_zbi_compressed_sections() {
        let uncompressed_len: u32 = 4096;
        let uncompressed_data: Vec<u8> = vec![0; uncompressed_len.try_into().unwrap()];
        let section_data = zstd::compress(&uncompressed_data, uncompressed_len, 3).unwrap();

        let mut section_bytes = header_bytes(
            zbi::Type::Discard,
            u32::try_from(section_data.len()).unwrap(),
            4096,
            ZBI_FLAGS_STORAGE_COMPRESSED,
        );
        section_bytes.extend(&section_data);

        let mut zbi_bytes = header_bytes(
            zbi::Type::Container,
            u32::try_from(section_bytes.len()).unwrap(),
            0,
            zbi::Flags::empty(),
        );
        zbi_bytes.extend(&section_bytes);

        let mut reader = ZbiReader::new(zbi_bytes);
        let sections = reader.parse().unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].buffer.len(), uncompressed_len as usize);
    }

    #[test]
    fn test_zbi_sections_unaligned() {
        let section_data: Vec<u8> = vec![0; 7];
        let section_data_two: Vec<u8> = vec![0; 10];

        let mut section_bytes = header_bytes(zbi::Type::Discard, 7, 7, zbi::Flags::empty());
        section_bytes.extend(&section_data);
        let padding_len = 8 - (section_bytes.len() % 8);
        let padding = vec![0; padding_len];
        section_bytes.extend(&padding);
        section_bytes.extend(header_bytes(zbi::Type::Discard, 10, 10, zbi::Flags::empty()));
        section_bytes.extend(&section_data_two);

        let mut zbi_bytes = header_bytes(
            zbi::Type::Container,
            u32::try_from(section_bytes.len()).unwrap(),
            0,
            zbi::Flags::empty(),
        );
        zbi_bytes.extend(&section_bytes);

        let mut reader = ZbiReader::new(zbi_bytes);
        let sections = reader.parse().unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].buffer.len(), 7);
        assert_eq!(sections[1].buffer.len(), 10);
    }
}
