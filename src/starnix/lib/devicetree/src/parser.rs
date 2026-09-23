// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::str::Utf8Error;

use zerocopy::{BigEndian, FromBytes, Immutable, KnownLayout, U32};

use crate::types::*;

/// The magic string that is expected to be present in `Header.magic`.
const FDT_MAGIC: u32 = 0xd00dfeed;

/// Marks the beginning of a node's representation. It is followed by the node's
/// unit name as extra data. The name is stored as a null-terminated string.
const FDT_BEGIN_NODE: u32 = 0x00000001;

/// Marks the end of a node's representation.
const FDT_END_NODE: u32 = 0x00000002;

/// Marks the beginning of a property in the device tree.
const FDT_PROP: u32 = 0x00000003;

/// A token that is meant to be ignored by the parser.
const FDT_NOP: u32 = 0x00000004;

/// Marks the end of the structure block. There should only be one, and it should be the
/// last token in the structure block.
const FDT_END: u32 = 0x00000009;

#[derive(Debug)]
pub enum ParseError {
    InvalidMagicNumber,
    UnsupportedVersion,
    Utf8Error(Utf8Error),
    MalformedStructure(String),
    ZeroCopyError,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidMagicNumber => write!(f, "Invalid magic number"),
            ParseError::UnsupportedVersion => write!(f, "Unsupported version"),
            ParseError::Utf8Error(e) => write!(f, "Utf8 error: {}", e),
            ParseError::MalformedStructure(e) => write!(f, "Malformed structure: {}", e),
            ParseError::ZeroCopyError => write!(f, "Zero copy error"),
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ParseError::Utf8Error(e) => Some(e),
            _ => None,
        }
    }
}

/// Parses a full `Devicetree` instance from `data`.
pub fn parse_devicetree(data: &[u8]) -> Result<Devicetree, ParseError> {
    // Parse and verify the header.
    let header = *parse_item::<Header>(&data)?;
    verify_header(&header)?;

    // Parse the memory reservation block.
    let off_mem_rsvmap = header.off_mem_rsvmap.get() as usize;
    let reserve_entries = parse_reserve_entries(&data[off_mem_rsvmap..])?;

    // Create the strings section.
    let off_dt_strings = header.off_dt_strings.get() as usize;
    let size_dt_strings = header.size_dt_strings.get() as usize;
    let string_data = verify_slice(&data, off_dt_strings, size_dt_strings)?;

    // Parse the structure block.
    let off_dt_struct = header.off_dt_struct.get() as usize;
    let size_dt_struct = header.size_dt_struct.get() as usize;
    let structure_data = verify_slice(&data, off_dt_struct, size_dt_struct)?;
    let mut struct_offset = 0;
    // When `parse_structure_block` returns, `struct_offset` will contain the number of bytes that
    // were parsed for the structure block.
    let root_node = parse_structure_block(&structure_data, &mut struct_offset, &string_data)?;

    // Verify that the last token is `FDT_END`, reading from the end of the structure block.
    let token = parse_item::<U32<BigEndian>>(&structure_data[struct_offset..])?.get();
    if token != FDT_END {
        return Err(ParseError::MalformedStructure(format!(
            "Expected FDT_END token at the end of structure block, got 0x{:x}",
            token
        )));
    }

    Ok(Devicetree { header, reserve_entries, root_node })
}

// Verifies that the provided `Header` is a valid header.
fn verify_header(header: &Header) -> Result<(), ParseError> {
    if header.magic.get() != FDT_MAGIC {
        return Err(ParseError::InvalidMagicNumber);
    }
    if header.version.get() < 17 {
        return Err(ParseError::UnsupportedVersion);
    }

    Ok(())
}

/// Creates a slice from `data` from `offset` to `offset + size`.
///
/// Returns an error if the slice is out of bounds of `data`.
fn verify_slice(data: &[u8], offset: usize, size: usize) -> Result<&[u8], ParseError> {
    if offset + size > data.len() {
        return Err(ParseError::MalformedStructure(format!(
            "Attempted to create slice from 0x{:x} to 0x{:x}, where total length was 0x{:x}",
            offset,
            offset + size,
            data.len()
        )));
    }
    Ok(&data[offset..offset + size])
}

/// Parses a &T out of `data`.
///
/// Returns an error if the parse failed.
fn parse_item<'a, T: FromBytes + KnownLayout + Immutable>(
    data: &'a [u8],
) -> Result<&'a T, ParseError> {
    let token = T::ref_from_prefix(&data).map_err(|_| ParseError::ZeroCopyError)?;
    Ok(token.0)
}

