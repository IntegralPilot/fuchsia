// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use aes_gcm_siv::aead::Aead;
use aes_gcm_siv::{Aes256GcmSiv, KeyInit as _, Nonce};
use async_trait::async_trait;
use fuchsia_sync::Mutex;
use fxfs_crypto::{
    Crypt, EncryptionKey, FscryptKeyIdentifierAndNonce, KeyPurpose, ObjectType, UnwrappedKey,
    WrappedKey, WrappingKeyId,
};
use rand::RngExt as _;
use rand::rngs::StdRng;
use std::collections::hash_map::{Entry, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use zx_status as zx;

fn zero_extended_nonce(val: u64) -> Nonce {
    let mut nonce = Nonce::default();
    nonce.as_mut_slice()[..8].copy_from_slice(&val.to_le_bytes());
    nonce
}

struct Cipher {
    // Used to create or unwrap `EncryptionKey::Fxfs`.
    aes_gcm_siv: Aes256GcmSiv,
    // Used to create or unwrap `EncryptionKey::FscryptInoLblk32Dir`.
    wrapping_key: [u8; 32],
}

impl Cipher {
    fn new(wrapping_key: [u8; 32]) -> Self {
        Self { aes_gcm_siv: Aes256GcmSiv::new_from_slice(&wrapping_key).unwrap(), wrapping_key }
    }

    fn encrypt(&self, nonce: &Nonce, plaintext: &[u8]) -> Result<Vec<u8>, zx::Status> {
        self.aes_gcm_siv.encrypt(nonce, plaintext).map_err(|_e| zx::Status::INTERNAL)
    }

    fn decrypt(&self, nonce: &Nonce, ciphertext: &[u8]) -> Result<Vec<u8>, zx::Status> {
        self.aes_gcm_siv.decrypt(nonce, ciphertext).map_err(|_e| zx::Status::INTERNAL)
    }
}

struct CryptBaseInner {
    ciphers: HashMap<WrappingKeyId, Cipher>,
    active_data_key: Option<WrappingKeyId>,
    active_metadata_key: Option<WrappingKeyId>,
}

/// `CryptBase` is a helper for managing wrapping keys and performing cryptographic operations.
pub struct CryptBase {
    inner: Mutex<CryptBaseInner>,
    shutdown: AtomicBool,
    /// Legacy fscrypt uses the filesystem UUID to salt encryption keys in some variants.
    /// We don't have direct access to the filesystem so we store the UUID here.
    filesystem_uuid: [u8; 16],
}

impl CryptBase {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(CryptBaseInner {
                ciphers: HashMap::new(),
                active_data_key: None,
                active_metadata_key: None,
            }),
            shutdown: AtomicBool::new(false),
            filesystem_uuid: [0; 16],
        }
    }

    pub fn add_wrapping_key(&self, id: WrappingKeyId, key: [u8; 32]) -> Result<(), zx::Status> {
        let mut inner = self.inner.lock();
        match inner.ciphers.entry(id) {
            Entry::Occupied(_) => Err(zx::Status::ALREADY_EXISTS),
            Entry::Vacant(v) => {
                v.insert(Cipher::new(key));
                Ok(())
            }
        }
    }

    pub fn set_active_key(&self, purpose: KeyPurpose, id: WrappingKeyId) -> Result<(), zx::Status> {
        let mut inner = self.inner.lock();
        if !inner.ciphers.contains_key(&id) {
            return Err(zx::Status::NOT_FOUND);
        }
        match purpose {
            KeyPurpose::Data => inner.active_data_key = Some(id),
            KeyPurpose::Metadata => inner.active_metadata_key = Some(id),
        }
        Ok(())
    }

    pub fn forget_wrapping_key(&self, id: &WrappingKeyId) -> Result<(), zx::Status> {
        let mut inner = self.inner.lock();
        if let Some(active_id) = inner.active_data_key {
            if active_id == *id {
                return Err(zx::Status::INVALID_ARGS);
            }
        }
        if let Some(active_id) = inner.active_metadata_key {
            if active_id == *id {
                return Err(zx::Status::INVALID_ARGS);
            }
        }
        inner.ciphers.remove(id);
        Ok(())
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    /// Fscrypt in INO_LBLK32 and INO_LBLK64 modes mix the filesystem_uuid into key derivation
    /// functions. Crypt should be told the uuid ahead of time to support decryption of migrated
    /// data. (Note that we make an assumption that there is only one filesystem.)
    pub fn set_filesystem_uuid(&mut self, uuid: &[u8; 16]) {
        self.filesystem_uuid = *uuid;
    }
}

