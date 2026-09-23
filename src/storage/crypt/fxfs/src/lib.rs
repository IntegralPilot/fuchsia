// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_fuchsia_fxfs::{
    CryptCreateKeyResult, CryptCreateKeyWithIdResult, CryptManagementAddWrappingKeyResult,
    CryptManagementForgetWrappingKeyResult, CryptManagementRequest, CryptManagementRequestStream,
    CryptManagementSetActiveKeyResult, CryptRequest, CryptRequestStream, CryptUnwrapKeyResult,
    KeyPurpose, ObjectType as FxfsFidlObjectType, WrappedKey,
};

use futures::stream::TryStreamExt;

pub mod log;
use log::*;

use fxfs_crypt_common::CryptBase;
use fxfs_crypto::Crypt as _;

pub enum Services {
    Crypt(CryptRequestStream),
    CryptManagement(CryptManagementRequestStream),
}

pub struct CryptService {
    inner: CryptBase,
}

impl CryptService {
    pub fn new() -> Self {
        Self { inner: CryptBase::new() }
    }

    async fn create_key(&self, owner: u64, purpose: KeyPurpose) -> CryptCreateKeyResult {
        let purpose = purpose.try_into().map_err(zx::Status::into_raw)?;
        let (fxfs_key, unwrapped_key) =
            self.inner.create_key(owner, purpose).await.map_err(|e| e.into_raw())?;
        Ok((fxfs_key.wrapping_key_id, (*fxfs_key.key).to_vec(), (*unwrapped_key).to_vec()))
    }

    async fn create_key_with_id(
        &self,
        owner: u64,
        wrapping_key_id: u128,
        object_type: FxfsFidlObjectType,
    ) -> CryptCreateKeyWithIdResult {
        let (encryption_key, unwrapped_key) = self
            .inner
            .create_key_with_id(owner, wrapping_key_id.to_le_bytes(), object_type)
            .await
            .map_err(|e| e.into_raw())?;

        Ok((WrappedKey::from(encryption_key), (*unwrapped_key).to_vec()))
    }

    async fn unwrap_key(&self, owner: u64, wrapped_key: WrappedKey) -> CryptUnwrapKeyResult {
        let unwrapped_key =
            self.inner.unwrap_key(&wrapped_key, owner).await.map_err(|e| e.into_raw())?;
        Ok((*unwrapped_key).to_vec())
    }

    pub fn add_wrapping_key(
        &self,
        wrapping_key_id: u128,
        key: Vec<u8>,
    ) -> CryptManagementAddWrappingKeyResult {
        let key: [u8; 32] = key.try_into().map_err(|_| zx::Status::INVALID_ARGS.into_raw())?;
        self.inner.add_wrapping_key(wrapping_key_id.to_le_bytes(), key).map_err(|e| e.into_raw())
    }

    pub fn set_active_key(
        &self,
        purpose: KeyPurpose,
        wrapping_key_id: u128,
    ) -> CryptManagementSetActiveKeyResult {
        let purpose = purpose.try_into().map_err(zx::Status::into_raw)?;
        self.inner.set_active_key(purpose, wrapping_key_id.to_le_bytes()).map_err(|e| e.into_raw())
    }

    fn forget_wrapping_key(&self, wrapping_key_id: u128) -> CryptManagementForgetWrappingKeyResult {
        self.inner.forget_wrapping_key(&wrapping_key_id.to_le_bytes()).map_err(|e| e.into_raw())
    }