/// Parses a &T out of `data`, and increments offset by `size_of::<T>()` if the parse
/// was successful.
///
/// Returns an error if the parse failed.
fn parse_item_and_increment_offset<'a, T: FromBytes + KnownLayout + Immutable>(
    data: &'a [u8],
    offset: &mut usize,
) -> Result<&'a T, ParseError> {
    let r = parse_item(data)?;
    *offset += std::mem::size_of::<T>();

    Ok(r)
}

/// Parses a null-terminated string from `data`.
///
/// When the function returns, `offset` is set to the index after the null character, aligned to a
/// 4-byte boundary.
///
/// Returns an error if the string is not null-terminated, or the string contains invalid utf8
/// characters.
fn parse_string(data: &[u8], offset: &mut usize) -> Result<String, ParseError> {
    let str_slice = &data[*offset..];
    let null_index = str_slice.iter().position(|c| *c == 0).ok_or(ParseError::ZeroCopyError)?;
    *offset += null_index + 1;
    // Align the offset to the next 4-byte boundary.
    *offset = (*offset + 3) & !3;

    std::str::from_utf8(&str_slice[..null_index])
        .map_err(|e| ParseError::Utf8Error(e))
        .map(|s| s.to_string())
}

/// Parses the structure block from `data`.
///
/// The `offset` is used for recursive calls, and will be set to the end of the parsed node when
/// the function returns.
///
/// `strings_block` is a reference to the strings data, and offsets read from properties will be
/// used to index into this data.
fn parse_structure_block(
    data: &[u8],
    offset: &mut usize,
    strings_block: &[u8],
) -> Result<Node, ParseError> {
    let token = parse_item_and_increment_offset::<U32<BigEndian>>(&data[*offset..], offset)?;
    if *token != FDT_BEGIN_NODE {
        return Err(ParseError::MalformedStructure(format!(
            "Expected FDT_BEGIN_NODE token at offset 0x{:x}, got 0x{:x}",
            offset, token
        )));
    }

    let name = parse_string(&data, offset)?;
    let mut node = Node::new(name);

    loop {
        let token =
            parse_item_and_increment_offset::<U32<BigEndian>>(&data[*offset..], offset)?.get();
        match token {
            token if token == FDT_BEGIN_NODE => {
                // Reset the offset to parse it again in the recursive call.
                *offset -= std::mem::size_of_val(&token);

                let child_node = parse_structure_block(data, offset, strings_block)?;
                node.children.push(child_node);
            }
            token if token == FDT_PROP => {
                let length =
                    parse_item_and_increment_offset::<U32<BigEndian>>(&data[*offset..], offset)?
                        .get() as usize;

                let mut prop_name_offset =
                    parse_item_and_increment_offset::<U32<BigEndian>>(&data[*offset..], offset)?
                        .get() as usize;
                let prop_name = parse_string(&strings_block, &mut prop_name_offset)?;

                // Parse the property value, and align the offset to the next 4-byte boundary.
                let value = data[*offset..*offset + length].to_vec();
                *offset += length;
                *offset = (*offset + 3) & !3;

                node.properties.push(Property { name: prop_name, value });
            }
            token if token == FDT_END_NODE => {
                return Ok(node);
            }
            token if token == FDT_NOP => {}
            token if token == FDT_END => {
                return Err(ParseError::MalformedStructure("FDT_END inside of node".to_string()));
            }
            token => {
                return Err(ParseError::MalformedStructure(format!(
                    "Expected valid token at offset 0x{:x}, got 0x{:x}",
                    offset, token
                )));
            }
        }
    }
}

/// Parses an array of `ReserveEntry`'s from `data`.
fn parse_reserve_entries(data: &[u8]) -> Result<Vec<ReserveEntry>, ParseError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    loop {
        let entry: &ReserveEntry = ReserveEntry::ref_from_prefix(&data[offset..])
            .map_err(|_| {
                ParseError::MalformedStructure(
                    "Memory reservation map entry too small or misaligned.".to_string(),
                )
            })?
            .0;

        // If both the address and the size are zero, this is the end of the entries.
        if entry.address.get() == 0 && entry.size.get() == 0 {
            return Ok(entries);
        }

        entries.push(*entry);
        offset += std::mem::size_of::<ReserveEntry>();
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn test_vim3() {
        let contents = std::fs::read("pkg/test-data/test.dtb").expect("failed to read file");
        crate::parser::parse_devicetree(&contents).expect("failed to parse devicetree");
    }
}
