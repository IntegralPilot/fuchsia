// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use super::{Cipher, SECTOR_SIZE, Tweak, UnwrappedKey, XtsInPlaceProcessor, XtsProcessor};
use aes::Aes256;
use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use anyhow::Error;
use log::warn;
pub use storage_ptr_slice::{MutPtrByteSlice, PtrByteSlice};
use zerocopy::IntoBytes;

#[derive(Debug)]
pub struct FxfsCipher {
    key: Aes256,
}
impl FxfsCipher {
    pub fn new(key: &UnwrappedKey) -> Self {
        Self { key: Aes256::new(key.as_slice().try_into().unwrap()) }
    }
}
impl Cipher for FxfsCipher {
    fn encrypt(
        &self,
        _ino: u64,
        attribute_id: u64,
        _device_offset: u64,
        file_offset: u64,
        mut buffer: MutPtrByteSlice<'_>,
    ) -> Result<(), Error> {
        fxfs_trace::duration!("encrypt", "len" => buffer.len());
        assert_eq!(file_offset % SECTOR_SIZE, 0);
        let mut sector_offset = file_offset / SECTOR_SIZE;
        assert_eq!(buffer.len() % (SECTOR_SIZE as usize), 0);
        let upper_tweak = (attribute_id as u128) << 64;
        let mut offset = 0;
        while offset < buffer.len() {
            let sector = buffer.reborrow().subslice_mut(offset..offset + SECTOR_SIZE as usize);
            let mut tweak = Tweak(upper_tweak | (sector_offset as u128));
            // The same key is used for encrypting the data and computing the tweak.
            self.key.encrypt_block(tweak.as_mut_bytes().try_into().unwrap());
            self.key.encrypt_with_backend(XtsInPlaceProcessor::new(tweak, sector));
            sector_offset += 1;
            offset += SECTOR_SIZE as usize;
        }
        Ok(())
    }

    fn decrypt(
        &self,
        _ino: u64,
        attribute_id: u64,
        _device_offset: u64,
        file_offset: u64,
        mut buffer: MutPtrByteSlice<'_>,
    ) -> Result<(), Error> {
        fxfs_trace::duration!("decrypt", "len" => buffer.len());
        assert_eq!(file_offset % SECTOR_SIZE, 0);
        let mut sector_offset = file_offset / SECTOR_SIZE;
        assert_eq!(buffer.len() % (SECTOR_SIZE as usize), 0);
        let upper_tweak = (attribute_id as u128) << 64;
        let mut offset = 0;
        while offset < buffer.len() {
            let sector = buffer.reborrow().subslice_mut(offset..offset + SECTOR_SIZE as usize);
            let mut tweak = Tweak(upper_tweak | (sector_offset as u128));
            // The same key is used for encrypting the data and computing the tweak.
            self.key.encrypt_block(tweak.as_mut_bytes().try_into().unwrap());
            self.key.decrypt_with_backend(XtsInPlaceProcessor::new(tweak, sector));
            sector_offset += 1;
            offset += SECTOR_SIZE as usize;
        }
        Ok(())
    }

    fn decrypt_to(
        &self,
        _ino: u64,
        attribute_id: u64,
        _device_offset: u64,
        file_offset: u64,
        src: PtrByteSlice<'_>,
        mut dst: MutPtrByteSlice<'_>,
    ) -> Result<(), Error> {
        fxfs_trace::duration!("decrypt_to", "len" => src.len());
        assert_eq!(src.len(), dst.len());
        assert_eq!(file_offset % SECTOR_SIZE, 0);
        let mut sector_offset = file_offset / SECTOR_SIZE;
        assert_eq!(src.len() % (SECTOR_SIZE as usize), 0);
        let upper_tweak = (attribute_id as u128) << 64;
        let mut offset = 0;
        while offset < src.len() {
            let src_sector = src.subslice(offset..offset + SECTOR_SIZE as usize);
            let dst_sector = dst.reborrow().subslice_mut(offset..offset + SECTOR_SIZE as usize);
            let mut tweak = Tweak(upper_tweak | (sector_offset as u128));
            // The same key is used for encrypting the data and computing the tweak.
            self.key.encrypt_block(tweak.as_mut_bytes().try_into().unwrap());
            // Both src and destination should be aligned to 64 bytes.
            self.key.decrypt_with_backend(XtsProcessor::new(tweak, src_sector, dst_sector));
            sector_offset += 1;
            offset += SECTOR_SIZE as usize;
        }
        Ok(())
    }

