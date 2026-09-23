// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use hmac::{KeyInit, Mac};

// Fscrypt tacks a prefix onto the 'info' field in HKDF used for different purposes.
// This prefix is built from one of the following context.
pub const HKDF_CONTEXT_KEY_IDENTIFIER: u8 = 1;
pub const HKDF_CONTEXT_PER_FILE_ENC_KEY: u8 = 2;
pub const HKDF_CONTEXT_DIRHASH_KEY: u8 = 5;
pub const HKDF_CONTEXT_IV_INO_LBLK_32_KEY: u8 = 6;
pub const HKDF_CONTEXT_INODE_HASH_KEY: u8 = 7;

/// An fscrypt compatible implementation of HKDF (HKDF-extract + HKDF-expand)
/// This is just regular HKDF but with 'info' prefixed.
/// `context` is an fscrypt special.
pub fn fscrypt_hkdf<const L: usize>(
    initial_key_material: &[u8],
    info: &[u8],
    context: u8,
) -> [u8; L] {
    let mut out = [0u8; L];
    let mut fscrypt_info = Vec::with_capacity(9 + info.len());
    fscrypt_info.extend_from_slice(b"fscrypt\0");
    fscrypt_info.push(context);
    debug_assert_eq!(fscrypt_info.len(), 9);
    fscrypt_info.extend_from_slice(info);
    hkdf::<L>(initial_key_material, &fscrypt_info, &mut out);
    out
}

/// Standard HKDF implementation. See https://datatracker.ietf.org/doc/html/rfc5869
/// Note that we assume an all-zero seed for PRK.
/// `initial_key_material` is the data being hashed.
/// `info` is optional context (can be zero length string)
/// `out` is populated with the result.
pub fn hkdf<const L: usize>(initial_key_material: &[u8], info: &[u8], out: &mut [u8; L]) {
    const HASH_LEN: usize = 64;
    // HKDF-extract
    let mut hmac = hmac::Hmac::<sha2::Sha512>::new_from_slice(&[0; HASH_LEN]).unwrap();
    hmac.update(initial_key_material);
    let prk = hmac.finalize().into_bytes();
    // HKDF-expand
    let mut last = [].as_slice();
    let mut out = out.as_mut_slice();
    let mut i = 1;
    loop {
        let mut hmac = hmac::Hmac::<sha2::Sha512>::new_from_slice(&prk).unwrap();
        hmac.update(&last);
        hmac.update(&info);
        hmac.update(&[i as u8]);
        let val = hmac.finalize().into_bytes();
        if out.len() < HASH_LEN {
            out.copy_from_slice(&val.as_slice()[..out.len()]);
            break;
        }
        out[..HASH_LEN].copy_from_slice(&val.as_slice());
        (last, out) = out.split_at_mut(HASH_LEN);
        i += 1;
    }
}
