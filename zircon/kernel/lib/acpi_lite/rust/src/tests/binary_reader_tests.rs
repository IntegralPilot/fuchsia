// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![cfg(test)]

use crate::binary_reader::{BinaryReader, Unaligned};
use crate::structures::VariableSized;

#[derive(
    Copy,
    Clone,
    zerocopy::FromBytes,
    zerocopy::Unaligned,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
)]
#[repr(C, packed)]
struct Header {
    length: u32,
}

impl VariableSized for Header {
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
)]
#[repr(C, packed)]
struct Payload {
    header: Header,
    payload: u32,
}

impl VariableSized for Payload {
    fn size(&self) -> usize {
        self.header.size()
    }
}

#[test]
fn test_empty() {
    let mut reader = BinaryReader::new(&[]);
    assert!(reader.is_empty());
    assert!(reader.read_fixed_length::<u8>().is_none());
    assert!(reader.read::<Header>().is_none());
    assert!(reader.skip_bytes(0));
    assert!(!reader.skip_bytes(1));
}

#[test]
fn test_read_struct() {
    let payload =
        Payload { header: Header { length: core::mem::size_of::<Payload>() as u32 }, payload: 42 };

    let payload_bytes = unsafe {
        core::slice::from_raw_parts(
            &payload as *const Payload as *const u8,
            core::mem::size_of::<Payload>(),
        )
    };

    // Ensure we can read the full struct.
    let mut reader = BinaryReader::new(payload_bytes);
    let read_payload = reader.read::<Payload>().unwrap();
    let val = read_payload.payload;
    assert_eq!(val, 42);

    // Ensure we can't read the struct if there is insufficient bytes.
    let mut reader = BinaryReader::new(&payload_bytes[..payload_bytes.len() - 1]);
    assert!(reader.read::<Payload>().is_none());
}

#[test]
fn test_skip_bytes() {
    let payload =
        Payload { header: Header { length: core::mem::size_of::<Payload>() as u32 }, payload: 42 };

    let payload_bytes = unsafe {
        core::slice::from_raw_parts(
            &payload as *const Payload as *const u8,
            core::mem::size_of::<Payload>(),
        )
    };

    // Seek past the header to the payload.
    let mut reader = BinaryReader::new(payload_bytes);
    assert!(reader.skip_bytes(core::mem::size_of::<Header>()));

    // Read the payload.
    let val = reader.read_fixed_length::<Unaligned<u32>>().unwrap();
    let val_copied = val.0;
    assert_eq!(val_copied, 42);

    // Can't skip any more.
    assert!(!reader.skip_bytes(1));
    assert!(reader.is_empty());
}
