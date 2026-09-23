// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//! Lexicographically ordered variable-length integer (varint) encoding.
//!
//! This encoding ensures that the byte-wise (lexicographical) comparison of encoded values
//! matches the numerical comparison of the original values.
//!
//! This is achieved by:
//! 1. Dividing the value space into distinct length categories.
//! 2. Assigning each category a non-overlapping range of first-byte values, ordered by length.
//! 3. Encoding the remaining bits in Big-Endian order (which naturally matches numerical order).
//!
//! Encoding rules (bit-level):
//! - `v < 0xc0` (0..191): 1 byte
//!   - Pattern: `0b0xxxxxxx` or `0b10xxxxxx`
//!   - First byte range: `0x00..=0xbf`
//! - `v < 0x2000` (192..8191): 2 bytes
//!   - Encoded as `v | 0xc000` (Big Endian)
//!   - Pattern: `0b110xxxxx xxxxxxxx`
//!   - First byte range: `0xc0..=0xdf`
//! - `v < 0x1000_0000` (8192..268435455): 4 bytes
//!   - Encoded as `v | 0xe000_0000` (Big Endian)
//!   - Pattern: `0b1110xxxx xxxxxxxx ...`
//!   - First byte range: `0xe0..=0xef`
//! - `v < 0x0f00_0000_0000_0000`: 8 bytes
//!   - Encoded as `v | 0xf000_0000_0000_0000` (Big Endian)
//!   - Pattern: `0b1111xxxx xxxxxxxx ...`
//!   - First byte range: `0xf0..=0xfe`
//! - Else: 9 bytes
//!   - Encoded as `0xff` followed by `v.to_be_bytes()`
//!   - Pattern: `0b11111111 xxxxxxxx ...`
//!   - First byte: `0xff`

use anyhow::{Error, ensure};

/// Abstraction over a contiguous in-memory byte buffer that supports appending and truncation.
pub trait Buffer: AsRef<[u8]> + AsMut<[u8]> + Send {
    /// Appends data to the end of the buffer.
    fn put(&mut self, data: &[u8]);

    /// Truncates the buffer to `len` bytes.
    fn truncate(&mut self, len: usize);
}

impl Buffer for Vec<u8> {
    fn put(&mut self, data: &[u8]) {
        self.extend_from_slice(data);
    }
    fn truncate(&mut self, len: usize) {
        self.truncate(len);
    }
}

/// Encodes `v` into an order-preserving varint returned as a fixed-size byte array and length.
#[inline]
pub fn encode_varint_bytes(v: u64) -> ([u8; 9], usize) {
    let mut buf = [0u8; 9];
    let len = if v < 0xc0 {
        buf[0] = v as u8;
        1
    } else if v < 0x2000 {
        buf[..2].copy_from_slice(&(v as u16 | 0xc000).to_be_bytes());
        2
    } else if v < 0x1000_0000 {
        buf[..4].copy_from_slice(&(v as u32 | 0xe000_0000).to_be_bytes());
        4
    } else if v < 0x0f00_0000_0000_0000 {
        buf[..8].copy_from_slice(&(v | 0xf000_0000_0000_0000).to_be_bytes());
        8
    } else {
        buf[0] = 0xff;
        buf[1..9].copy_from_slice(&v.to_be_bytes());
        9
    };
    (buf, len)
}

/// Encodes `v` into an order-preserving varint directly written to `buf`.
#[inline]
pub fn encode_varint(v: u64, buf: &mut impl Buffer) -> usize {
    if v < 0xc0 {
        buf.put(&[v as u8]);
        1
    } else if v < 0x2000 {
        buf.put(&(v as u16 | 0xc000).to_be_bytes());
        2
    } else if v < 0x1000_0000 {
        buf.put(&(v as u32 | 0xe000_0000).to_be_bytes());
        4
    } else if v < 0x0f00_0000_0000_0000 {
        buf.put(&(v | 0xf000_0000_0000_0000).to_be_bytes());
        8
    } else {
        let mut b = [0u8; 9];
        b[0] = 0xff;
        b[1..9].copy_from_slice(&v.to_be_bytes());
        buf.put(&b);
        9
    }
}