    fn encrypt_filename(&self, _object_id: u64, _buffer: &mut Vec<u8>) -> Result<(), Error> {
        debug_assert!(false, "encrypt_filename called on fxfs cipher");
        Err(zx_status::Status::NOT_SUPPORTED.into())
    }

    fn decrypt_filename(&self, _object_id: u64, _buffer: &mut Vec<u8>) -> Result<(), Error> {
        // NOTE: This isn't a debug assertion because it would trip on the golden image tests.
        warn!("decrypt_filename called on fxfs cipher");
        Err(zx_status::Status::NOT_SUPPORTED.into())
    }

    fn hash_code(&self, _raw_filename: &[u8], _filename: &str) -> Option<u32> {
        debug_assert!(false, "hash_code called on fxfs cipher");
        None
    }

    fn hash_code_casefold(&self, _filename: &str) -> u32 {
        debug_assert!(false, "hash_code_casefold called on fxfs cipher");
        0
    }

    fn supports_inline_encryption(&self) -> bool {
        false
    }

    fn crypt_ctx(&self, _ino: u64, _attribute_id: u64, _file_offset: u64) -> Option<(u64, u8)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{Cipher, FxfsCipher, SECTOR_SIZE};
    use crate::UnwrappedKey;
    use storage_ptr_slice::{MutPtrByteSlice, PtrByteSlice};

    #[test]
    fn test_fxfs_cipher_domain_separates_attribute_id() {
        let key = UnwrappedKey::new(vec![0x42; 32]);
        let cipher = FxfsCipher::new(&key);
        let mut buf0 = vec![0x12; SECTOR_SIZE as usize];
        let mut buf1 = vec![0x12; SECTOR_SIZE as usize];

        cipher.encrypt(1, 0, 0, 0, MutPtrByteSlice::from(&mut buf0[..])).expect("encrypt attr 0");
        cipher.encrypt(1, 4, 0, 0, MutPtrByteSlice::from(&mut buf1[..])).expect("encrypt attr 4");
        assert_ne!(buf0, buf1, "FxfsCipher should domain-separate tweaks across attribute_id");

        // Verify decryption works correctly for each attribute_id
        cipher.decrypt(1, 0, 0, 0, MutPtrByteSlice::from(&mut buf0[..])).expect("decrypt attr 0");
        assert_eq!(buf0, vec![0x12; SECTOR_SIZE as usize]);

        cipher.decrypt(1, 4, 0, 0, MutPtrByteSlice::from(&mut buf1[..])).expect("decrypt attr 4");
        assert_eq!(buf1, vec![0x12; SECTOR_SIZE as usize]);
    }

    #[test]
    fn test_fxfs_cipher_decrypt_to() {
        let key = UnwrappedKey::new(vec![0x42; 32]);
        let cipher = FxfsCipher::new(&key);
        #[derive(Clone, Copy)]
        #[repr(align(64))]
        struct Aligned64<T>(T);
        let plaintext = Aligned64([0x12u8; 4 * SECTOR_SIZE as usize]);
        let mut ciphertext = plaintext;
        cipher.encrypt(1, 0, 0, 0, MutPtrByteSlice::from(&mut ciphertext.0[..])).expect("encrypt");

        let mut decrypted = Aligned64([0u8; 4 * SECTOR_SIZE as usize]);
        cipher
            .decrypt_to(
                1,
                0,
                0,
                0,
                PtrByteSlice::from(&ciphertext.0[..]),
                MutPtrByteSlice::from(&mut decrypted.0[..]),
            )
            .expect("decrypt_to");
        assert_eq!(decrypted.0, plaintext.0);
    }
}