#[async_trait]
impl Crypt for CryptBase {
    async fn create_key(
        &self,
        owner: u64,
        purpose: KeyPurpose,
    ) -> Result<(fxfs_crypto::FxfsKey, UnwrappedKey), zx::Status> {
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(zx::Status::INTERNAL);
        }
        let inner = self.inner.lock();
        let wrapping_key_id = match purpose {
            KeyPurpose::Data => inner.active_data_key,
            KeyPurpose::Metadata => inner.active_metadata_key,
        }
        .ok_or(zx::Status::INVALID_ARGS)?;

        let cipher = inner.ciphers.get(&wrapping_key_id).ok_or(zx::Status::UNAVAILABLE)?;

        let nonce = zero_extended_nonce(owner);

        let mut uwnrapped_key = [0u8; 32];
        rand::make_rng::<StdRng>().fill(&mut uwnrapped_key);

        let wrapped_key = cipher.encrypt(&nonce, &uwnrapped_key[..])?;
        Ok((
            fxfs_crypto::FxfsKey {
                wrapping_key_id,
                key: wrapped_key.try_into().map_err(|_| zx::Status::INTERNAL)?,
            },
            UnwrappedKey::new(uwnrapped_key.to_vec()),
        ))
    }

    async fn create_key_with_id(
        &self,
        owner: u64,
        wrapping_key_id: WrappingKeyId,
        object_type: ObjectType,
    ) -> Result<(EncryptionKey, UnwrappedKey), zx::Status> {
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(zx::Status::INTERNAL);
        }

        match object_type {
            ObjectType::Directory | ObjectType::Symlink => {
                let mut nonce = [0; 16];
                rand::make_rng::<StdRng>().fill(&mut nonce);
                let inner = self.inner.lock();
                let cipher = inner.ciphers.get(&wrapping_key_id).ok_or(zx::Status::UNAVAILABLE)?;
                let mut unwrapped_key = [0u8; 96];
                fscrypt::hkdf::hkdf(&cipher.wrapping_key, &nonce, &mut unwrapped_key);
                Ok((
                    EncryptionKey::FscryptInoLblk32Dir {
                        key_identifier: wrapping_key_id,
                        nonce: nonce.try_into().map_err(|_| zx::Status::INTERNAL)?,
                    },
                    UnwrappedKey::new(unwrapped_key.to_vec()),
                ))
            }
            _ => {
                let inner = self.inner.lock();
                let cipher = inner.ciphers.get(&wrapping_key_id).ok_or(zx::Status::UNAVAILABLE)?;
                let nonce = zero_extended_nonce(owner);
                let mut unwrapped_key = [0u8; 32];
                rand::make_rng::<StdRng>().fill(&mut unwrapped_key);
                let wrapped = cipher.encrypt(&nonce, &unwrapped_key[..])?;
                Ok((
                    EncryptionKey::Fxfs(fxfs_crypto::FxfsKey {
                        wrapping_key_id,
                        key: wrapped.try_into().map_err(|_| zx::Status::INTERNAL)?,
                    }),
                    UnwrappedKey::new(unwrapped_key.to_vec()),
                ))
            }
        }
    }

    async fn unwrap_key(
        &self,
        wrapped_key: &WrappedKey,
        owner: u64,
    ) -> Result<UnwrappedKey, zx::Status> {
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(zx::Status::INTERNAL);
        }

        match wrapped_key {
            WrappedKey::FscryptInoLblk32Dir(FscryptKeyIdentifierAndNonce {
                key_identifier,
                nonce,
            }) => {
                let inner = self.inner.lock();
                let cipher = inner.ciphers.get(key_identifier).ok_or(zx::Status::UNAVAILABLE)?;
                let mut unwrapped_key = [0u8; 96];
                fscrypt::hkdf::hkdf(&cipher.wrapping_key, nonce, &mut unwrapped_key);
                Ok(UnwrappedKey::new(unwrapped_key.to_vec()))
            }
            WrappedKey::Fxfs(fidl_fuchsia_fxfs::FxfsKey { wrapping_key_id, wrapped_key }) => {
                let inner = self.inner.lock();
                let cipher = inner.ciphers.get(wrapping_key_id).ok_or(zx::Status::UNAVAILABLE)?;
                let mut nonce = Nonce::default();
                nonce.as_mut_slice()[..8].copy_from_slice(&owner.to_le_bytes());
                Ok(UnwrappedKey::new(cipher.decrypt(&nonce, wrapped_key)?))
            }
            _ => Err(zx::Status::NOT_SUPPORTED),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn test_wrap_unwrap() {
        let crypt = CryptBase::new();
        let key = [0xABu8; 32];
        let id = [1u8; 16];
        crypt.add_wrapping_key(id, key).expect("add_wrapping_key failed");
        crypt.set_active_key(KeyPurpose::Data, id).expect("set_active_key failed");

        let (fxfs_key, unwrapped_key) =
            crypt.create_key(0, KeyPurpose::Data).await.expect("create_key failed");
        assert_eq!(fxfs_key.wrapping_key_id, id);
        assert_eq!(unwrapped_key.len(), 32);

        let unwrapped_back = crypt
            .unwrap_key(&WrappedKey::Fxfs(fxfs_key.into()), 0)
            .await
            .expect("unwrap_key failed");
        assert_eq!(*unwrapped_key, *unwrapped_back);
    }

    #[fuchsia::test]
    async fn test_forget_wrapping_key() {
        let crypt = CryptBase::new();
        let key = [0xABu8; 32];
        let id = [1u8; 16];
        crypt.add_wrapping_key(id, key).expect("add_wrapping_key failed");
        assert_eq!(crypt.add_wrapping_key(id, key), Err(zx::Status::ALREADY_EXISTS));
        crypt.forget_wrapping_key(&id).unwrap();
        assert_eq!(
            crypt
                .unwrap_key(
                    &WrappedKey::Fxfs(fidl_fuchsia_fxfs::FxfsKey {
                        wrapping_key_id: id,
                        wrapped_key: [0u8; 48]
                    }),
                    0
                )
                .await
                .expect_err("unwrap_key should fail when wrapping key is forgotten"),
            zx::Status::UNAVAILABLE
        );
        crypt.add_wrapping_key(id, key).expect("add_wrapping_key failed");
    }

    #[fuchsia::test]
    async fn test_active_key_management() {
        let crypt = CryptBase::new();
        let key = [0xABu8; 32];
        let id1 = [0u8; 16];
        let id2 = [1u8; 16];
        crypt.add_wrapping_key(id1, key).expect("add_wrapping_key failed");
        crypt.add_wrapping_key(id2, key).expect("add_wrapping_key failed");

        crypt.set_active_key(KeyPurpose::Data, id1).expect("set_active_key failed");
        crypt.set_active_key(KeyPurpose::Metadata, id2).expect("set_active_key failed");

        assert_eq!(crypt.forget_wrapping_key(&id1), Err(zx::Status::INVALID_ARGS));
        assert_eq!(crypt.forget_wrapping_key(&id2), Err(zx::Status::INVALID_ARGS));
    }

    #[fuchsia::test]
    async fn test_shutdown() {
        let crypt = CryptBase::new();
        let key = [0xABu8; 32];
        let id = [1u8; 16];
        crypt.add_wrapping_key(id, key).expect("add_wrapping_key failed");
        crypt.set_active_key(KeyPurpose::Data, id).expect("set_active_key failed");

        crypt.shutdown();

        assert_eq!(
            crypt
                .create_key(0, KeyPurpose::Data)
                .await
                .expect_err("create_key should fail when crypt has shut down"),
            zx::Status::INTERNAL
        );
        assert_eq!(
            crypt
                .create_key_with_id(0, id, ObjectType::File)
                .await
                .expect_err("create_key_with_id should fail when crypt has shut down"),
            zx::Status::INTERNAL
        );
        assert_eq!(
            crypt
                .unwrap_key(
                    &WrappedKey::Fxfs(fidl_fuchsia_fxfs::FxfsKey {
                        wrapping_key_id: id,
                        wrapped_key: [0u8; 48]
                    }),
                    0,
                )
                .await
                .expect_err("unwrap_key should fail when crypt has shut down"),
            zx::Status::INTERNAL
        );
    }

    #[fuchsia::test]
    async fn test_create_key_no_active_key() {
        let crypt = CryptBase::new();
        assert_eq!(
            crypt
                .create_key(0, KeyPurpose::Data)
                .await
                .expect_err("create_key should fail when no active key is set"),
            zx::Status::INVALID_ARGS
        );
    }

    #[fuchsia::test]
    async fn test_create_key_with_id_not_found() {
        let crypt = CryptBase::new();
        let id = [1u8; 16];
        assert_eq!(
            crypt.create_key_with_id(0, id, ObjectType::File).await.expect_err(
                "create_key_with_id should fail when no active key is set at wrapping key id"
            ),
            zx::Status::UNAVAILABLE
        );
    }

    #[fuchsia::test]
    async fn test_unwrap_key_not_found() {
        let crypt = CryptBase::new();
        let id = [1u8; 16];
        assert_eq!(
            crypt
                .unwrap_key(
                    &WrappedKey::Fxfs(fidl_fuchsia_fxfs::FxfsKey {
                        wrapping_key_id: id,
                        wrapped_key: [0u8; 48]
                    }),
                    0,
                )
                .await
                .expect_err("unwrap_key should fail when no active key is set at wrapping key id"),
            zx::Status::UNAVAILABLE
        );
    }

    #[fuchsia::test]
    async fn test_unwrap_key_wrong_owner() {
        let crypt = CryptBase::new();
        let key = [0xABu8; 32];
        let id = [0u8; 16];
        crypt.add_wrapping_key(id, key).expect("add_wrapping_key failed");
        crypt.set_active_key(KeyPurpose::Data, id).expect("set_active_key failed");

        let (fxfs_key, _unwrapped_key) =
            crypt.create_key(0, KeyPurpose::Data).await.expect("create_key failed");
        // Try to unwrap with wrong owner (1 instead of 0)
        assert_eq!(
            crypt
                .unwrap_key(&WrappedKey::Fxfs(fxfs_key.into()), 1)
                .await
                .expect_err("unwrap_key should fail when owner does not match"),
            zx::Status::INTERNAL
        );
    }

    #[fuchsia::test]
    async fn test_wrap_unwrap_key_with_arbitrary_wrapping_key_id() {
        let crypt = CryptBase::new();
        let key = [0xABu8; 32];
        let id = [2u8; 16];
        crypt.add_wrapping_key(id, key).expect("add_key failed");

        let (wrapped_key, unwrapped_key) = crypt
            .create_key_with_id(0, id, ObjectType::File)
            .await
            .expect("create_key_with_id failed");
        let unwrap_result =
            crypt.unwrap_key(&WrappedKey::from(wrapped_key), 0).await.expect("unwrap_key failed");
        assert_eq!(*unwrap_result, *unwrapped_key);

        // Do it twice to make sure the service can use the same key repeatedly.
        let (wrapped_key, unwrapped_key) = crypt
            .create_key_with_id(1, id, ObjectType::File)
            .await
            .expect("create_key_with_id failed");
        let unwrap_result =
            crypt.unwrap_key(&WrappedKey::from(wrapped_key), 1).await.expect("unwrap_key failed");
        assert_eq!(*unwrap_result, *unwrapped_key);
    }

    #[fuchsia::test]
    async fn test_unwrap_key_wrong_key() {
        let crypt = CryptBase::new();
        let key = [0xABu8; 32];
        let id = [0u8; 16];
        crypt.add_wrapping_key(id, key).expect("add_key failed");
        crypt.set_active_key(KeyPurpose::Data, id).expect("set_active_key failed");

        let (fxfs_key, _unwrapped_key) =
            crypt.create_key(0, KeyPurpose::Data).await.expect("create_key failed");
        let mut modified_wrapped_key = fxfs_key.key.to_vec();
        for byte in &mut modified_wrapped_key {
            *byte ^= 0xff;
        }
        assert_eq!(
            crypt
                .unwrap_key(
                    &WrappedKey::Fxfs(fidl_fuchsia_fxfs::FxfsKey {
                        wrapping_key_id: fxfs_key.wrapping_key_id,
                        wrapped_key: modified_wrapped_key.clone().try_into().unwrap(),
                    }),
                    0,
                )
                .await
                .is_err(),
            true
        );
    }
}
