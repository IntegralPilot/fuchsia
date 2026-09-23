// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
pub mod direntry;
pub mod hkdf;
pub mod proxy_filename;

use anyhow::{Error, anyhow, ensure};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

pub const POLICY_FLAGS_PAD_16: u8 = 0x02;
pub const POLICY_FLAGS_INO_LBLK_32: u8 = 0x10;
const SUPPORTED_POLICY_FLAGS: u8 = POLICY_FLAGS_PAD_16 | POLICY_FLAGS_INO_LBLK_32;

pub const ENCRYPTION_MODE_AES_256_XTS: u8 = 1;
pub const ENCRYPTION_MODE_AES_256_CTS: u8 = 4;

/// An encryption context is written as an xattr to the directory root of each FBE hierarchy.
/// For f2fs, this is stored with index 9 and name "c".
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Immutable, KnownLayout, FromBytes, IntoBytes, Unaligned)]
pub struct Context {
    pub version: u8,                   // = 2
    pub contents_encryption_mode: u8,  // = ENCRYPTION_MODE_AES_256_XTS
    pub filenames_encryption_mode: u8, // = ENCRYPTION_MODE_AES_256_CTS
    pub flags: u8,                     // = POLICY_FLAGS_*
    pub log2_data_unit_size: u8,       // = 0
    _reserved: [u8; 3],
    pub main_key_identifier: [u8; 16],
    pub nonce: [u8; 16],
}

impl Context {
    pub fn try_from_bytes(raw_context: &[u8]) -> Result<Option<Self>, Error> {
        let this = Context::read_from_bytes(raw_context)
            .map_err(|_| anyhow!("Bad sized crypto context"))?;
        ensure!(this.version == 2, "Bad version number in crypto context");
        ensure!(
            this.contents_encryption_mode == ENCRYPTION_MODE_AES_256_XTS,
            "Unsupported contents_encryption_mode",
        );
        ensure!(
            this.filenames_encryption_mode == ENCRYPTION_MODE_AES_256_CTS,
            "Unsupported filenames_encryption_mode"
        );
        // We assume 16 byte zero padding.
        // We also only support standard key derivation and INO_LBLK_32, no INO_LBLK_64 or DIRECT.
        ensure!(this.flags & !SUPPORTED_POLICY_FLAGS == 0, "Unsupported flags in crypto context");
        // This controls the data unit size used for encryption blocks. We only support the default.
        ensure!(this.log2_data_unit_size == 0, "Unsupported custom DUN size");
        Ok(Some(this))
    }
}

/// Returns the identifier for a given main key.
pub fn main_key_to_identifier(main_key: &[u8; 64]) -> [u8; 16] {
    hkdf::fscrypt_hkdf::<16>(main_key, &[], 1)
}

pub struct DirectoryKeys {
    cts_key: [u8; 32],
    ino_hash_key: [u8; 16],
    dir_hash_key: [u8; 16],
}

impl DirectoryKeys {
    /// Returns the keys in concatenated form (as found in Fxfs's crypt protocol).
    pub fn to_unwrapped_key(&self) -> Vec<u8> {
        let mut keys = Vec::with_capacity(64);
        keys.extend_from_slice(&self.cts_key);
        keys.extend_from_slice(&self.ino_hash_key);
        keys.extend_from_slice(&self.dir_hash_key);
        keys
    }
}

/// Returns fscrypt directory keys (for the ino-lblk32 algorithm).  These are all the keys
/// required to encrypt file names using fscrypt.
pub fn to_directory_keys(main_key: &[u8], uuid: &[u8], nonce: &[u8]) -> DirectoryKeys {
    let mut hdkf_info = [0; 17];
    hdkf_info[0] = ENCRYPTION_MODE_AES_256_CTS;
    hdkf_info[1..17].copy_from_slice(&uuid);
    DirectoryKeys {
        cts_key: hkdf::fscrypt_hkdf(main_key, &hdkf_info, hkdf::HKDF_CONTEXT_IV_INO_LBLK_32_KEY),
        ino_hash_key: hkdf::fscrypt_hkdf(main_key, &[], hkdf::HKDF_CONTEXT_INODE_HASH_KEY),
        dir_hash_key: hkdf::fscrypt_hkdf(main_key, &nonce, hkdf::HKDF_CONTEXT_DIRHASH_KEY),
    }
}

pub fn to_xts_key(main_key: &[u8], uuid: [u8; 16]) -> [u8; 64] {
    let mut hdkf_info = [0; 17];
    hdkf_info[0] = ENCRYPTION_MODE_AES_256_XTS;
    hdkf_info[1..17].copy_from_slice(&uuid);
    hkdf::fscrypt_hkdf(&main_key, &hdkf_info, hkdf::HKDF_CONTEXT_IV_INO_LBLK_32_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_main_key_to_identifier() {
        // Nb: Hard coded test vector from an fscrypt instance.
        let key_digest = "dc34d175ba21b27e2e92829b0dc12666ce8bfbcbae387014c6bb0d8b7678dafa6466bd7565b1a5999cd3f8a39a470528fa6816768e6985f0b10804af7d657810";
        let key: [u8; 64] = hex::decode(&key_digest).unwrap().try_into().unwrap();
        assert_eq!(hex::encode(main_key_to_identifier(&key)), "fc7f69a149f89a7529374cf9e96a6d13");
    }
}