    pub async fn handle_request(&self, stream: Services) -> Result<(), Error> {
        match stream {
            Services::Crypt(mut stream) => {
                while let Some(request) = stream.try_next().await.context("Reading request")? {
                    match request {
                        CryptRequest::CreateKey { owner, purpose, responder } => {
                            responder
                                .send(match &self.create_key(owner, purpose).await {
                                    Ok((id, wrapped, key)) => Ok((id, wrapped, key)),
                                    Err(e) => Err(*e),
                                })
                                .unwrap_or_else(|e| {
                                    // TODO(https://fxbug.dev/360919323): we can use `:err` when we
                                    // enable the log kv_std feature.
                                    error!(
                                        error:? = e;
                                        "Failed to send CreateKey response"
                                    )
                                });
                        }
                        CryptRequest::CreateKeyWithId {
                            owner,
                            wrapping_key_id,
                            object_type,
                            responder,
                            ..
                        } => {
                            responder
                                .send(
                                    match self
                                        .create_key_with_id(
                                            owner,
                                            u128::from_le_bytes(wrapping_key_id),
                                            object_type,
                                        )
                                        .await
                                    {
                                        Ok((ref wrapped, ref key)) => Ok((wrapped, key)),
                                        Err(e) => Err(e),
                                    },
                                )
                                .unwrap_or_else(|e| {
                                    // TODO(https://fxbug.dev/360919323): we can use `:err` when we
                                    // enable the log kv_std feature.
                                    error!(
                                        error:? = e;
                                        "Failed to send CreateKeyWithId response"
                                    )
                                });
                        }
                        CryptRequest::UnwrapKey { owner, wrapped_key, responder } => {
                            let response;
                            responder
                                .send({
                                    response = self.unwrap_key(owner, wrapped_key).await;
                                    match &response {
                                        Ok(v) => Ok(&v[..]),
                                        Err(e) => Err(*e),
                                    }
                                })
                                .unwrap_or_else(|e| {
                                    // TODO(https://fxbug.dev/360919323): we can use `:err` when we
                                    // enable the log kv_std feature.
                                    error!(
                                        error:? = e;
                                        "Failed to send UnwrapKey response"
                                    )
                                });
                        }
                    }
                }
            }
            Services::CryptManagement(mut stream) => {
                while let Some(request) = stream.try_next().await.context("Reading request")? {
                    match request {
                        CryptManagementRequest::AddWrappingKey {
                            wrapping_key_id,
                            key,
                            responder,
                        } => {
                            let response =
                                self.add_wrapping_key(u128::from_le_bytes(wrapping_key_id), key);
                            responder.send(response).unwrap_or_else(|e| {
                                // TODO(https://fxbug.dev/360919323): we can use `:err` when we
                                // enable the log kv_std feature.
                                error!(
                                    error:? = e;
                                    "Failed to send AddWrappingKey response"
                                )
                            });
                        }
                        CryptManagementRequest::SetActiveKey {
                            purpose,
                            wrapping_key_id,
                            responder,
                        } => {
                            let response =
                                self.set_active_key(purpose, u128::from_le_bytes(wrapping_key_id));
                            responder.send(response).unwrap_or_else(
                                // TODO(https://fxbug.dev/360919323): we can use `:err` when we
                                // enable the log kv_std feature.
                                |e| error!(error:? = e;"Failed to send SetActiveKey response"),
                            );
                        }
                        CryptManagementRequest::ForgetWrappingKey {
                            wrapping_key_id,
                            responder,
                        } => {
                            let response =
                                self.forget_wrapping_key(u128::from_le_bytes(wrapping_key_id));
                            responder.send(response).unwrap_or_else(|e| {
                                // TODO(https://fxbug.dev/360919323): we can use `:err` when we
                                // enable the log kv_std feature.
                                error!(
                                    error:? = e;
                                    "Failed to send ForgetWrappingKey response"
                                )
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::CryptService;
    use fidl_fuchsia_fxfs::{FxfsKey, KeyPurpose, ObjectType, WrappedKey};

    #[fuchsia::test]
    async fn wrap_unwrap_key() {
        let service = CryptService::new();
        let key = vec![0xABu8; 32];
        service.add_wrapping_key(1, key.clone()).expect("add_key failed");
        service.set_active_key(KeyPurpose::Data, 1).expect("set_active_key failed");

        let (wrapping_key_id, wrapped_key, unwrapped_key) =
            service.create_key(0, KeyPurpose::Data).await.expect("create_key failed");
        let wrapping_key_id_int = u128::from_le_bytes(wrapping_key_id);
        assert_eq!(wrapping_key_id_int, 1);
        let unwrap_result = service
            .unwrap_key(
                0,
                WrappedKey::Fxfs(FxfsKey {
                    wrapping_key_id,
                    wrapped_key: wrapped_key.try_into().unwrap(),
                }),
            )
            .await
            .expect("unwrap_key failed");
        assert_eq!(unwrap_result, unwrapped_key);

        // Do it twice to make sure the service can use the same key repeatedly.
        let (wrapping_key_id, wrapped_key, unwrapped_key) =
            service.create_key(1, KeyPurpose::Data).await.expect("create_key failed");
        let wrapping_key_id_int = u128::from_le_bytes(wrapping_key_id);
        assert_eq!(wrapping_key_id_int, 1);
        let unwrap_result = service
            .unwrap_key(
                1,
                WrappedKey::Fxfs(FxfsKey {
                    wrapping_key_id,
                    wrapped_key: wrapped_key.try_into().unwrap(),
                }),
            )
            .await
            .expect("unwrap_key failed");
        assert_eq!(unwrap_result, unwrapped_key);
    }

    #[fuchsia::test]
    async fn wrap_unwrap_key_with_arbitrary_wrapping_key() {
        let service = CryptService::new();
        let key = vec![0xABu8; 32];
        service.add_wrapping_key(2, key.clone()).expect("add_key failed");

        let (wrapped_key, unwrapped_key) = service
            .create_key_with_id(0, 2, ObjectType::File)
            .await
            .expect("create_key_with_id failed");
        let unwrap_result = service.unwrap_key(0, wrapped_key).await.expect("unwrap_key failed");
        assert_eq!(unwrap_result, unwrapped_key);

        // Do it twice to make sure the service can use the same key repeatedly.
        let (wrapped_key, unwrapped_key) = service
            .create_key_with_id(1, 2, ObjectType::File)
            .await
            .expect("create_key_with_id failed");
        let unwrap_result = service.unwrap_key(1, wrapped_key).await.expect("unwrap_key failed");
        assert_eq!(unwrap_result, unwrapped_key);
    }

    #[fuchsia::test]
    async fn create_key_with_wrapping_key_that_does_not_exist() {
        let service = CryptService::new();
        service
            .create_key_with_id(0, 2, ObjectType::File)
            .await
            .expect_err("create_key_with_id should fail if the wrapping key does not exist");

        let wrapping_key = vec![0xABu8; 32];
        service.add_wrapping_key(2, wrapping_key.clone()).expect("add_key failed");

        let (wrapped_key, unwrapped_key) = service
            .create_key_with_id(0, 2, ObjectType::File)
            .await
            .expect("create_key_with_id failed");
        let unwrap_result = service.unwrap_key(0, wrapped_key).await.expect("unwrap_key failed");
        assert_eq!(unwrap_result, unwrapped_key);
    }

    #[fuchsia::test]
    async fn unwrap_key_wrong_key() {
        let service = CryptService::new();
        let key = vec![0xABu8; 32];
        service.add_wrapping_key(0, key.clone()).expect("add_key failed");
        service.set_active_key(KeyPurpose::Data, 0).expect("set_active_key failed");

        let (wrapping_key_id, mut wrapped_key, _) =
            service.create_key(0, KeyPurpose::Data).await.expect("create_key failed");
        for byte in &mut wrapped_key {
            *byte ^= 0xff;
        }
        service
            .unwrap_key(
                0,
                WrappedKey::Fxfs(FxfsKey {
                    wrapping_key_id,
                    wrapped_key: wrapped_key.try_into().unwrap(),
                }),
            )
            .await
            .expect_err("unwrap_key should fail");
    }

    #[fuchsia::test]
    async fn unwrap_key_wrong_owner() {
        let service = CryptService::new();
        let key = vec![0xABu8; 32];
        service.add_wrapping_key(0, key.clone()).expect("add_key failed");
        service.set_active_key(KeyPurpose::Data, 0).expect("set_active_key failed");

        let (wrapping_key_id, wrapped_key, _) =
            service.create_key(0, KeyPurpose::Data).await.expect("create_key failed");
        service
            .unwrap_key(
                1,
                WrappedKey::Fxfs(FxfsKey {
                    wrapping_key_id,
                    wrapped_key: wrapped_key.try_into().unwrap(),
                }),
            )
            .await
            .expect_err("unwrap_key should fail");
    }

    #[test]
    fn add_forget_key() {
        let service = CryptService::new();
        let key = vec![0xABu8; 32];
        service.add_wrapping_key(0, key.clone()).expect("add_key failed");
        service.add_wrapping_key(0, key.clone()).expect_err("add_key should fail on a used slot");
        service.add_wrapping_key(1, key.clone()).expect("add_key failed");

        service.forget_wrapping_key(0).expect("forget_key failed");

        service.add_wrapping_key(0, key.clone()).expect("add_key failed");
    }

    #[test]
    fn set_active_key() {
        let service = CryptService::new();
        let key = vec![0xABu8; 32];

        service
            .set_active_key(KeyPurpose::Data, 0)
            .expect_err("set_active_key should fail when targeting nonexistent keys");

        service.add_wrapping_key(0, key.clone()).expect("add_key failed");
        service.add_wrapping_key(1, key.clone()).expect("add_key failed");

        service.set_active_key(KeyPurpose::Data, 0).expect("set_active_key failed");
        service.set_active_key(KeyPurpose::Metadata, 1).expect("set_active_key failed");

        service.forget_wrapping_key(0).expect_err("forget_key should fail on an active key");
        service.forget_wrapping_key(1).expect_err("forget_key should fail on an active key");
    }
}
