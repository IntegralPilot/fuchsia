// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::{Error, anyhow, ensure};
use enumn::N;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Ref, Unaligned};

pub const XATTR_MAGIC: u32 = 0xF2F52011;

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Immutable, KnownLayout, FromBytes, IntoBytes, Unaligned)]
pub struct XattrHeader {
    pub magic: u32, // XATTR_MAGIC
    pub ref_count: u32,
    _reserved: [u32; 4],
}

/// xattr have an attached 'index' that serves as a namespace/purpose.
#[derive(Copy, Clone, Debug, Eq, PartialEq, N)]
#[repr(u8)]
pub enum Index {
    Unused0 = 0,
    User = 1,
    PosixAclAccess = 2,
    PosixAclDefault = 3,
    Trusted = 4,
    Lustre = 5,
    Security = 6,
    IndexAdvise = 7,
    Unused8 = 8,
    Encryption = 9,
    Unused10 = 10,
    Verity = 11,
}

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Immutable, KnownLayout, FromBytes, IntoBytes, Unaligned)]
struct RawXattrEntry {
    name_index: u8,
    // Length in bytes of name.
    name_len: u8,
    // Length in bytes of value.
    value_size: u16,
    // Note: name follows, then value.
    // Struct should be padded to u32. i.e. (padding_len + name_len + value_len) % 4 = 0
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XattrEntry {
    // Index dictates use. 1 -> user, 6 -> security, 9 -> encryption, 11 -> verity...
    pub index: Index,
    pub name: Box<[u8]>,
    pub value: Box<[u8]>,
}

/// Returns all xattr for this inode as a set of key/value pairs.
/// Requires another read to look up the xattr node.
pub fn decode_xattr(raw_data: &[u8]) -> Result<Vec<XattrEntry>, Error> {
    if raw_data.is_empty() {
        return Ok(vec![]);
    }
    let (header, mut rest): (Ref<_, XattrHeader>, _) =
        Ref::from_prefix(raw_data).map_err(|_| anyhow!("xattr too small"))?;
    if header.magic != XATTR_MAGIC {
        return Ok(vec![]);
    }

    let mut entries = Vec::new();
    while rest.len() >= std::mem::size_of::<RawXattrEntry>() {
        let (entry, remainder): (Ref<_, RawXattrEntry>, _) = Ref::from_prefix(rest).unwrap();
        rest = remainder;
        if entry.name_index > 0 {
            let index =
                Index::n(entry.name_index).ok_or_else(|| anyhow!("Unexpected xattr index"))?;
            ensure!(
                (entry.name_len as usize + entry.value_size as usize) <= rest.len(),
                "invalid name_len/value_size in xattr"
            );
            let padding_len = 3 - (entry.name_len as usize + entry.value_size as usize + 3) % 4;
            // TODO(https://fxbug.dev/450685631): Add a test for xattr padding.

            let name = rest[..entry.name_len as usize].to_owned().into_boxed_slice();
            rest = &rest[entry.name_len as usize..];
            let value = rest[..entry.value_size as usize].to_owned().into_boxed_slice();
            rest = &rest[entry.value_size as usize + padding_len..];
            entries.push(XattrEntry { index, name, value });
        } else {
            break;
        }
    }
    Ok(entries)
}
