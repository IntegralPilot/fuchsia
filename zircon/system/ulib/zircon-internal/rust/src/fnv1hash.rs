// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const FNV32_PRIME: u32 = 16777619;
pub const FNV32_OFFSET_BASIS: u32 = 2166136261;

pub fn fnv1a32(data: &[u8]) -> u32 {
    let mut hash = FNV32_OFFSET_BASIS;
    for &byte in data {
        hash = hash ^ (byte as u32);
        hash = hash.wrapping_mul(FNV32_PRIME);
    }
    hash
}

pub const FNV64_PRIME: u64 = 1099511628211;
pub const FNV64_OFFSET_BASIS: u64 = 14695981039346656037;

pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash = FNV64_OFFSET_BASIS;
    for &byte in data {
        hash = hash ^ (byte as u64);
        hash = hash.wrapping_mul(FNV64_PRIME);
    }
    hash
}

pub fn fnv1a_tiny(mut n: u32, bits: u32) -> u32 {
    let mut hash = FNV32_OFFSET_BASIS;
    hash = (hash ^ (n & 0xFF)).wrapping_mul(FNV32_PRIME);
    n >>= 8;
    hash = (hash ^ (n & 0xFF)).wrapping_mul(FNV32_PRIME);
    n >>= 8;
    hash = (hash ^ (n & 0xFF)).wrapping_mul(FNV32_PRIME);
    n >>= 8;
    hash = (hash ^ n).wrapping_mul(FNV32_PRIME);
    ((hash >> bits) ^ hash) & ((1 << bits) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a32() {
        assert_eq!(fnv1a32(b""), FNV32_OFFSET_BASIS);
        assert_eq!(fnv1a32(b"foobar"), 0xbf9cf968);
    }

    #[test]
    fn test_fnv1a64() {
        assert_eq!(fnv1a64(b""), FNV64_OFFSET_BASIS);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn test_fnv1a_tiny() {
        assert_eq!(fnv1a_tiny(0, 8), 0xe0);
        assert_eq!(fnv1a_tiny(0x12345678, 12), 0x1cc);
    }
}