/// Decodes an order-preserving varint from the beginning of `data`.
///
/// Returns the decoded `u64` value and the remaining unconsumed slice.
#[inline]
pub fn decode_varint<'a>(data: &'a [u8]) -> Result<(u64, &'a [u8]), Error> {
    ensure!(!data.is_empty(), "Data too short");
    let b = data[0];
    let (val, len) = if b < 0xc0 {
        (b as u64, 1)
    } else if b < 0xe0 {
        ensure!(data.len() >= 2, "Data too short");
        let v = u16::from_be_bytes(data[..2].try_into().unwrap());
        let val = (v & !0xc000) as u64;
        ensure!(val >= 0xc0, "Non-canonical varint");
        (val, 2)
    } else if b < 0xf0 {
        ensure!(data.len() >= 4, "Data too short");
        let v = u32::from_be_bytes(data[..4].try_into().unwrap());
        let val = (v & !0xe000_0000) as u64;
        ensure!(val >= 0x2000, "Non-canonical varint");
        (val, 4)
    } else if b < 0xff {
        ensure!(data.len() >= 8, "Data too short");
        let v = u64::from_be_bytes(data[..8].try_into().unwrap());
        let val = v & !0xf000_0000_0000_0000;
        ensure!(val >= 0x1000_0000, "Non-canonical varint");
        (val, 8)
    } else {
        ensure!(data.len() >= 9, "Data too short");
        let val = u64::from_be_bytes(data[1..9].try_into().unwrap());
        ensure!(val >= 0x0f00_0000_0000_0000, "Non-canonical varint");
        (val, 9)
    };
    Ok((val, &data[len..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_correctness() {
        let test_cases = vec![
            0,
            1,
            0xbe,
            0xbf,
            0xc0,
            0x1ffe,
            0x1fff,
            0x2000,
            0x0fff_ffff,
            0x1000_0000,
            0x0eff_ffff_ffff_ffff,
            0x0f00_0000_0000_0000,
            u64::MAX,
        ];
        for v in test_cases {
            let mut buf = Vec::new();
            let len = encode_varint(v, &mut buf);
            let (decoded, remainder) = decode_varint(&buf).unwrap();
            assert_eq!(v, decoded);
            assert_eq!(remainder.len(), 0);
            assert_eq!(buf.len(), len);

            let (bytes, bytes_len) = encode_varint_bytes(v);
            assert_eq!(len, bytes_len);
            assert_eq!(&buf[..], &bytes[..bytes_len]);
        }
    }

    #[test]
    fn test_ordering_correctness() {
        let test_cases = vec![
            0,
            1,
            0xbe,
            0xbf,
            0xc0,
            0x1ffe,
            0x1fff,
            0x2000,
            0x0fff_ffff,
            0x1000_0000,
            0x0eff_ffff_ffff_ffff,
            0x0f00_0000_0000_0000,
            u64::MAX,
        ];
        for i in 0..test_cases.len() {
            for j in 0..test_cases.len() {
                let a = test_cases[i];
                let b = test_cases[j];

                let mut buf_a = Vec::new();
                let mut buf_b = Vec::new();

                encode_varint(a, &mut buf_a);
                encode_varint(b, &mut buf_b);

                let ord_cmp = a.cmp(&b);
                let ser_cmp = buf_a.cmp(&buf_b);
                assert_eq!(ord_cmp, ser_cmp, "Mismatch for {} and {}", a, b);
            }
        }
    }

    #[test]
    fn test_non_canonical() {
        assert!(decode_varint(&[0xc0, 0x03]).is_err());
        assert!(decode_varint(&[0xe0, 0x00, 0x1f, 0xff]).is_err());
        assert!(decode_varint(&[0xf0, 0x00, 0x00, 0x00, 0x0f, 0xff, 0xff, 0xff]).is_err());
        assert!(decode_varint(&[0xff, 0x0e, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).is_err());
    }
}
