// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fxfs_unicode::CasefoldStr;
use siphasher::sip::SipHasher;
use std::hash::Hasher;

// We have to match the hash_code implementation used by f2fs to migrate direntry without
// decrypting them.

// See: https://en.wikipedia.org/wiki/Tiny_Encryption_Algorithm
fn tea(input: &[u32; 4], buf: &mut [u32; 2]) {
    const DELTA: u32 = 0x9e3779b9;
    let mut sum = 0u32;
    let mut v = buf.clone();
    for _ in 0..16 {
        sum = sum.wrapping_add(DELTA);
        v[0] = v[0].wrapping_add(
            (v[1] << 4).wrapping_add(input[0])
                ^ v[1].wrapping_add(sum)
                ^ (v[1] >> 5).wrapping_add(input[1]),
        );
        v[1] = v[1].wrapping_add(
            (v[0] << 4).wrapping_add(input[2])
                ^ v[0].wrapping_add(sum)
                ^ (v[0] >> 5).wrapping_add(input[3]),
        );
    }
    buf[0] = buf[0].wrapping_add(v[0]);
    buf[1] = buf[1].wrapping_add(v[1]);
}

/// This algorithm is not cryptographically secure. It is used for
/// filename hashing for compatibility reasons. `name_bytes` can be one of:
///   * A case-sensitive UTF-8 filename (unencrypted)
///   * A casefolded (+ nfd + default_ignorable) filename (unencrypted)
///   * An encrypted case-sensitive UTF-8 filename (hashed over encrypted bytes)
/// Encrypted casefolded filenames always use `casefold_encrypt_hash_filename()`.
pub fn tea_hash_filename(name_bytes: impl IntoIterator<Item = u8>) -> u32 {
    let mut name_bytes = name_bytes.into_iter().peekable();
    if name_bytes.peek().is_none() {
        return 0;
    }
    // The tea hash algorithm operates on groups of [4; u32], but the u32
    // need to be big-endian.
    let mut buf = [0x67452301, 0xefcdab89];
    let mut done = false;
    let mut bytes = [0u8; 16];
    while !done {
        let mut out = 0;
        while out < 16 {
            let Some(c) = name_bytes.next() else {
                if out == 0 {
                    return buf[0];
                }
                // Pad the remainder of the last buffer with the length of this
                // chunk.
                bytes[out..].fill(out as u8);
                done = true;
                break;
            };
            bytes[out] = c;
            out += 1;
        }
        let mut k = [
            u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
            u32::from_be_bytes(bytes[4..8].try_into().unwrap()),
            u32::from_be_bytes(bytes[8..12].try_into().unwrap()),
            u32::from_be_bytes(bytes[12..16].try_into().unwrap()),
        ];
        if done {
            // Due to a quirk of how the buffer is filled, the last u32 needs
            // to be rotated.
            k[out / 4] = k[out / 4].rotate_left(out as u32 % 4 * 8);
        }
        tea(&k, &mut buf);
    }
    buf[0]
}

/// Hashes a filename for direntry indexing when both encryption and casefolding are enabled.
/// Under this configuration, we need to avoid the hash_code leaking details of the filename
/// but we also need it to be case insensitive so we cannot simply hash the encrypted form.
pub fn casefold_encrypt_hash_filename(filename: &CasefoldStr, dirhash_key: &[u8; 16]) -> u32 {
    if filename.as_str().is_empty() {
        return 0;
    }
    let mut hasher = SipHasher::new_with_key(dirhash_key);
    for b in filename.casefold_normalized_chars().flat_map(fxfs_unicode::utf8_bytes) {
        hasher.write_u8(b);
    }
    hasher.finish() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tea_hash_empty_filename() {
        assert_eq!(tea_hash_filename(std::iter::empty()), 0);
        // Verify non-empty still works (regression check)
        assert_ne!(tea_hash_filename(b"a".iter().copied()), 0);
    }

    #[test]
    fn test_casefold_encrypt_hash_empty_filename() {
        let key = [0u8; 16];
        assert_eq!(casefold_encrypt_hash_filename("".into(), &key), 0);
        // Verify non-empty still works
        assert_ne!(casefold_encrypt_hash_filename("a".into(), &key), 0);
    }
}
