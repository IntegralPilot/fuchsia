// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::xattr;
use aes::Block;
use aes::cipher::inout::InOutBuf;
use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, BlockModeDecrypt, KeyInit, KeyIvInit};
use anyhow::Error;
use fscrypt::{Context, hkdf};
use siphasher::sip::SipHasher;
use std::hash::Hasher;
use zerocopy::{FromBytes, IntoBytes};

const NAME_XATTR_CRYPTO_CONTEXT: &[u8] = b"c";

pub fn try_read_context_from_xattr(
    xattr: &Vec<xattr::XattrEntry>,
) -> Result<Option<Context>, Error> {
    let raw_context = if let Some(content) = xattr.iter().find(|entry| {
        entry.index == xattr::Index::Encryption && *entry.name == *NAME_XATTR_CRYPTO_CONTEXT
    }) {
        &content.value
    } else {
        // No crypto context present.
        return Ok(None);
    };
    Context::try_from_bytes(raw_context.as_ref())
}

pub struct PerFileDecryptor {
    // For file contents (XTS)
    xts_key1: aes::Aes256,
    xts_key2: aes::Aes256,
    // For file names (CTS)
    cts_key: [u8; 32],
    // For seeding SipHasher for directory entry hashes
    dirhash_key: [u8; 16],

    // For INO_LBLK_32 policy
    ino_hash_key: Option<[u8; 16]>,
}

impl PerFileDecryptor {
    pub(super) fn new(main_key: &[u8; 64], context: Context, uuid: &[u8; 16]) -> Self {
        if context.flags & fscrypt::POLICY_FLAGS_INO_LBLK_32 != 0 {
            // To support eMMC inline crypto hardware (and hardware wrapped keys), the lblk_32
            // policy creates a shared key from the main key and filesystem UUID. In this mode the
            // inode is hashed and mixed into the tweak. The nonce is still used for dirhash.
            let mut hdkf_info = [0; 17];
            hdkf_info[1..17].copy_from_slice(uuid);
            hdkf_info[0] = fscrypt::ENCRYPTION_MODE_AES_256_XTS;
            let xts_key = hkdf::fscrypt_hkdf::<64>(
                main_key,
                &hdkf_info,
                hkdf::HKDF_CONTEXT_IV_INO_LBLK_32_KEY,
            );
            hdkf_info[0] = fscrypt::ENCRYPTION_MODE_AES_256_CTS;
            let cts_key = hkdf::fscrypt_hkdf::<32>(
                main_key,
                &hdkf_info,
                hkdf::HKDF_CONTEXT_IV_INO_LBLK_32_KEY,
            );
            let dirhash_key =
                hkdf::fscrypt_hkdf::<16>(main_key, &context.nonce, hkdf::HKDF_CONTEXT_DIRHASH_KEY);
            let ino_hash_key =
                Some(hkdf::fscrypt_hkdf::<16>(main_key, &[], hkdf::HKDF_CONTEXT_INODE_HASH_KEY));
            Self {
                xts_key1: aes::Aes256::new(xts_key[..32].try_into().unwrap()),
                xts_key2: aes::Aes256::new(xts_key[32..].try_into().unwrap()),
                cts_key,
                dirhash_key,
                ino_hash_key,
            }
        } else {
            // The default policy creates a unique key for each file using the main key and a
            // 16-byte nonce.
            let key = hkdf::fscrypt_hkdf::<64>(
                main_key,
                &context.nonce,
                hkdf::HKDF_CONTEXT_PER_FILE_ENC_KEY,
            );
            let dirhash_key =
                hkdf::fscrypt_hkdf::<16>(main_key, &context.nonce, hkdf::HKDF_CONTEXT_DIRHASH_KEY);
            let cts_key: [u8; 32] = key[..32].try_into().unwrap();
            Self {
                xts_key1: aes::Aes256::new(key[..32].try_into().unwrap()),
                xts_key2: aes::Aes256::new(key[32..].try_into().unwrap()),
                cts_key,
                dirhash_key,
                ino_hash_key: None,
            }
        }
    }

    pub fn decrypt_data(&self, ino: u32, block_num: u32, buffer: &mut [u8]) {
        assert_eq!((buffer.as_ptr() as usize) % 16, 0, "Require 16-byte aligned buffers");
        assert_eq!(buffer.len() % 16, 0, "Require buffters be multiple of 16-bytes");
        // TODO(https://fxbug.dev/406351838): Migrate to share implementation with fxfs-crypto?
        let key1 = self.xts_key1.clone();
        let key2 = self.xts_key2.clone();
        let mut tweak: u128 = if let Some(ino_hash_key) = self.ino_hash_key {
            let mut hasher = SipHasher::new_with_key(&ino_hash_key);
            let ino64 = ino as u64;
            hasher.write(ino64.as_bytes());
            hasher.finish() as u32 + block_num
        } else {
            block_num
        } as u128;
        let mut tweak_bytes = tweak.to_ne_bytes();
        key2.encrypt_block((&mut tweak_bytes).into());
        tweak = u128::from_ne_bytes(tweak_bytes);
        for chunk in buffer.chunks_exact_mut(16) {
            *u128::mut_from_bytes(chunk).unwrap() ^= tweak;
            let chunk_block: &mut Block = chunk.try_into().unwrap();
            key1.decrypt_block(chunk_block);
            *u128::mut_from_bytes(chunk).unwrap() ^= tweak;
            tweak = (tweak << 1) ^ (if tweak >> 127 != 0 { 0x87 } else { 0 });
        }
    }

    /// Decrypt a filename (from a dentry or symlink).
    pub fn decrypt_filename_data(&self, ino: u32, data: &mut [u8]) {
        let mut iv = [0u8; 16];
        if let Some(ino_hash_key) = self.ino_hash_key {
            let mut hasher = SipHasher::new_with_key(&ino_hash_key);
            hasher.write((ino as u64).as_bytes());
            iv[..4].copy_from_slice(&hasher.finish().as_bytes()[..4]);
        }
        // AES-256-CTS is used for filename encryption and symlinks but because we
        // require POLICY_FLAGS_PAD_16, we never actually steal any ciphertext and
        // so CTS is equivalent to swapping the last two blocks and using CBC instead.
        let mut cbc = cbc::Decryptor::<aes::Aes256>::new((&self.cts_key).into(), (&iv).into());
        let inout: InOutBuf<'_, '_, u8> = data.into();
        let (mut blocks, tail): (InOutBuf<'_, '_, Block>, _) = inout.into_chunks();
        debug_assert_eq!(tail.len(), 0);
        let mut chunks = blocks.get_out();
        if chunks.len() >= 2 {
            chunks.swap(chunks.len() - 1, chunks.len() - 2);
        }
        cbc.decrypt_blocks(&mut chunks);
    }

    pub fn dirhash_key(&self) -> &[u8; 16] {
        &self.dirhash_key
    }
}
