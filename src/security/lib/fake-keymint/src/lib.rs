// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A fake implementation of fuchsia.security.keymint.SealingKeys and
//! fuchsia.security.keymint.Admin for testing purposes.
//!
//! IMPORTANT: This implementation is insecure!

use aes_gcm_siv::aead::Aead as _;
use aes_gcm_siv::{Aes128GcmSiv, KeyInit as _};
use fidl::endpoints::{ClientEnd, create_request_stream};
use fidl_fuchsia_security_keymint::{
    AdminMarker, AdminRequest, AdminRequestStream, DeleteError, SealError, SealingKeysMarker,
    SealingKeysRequest, SealingKeysRequestStream, UnsealError, UpgradeError,
};
use fuchsia_sync::Mutex;
use futures::{FutureExt as _, TryStreamExt as _};
use log::warn;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::pin;

use futures::future::BoxFuture;
use std::sync::Arc;

type KeyInfo = Vec<u8>;

/// This is the *internal* representation of a sealing key in FakeKeymint.
///
/// While an external representation consists of a key_info and key_blob, internally we
/// are keeping a set of keyblobs to allow for key upgrading.
struct SealingKey {
    cipher: Aes128GcmSiv,
    /// Holds a collection of key blobs (bag of bytes) that are to be considered
    /// valid for this sealing key. These are added when we create a key and
    /// when we upgrade a key. They are removed when we delete a sealing key.
    key_blobs: Vec<Vec<u8>>,
}

type DeleteHook = Arc<dyn Fn(Vec<u8>) -> BoxFuture<'static, Option<DeleteError>> + Send + Sync>;

#[derive(Default)]
struct Inner {
    sealing_keys: BTreeMap<KeyInfo, SealingKey>,
    // Epoch is mixed into keys, and is incremented each time DeleteAllKeys is called.  This ensures
    // that previously deleted keys cannot be reused.
    epoch: u64,

    /// An optional async hook that is called before `DeleteSealingKey` performs the deletion.
    /// If the hook returns `Some(err)`, that error is returned to the client and the deletion is
    /// skipped.
    /// If the hook returns `None`, the deletion proceeds normally.
    /// The hook can also hang indefinitely to simulate a crash.
    delete_hook: Option<DeleteHook>,
}

/// Generate a key blob for a given key_info and epoch.
fn blob_for_key(key_info: &KeyInfo, epoch: u64) -> Vec<u8> {
    format!("bL0b_F0r_t3$t_{}|{epoch}", String::from_utf8_lossy(key_info)).into_bytes().to_vec()
}

/// Extract the epoch from a key_blob.
fn epoch_from_blob(blob: &[u8]) -> Option<u64> {
    let blob_str = String::from_utf8_lossy(blob);
    blob_str.split('|').last().and_then(|s| s.parse().ok())
}

impl Inner {
    const IV: [u8; 12] = [0u8; 12];

    // NB: A real Keymint implementation would return a different sealing key each time this is
    // called, and remember the keys that are created.  Since we don't have anywhere to persist
    // them, we just derive the sealing key from the key info directly, and then derive the key blob
    // from the key info and epoch.  Obviously, this is not secure.
    fn derive_key(key_info: &KeyInfo, epoch: u64) -> SealingKey {
        let mut key_bytes = [0u8; 16];
        let len = key_info.len().min(16);
        key_bytes[..len].copy_from_slice(&key_info[..len]);

        // Cipher should be stable for a given key_info so that we can unseal secrets
        // after an epoch bump (which triggers a blob upgrade but shouldn't break decryption).
        let cipher = Aes128GcmSiv::new_from_slice(&key_bytes).unwrap();
        let key_blobs = vec![blob_for_key(key_info, epoch)];
        SealingKey { cipher, key_blobs }
    }

    fn handle_create_request(&mut self, key_info: KeyInfo) -> Vec<u8> {
        let epoch = self.epoch;
        let sealing_key = self
            .sealing_keys
            .entry(key_info.clone())
            .or_insert_with(|| Self::derive_key(&key_info, epoch));
        if !sealing_key.key_blobs.iter().any(|b| epoch_from_blob(b) == Some(epoch)) {
            sealing_key.key_blobs.push(blob_for_key(&key_info, epoch));
        }
        sealing_key.key_blobs.iter().find(|b| epoch_from_blob(b) == Some(epoch)).unwrap().clone()
    }

    /// Handles a seal request.
    ///
    /// If the key_blob is not found, or the epoch is too old, it will return an error.
    /// Otherwise, it will encrypt the secret with the sealing key and return the sealed secret.
    fn handle_seal_request(
        &mut self,
        key_info: KeyInfo,
        key_blob: Vec<u8>,
        secret: Vec<u8>,
    ) -> Result<Vec<u8>, SealError> {
        let sealing_key = self.sealing_keys.get(&key_info).ok_or_else(|| SealError::FailedSeal)?;
        if !sealing_key.key_blobs.contains(&key_blob) {
            warn!("Wrong key blob");
            return Err(SealError::FailedSeal);
        }
        let epoch = epoch_from_blob(&key_blob).ok_or_else(|| {
            warn!("Invalid key blob");
            SealError::FailedSeal
        })?;
        if epoch < self.epoch {
            warn!("Key requires upgrade");
            return Err(SealError::KeyRequiresUpgrade);
        }
        let sealed_secret = sealing_key
            .cipher
            .encrypt((&Self::IV[..]).try_into().unwrap(), &secret[..])
            .map_err(|e| {
                warn!("Failed to seal secret: {}", e);
                SealError::FailedSeal
            })?;
        Ok(sealed_secret)
    }

    /// Handles an unseal request.
    ///
    /// If the key_blob is not found, or the epoch is too old, it will return an error.
    /// Otherwise, it will decrypt the sealed secret with the sealing key and return the secret.
    fn handle_unseal_request(
        &mut self,
        key_info: KeyInfo,
        key_blob: Vec<u8>,
        sealed_secret: Vec<u8>,
    ) -> Result<Vec<u8>, UnsealError> {
        let sealing_key = self
            .sealing_keys
            .entry(key_info.clone())
            .or_insert_with(|| Self::derive_key(&key_info, self.epoch));
        if !sealing_key.key_blobs.contains(&key_blob) {
            warn!("Unknown keyblob");
            return Err(UnsealError::FailedUnseal);
        }
        let epoch = epoch_from_blob(&key_blob).ok_or_else(|| {
            warn!("Invalid key blob");
            UnsealError::FailedUnseal
        })?;
        if epoch < self.epoch {
            warn!("Key requires upgrade");
            return Err(UnsealError::KeyRequiresUpgrade);
        }
        let secret = sealing_key
            .cipher
            .decrypt((&Self::IV[..]).try_into().unwrap(), &sealed_secret[..])
            .map_err(|e| {
                warn!("Failed to unseal secret: {}", e);
                UnsealError::FailedUnseal
            })?;
        Ok(secret)
    }

    fn handle_delete_all_keys_request(&mut self) {
        self.sealing_keys.clear();
        self.epoch += 1;
    }

    /// Bumps the epoch and returns the new epoch.
    fn bump_epoch(&mut self) {
        self.epoch += 1;
    }

    /// Handles an upgrade request.
    ///
    /// If the key_blob is not found, it will return an error.
    /// Otherwise, it will return a new key blob for the current epoch.
    /// Note that the old key blob will continue to work (but require an upgrade) until deleted.
    fn handle_upgrade_request(
        &mut self,
        key_info: KeyInfo,
        key_blob: Vec<u8>,
    ) -> Result<Vec<u8>, UpgradeError> {
        let epoch = self.epoch;
        let sealing_key = self
            .sealing_keys
            .entry(key_info.clone())
            .or_insert_with(|| Self::derive_key(&key_info, epoch));
        if !sealing_key.key_blobs.contains(&key_blob) {
            warn!("Wrong key blob for upgrade");
            return Err(UpgradeError::FailedUpgrade);
        }
        let upgraded = blob_for_key(&key_info, self.epoch);
        if !sealing_key.key_blobs.contains(&upgraded) {
            sealing_key.key_blobs.push(upgraded.clone());
        }
        Ok(upgraded)
    }

    /// Handles a delete request.
    ///
    /// If the key_blob is not found, it will return an error.
    /// Otherwise, it will remove the key blob from the key blobs.
    /// The sealing key will no longer be valid after this call.
    fn handle_delete_request(&mut self, key_blob: &[u8]) -> Result<(), DeleteError> {
        let mut removed = false;
        self.sealing_keys.retain(|_, sealing_key| {
            let initial_len = sealing_key.key_blobs.len();
            sealing_key.key_blobs.retain(|kb| kb != key_blob);
            if sealing_key.key_blobs.len() < initial_len {
                removed = true;
            }
            !sealing_key.key_blobs.is_empty()
        });

        if removed { Ok(()) } else { Err(DeleteError::FailedDelete) }
    }

    fn has_key_blob(&self, key_blob: &[u8]) -> bool {
        self.sealing_keys.values().any(|sk| sk.key_blobs.iter().any(|b| b == key_blob))
    }
}

#[derive(Default, Clone)]
pub struct FakeKeymint {
    inner: Arc<Mutex<Inner>>,
}

impl FakeKeymint {
    /// Sets an async hook to be called before `DeleteSealingKey` performs the deletion.
    pub fn set_delete_hook<F>(&self, hook: F)
    where
        F: Fn(Vec<u8>) -> BoxFuture<'static, Option<DeleteError>> + Send + Sync + 'static,
    {
        self.inner.lock().delete_hook = Some(Arc::new(hook));
    }

    /// Checks whether the underlying state currently contains the specified key blob.
    pub fn has_key_blob(&self, key_blob: &[u8]) -> bool {
        self.inner.lock().has_key_blob(key_blob)
    }

    /// Used to force a key upgrade of all sealing keys with the current epoch.
    pub fn bump_epoch(&self) {
        self.inner.lock().bump_epoch();
    }

    /// Injects mock key state directly into FakeKeymint simulating an arbitrary list of key blobs.
    /// If a key already exists, the injected blobs are appended to the existing state.
    pub fn insert_sealing_key(&self, key_info: &[u8], blobs: Vec<Vec<u8>>) {
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.sealing_keys.get_mut(key_info) {
            for blob in blobs {
                if !existing.key_blobs.contains(&blob) {
                    existing.key_blobs.push(blob);
                }
            }
        } else {
            inner.sealing_keys.insert(
                key_info.to_vec(),
                SealingKey {
                    cipher: Inner::derive_key(&key_info.to_vec(), 0).cipher,
                    key_blobs: blobs,
                },
            );
        }
    }

    /// Generates a mock `sealing_key_blob` payload synchronously without traversing FIDL channels.
    pub fn generate_static_sealing_key(&self, key_info: &[u8]) -> Vec<u8> {
        self.inner.lock().handle_create_request(key_info.to_vec()).to_vec()
    }

    /// Generates a mock `sealed_keys` payload synchronously without traversing FIDL channels.
    pub fn generate_static_sealed_data(
        &self,
        key_info: &[u8],
        key_blob: &[u8],
        secret: &[u8],
    ) -> Vec<u8> {
        self.inner
            .lock()
            .handle_seal_request(key_info.to_vec(), key_blob.to_vec(), secret.to_vec())
            .expect("Failed static seal request")
            .to_vec()
    }

    /// Handles [`SealingKeysRequestStream`] to completion.
    pub async fn run_sealing_keys_service(
        &self,
        stream: SealingKeysRequestStream,
    ) -> Result<(), fidl::Error> {
        stream
            .try_for_each_concurrent(None, move |request| async move {
                match request {
                    SealingKeysRequest::CreateSealingKey { key_info, responder } => {
                        responder.send(Ok(&*self.inner.lock().handle_create_request(key_info)))?;
                    }
                    SealingKeysRequest::Seal { key_info, key_blob, secret, responder } => {
                        match self.inner.lock().handle_seal_request(key_info, key_blob, secret) {
                            Ok(sealed_secret) => responder.send(Ok(&*sealed_secret))?,
                            Err(err) => responder.send(Err(err))?,
                        }
                    }
                    SealingKeysRequest::Unseal { key_info, key_blob, sealed_secret, responder } => {
                        match self.inner.lock().handle_unseal_request(
                            key_info,
                            key_blob,
                            sealed_secret,
                        ) {
                            Ok(secret) => responder.send(Ok(&*secret))?,
                            Err(err) => responder.send(Err(err))?,
                        }
                    }
                    SealingKeysRequest::UpgradeSealingKey { key_info, key_blob, responder } => {
                        match self.inner.lock().handle_upgrade_request(key_info, key_blob) {
                            Ok(key) => responder.send(Ok(&*key))?,
                            Err(err) => responder.send(Err(err))?,
                        }
                    }
                    SealingKeysRequest::DeleteSealingKey { key_blob, responder } => {
                        let hook = self.inner.lock().delete_hook.clone();
                        let hook_result =
                            if let Some(hook) = hook { hook(key_blob.clone()).await } else { None };

                        if let Some(err) = hook_result {
                            responder.send(Err(err))?;
                        } else {
                            let result = self.inner.lock().handle_delete_request(&key_blob);
                            match result {
                                Ok(()) => responder.send(Ok(()))?,
                                Err(e) => responder.send(Err(e))?,
                            }
                        }
                    }
                }
                Ok(())
            })
            .await
    }

    /// Handles [`AdminRequestStream`] to completion.
    pub async fn run_admin_service(&self, stream: AdminRequestStream) -> Result<(), fidl::Error> {
        stream
            .try_for_each_concurrent(None, move |request| async move {
                match request {
                    AdminRequest::DeleteAllKeys { responder } => {
                        responder.send(Ok(self.inner.lock().handle_delete_all_keys_request()))?;
                    }
                }
                Ok(())
            })
            .await
    }
}

/// Runs `f` with a scoped FakeKeymint instance.  The instance will be automatically terminated on
/// completion.
pub async fn with_keymint_service<R, Fut: Future<Output = anyhow::Result<R>>>(
    f: impl FnOnce(ClientEnd<SealingKeysMarker>, ClientEnd<AdminMarker>) -> Fut,
) -> anyhow::Result<R> {
    let (sealing_keys_client, sealing_keys_stream) = create_request_stream::<SealingKeysMarker>();
    let (admin_client, admin_stream) = create_request_stream::<AdminMarker>();
    let fake_keymint = FakeKeymint::default();
    let mut sealing_keys_service =
        pin!(async { fake_keymint.run_sealing_keys_service(sealing_keys_stream).await }.fuse());
    let mut admin_service =
        pin!(async { fake_keymint.run_admin_service(admin_stream).await }.fuse());
    let mut fut = pin!(f(sealing_keys_client, admin_client).fuse());

    loop {
        futures::select! {
            _ = sealing_keys_service => {}
            _ = admin_service => {}
            result = fut => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn create_seal_unseal() {
        with_keymint_service(|keymint, _| async {
            let keymint = keymint.into_proxy();
            const KEY_INFO: [u8; 16] = [1u8; 16];
            let key_blob = keymint
                .create_sealing_key(&KEY_INFO[..])
                .await
                .expect("FIDL error")
                .expect("create error");
            const SECRET: [u8; 16] = [0xffu8; 16];
            let sealed = keymint
                .seal(&KEY_INFO[..], &key_blob[..], &SECRET[..])
                .await
                .expect("FIDL error")
                .expect("seal error");
            assert_ne!(sealed, SECRET);
            let unsealed = keymint
                .unseal(&KEY_INFO[..], &key_blob[..], &sealed[..])
                .await
                .expect("FIDL error")
                .expect("unseal error");
            assert_eq!(unsealed, SECRET);
            Ok(())
        })
        .await
        .unwrap();
    }

    #[fuchsia::test]
    async fn seal_failure_on_wrong_key_info() {
        with_keymint_service(|keymint, _| async {
            let keymint = keymint.into_proxy();
            const KEY_INFO: [u8; 16] = [1u8; 16];
            let key_blob = keymint
                .create_sealing_key(&KEY_INFO[..])
                .await
                .expect("FIDL error")
                .expect("create error");
            const SECRET: [u8; 16] = [0xffu8; 16];
            keymint
                .seal(&[2u8; 16], &key_blob[..], &SECRET[..])
                .await
                .expect("FIDL error")
                .expect_err("seal should fail");
            Ok(())
        })
        .await
        .unwrap();
    }

    #[fuchsia::test]
    async fn unseal_failure_on_wrong_sealing_key() {
        with_keymint_service(|keymint, _| async {
            let keymint = keymint.into_proxy();
            const KEY_INFO: [u8; 16] = [1u8; 16];
            let key_blob = keymint
                .create_sealing_key(&KEY_INFO[..])
                .await
                .expect("FIDL error")
                .expect("create error");
            const SECRET: [u8; 16] = [0xffu8; 16];
            let sealed = keymint
                .seal(&KEY_INFO[..], &key_blob[..], &SECRET[..])
                .await
                .expect("FIDL error")
                .expect("seal error");
            assert_ne!(sealed, SECRET);
            keymint
                .unseal(&[2u8; 16], &key_blob[..], &sealed[..])
                .await
                .expect("FIDL error")
                .expect_err("unseal should fail");
            Ok(())
        })
        .await
        .unwrap();
    }

    #[fuchsia::test]
    async fn unseal_failure_on_wrong_secret_blob() {
        with_keymint_service(|keymint, _| async {
            let keymint = keymint.into_proxy();
            const KEY_INFO: [u8; 16] = [1u8; 16];
            let key_blob = keymint
                .create_sealing_key(&KEY_INFO[..])
                .await
                .expect("FIDL error")
                .expect("create error");
            const SECRET: [u8; 16] = [0xffu8; 16];
            let _ = keymint
                .seal(&KEY_INFO[..], &key_blob[..], &SECRET[..])
                .await
                .expect("FIDL error")
                .expect("seal error");
            keymint
                .unseal(&KEY_INFO[..], &key_blob[..], &[0u8; 16])
                .await
                .expect("FIDL error")
                .expect_err("unseal should fail");
            Ok(())
        })
        .await
        .unwrap();
    }

    #[fuchsia::test]
    async fn delete_all_keys_renders_key_unusable() {
        with_keymint_service(|keymint, admin| async {
            let keymint = keymint.into_proxy();
            const KEY_INFO: [u8; 16] = [1u8; 16];
            let key_blob = keymint
                .create_sealing_key(&KEY_INFO[..])
                .await
                .expect("FIDL error")
                .expect("create error");

            let admin = admin.into_proxy();
            admin.delete_all_keys().await.expect("FIDL error").expect("create error");

            const SECRET: [u8; 16] = [0xffu8; 16];
            let _ = keymint
                .seal(&KEY_INFO[..], &key_blob[..], &SECRET[..])
                .await
                .expect("FIDL error")
                .expect_err("seal should fail");
            Ok(())
        })
        .await
        .unwrap();
    }

    #[fuchsia::test]
    async fn delete_keys_succeeds() {
        with_keymint_service(|keymint, _admin| async {
            let keymint = keymint.into_proxy();
            const KEY_INFO: [u8; 16] = [1u8; 16];
            let key_blob = keymint
                .create_sealing_key(&KEY_INFO[..])
                .await
                .expect("FIDL error")
                .expect("create error");

            keymint.delete_sealing_key(&key_blob).await.expect("FIDL error").expect("delete error");
            Ok(())
        })
        .await
        .unwrap();
    }

    #[fuchsia::test]
    async fn delete_keys_fails_bad_key_blob() {
        with_keymint_service(|keymint, _admin| async {
            let keymint = keymint.into_proxy();
            const KEY_INFO: [u8; 16] = [1u8; 16];
            let mut key_blob = keymint
                .create_sealing_key(&KEY_INFO[..])
                .await
                .expect("FIDL error")
                .expect("create error");

            key_blob[0] ^= 0xffu8;

            keymint
                .delete_sealing_key(&key_blob)
                .await
                .expect("FIDL error")
                .expect_err("delete success");
            Ok(())
        })
        .await
        .unwrap();
    }

    #[fuchsia::test]
    async fn set_delete_hook_error() {
        let fake_keymint = FakeKeymint::default();
        let (sealing_keys_client, sealing_keys_stream) =
            create_request_stream::<SealingKeysMarker>();
        let fake_keymint_clone = fake_keymint.clone();
        let mut sealing_keys_service = pin!(
            async { fake_keymint_clone.run_sealing_keys_service(sealing_keys_stream).await }.fuse()
        );
        let keymint = sealing_keys_client.into_proxy();

        const KEY_INFO: [u8; 16] = [1u8; 16];
        let key_blob = fake_keymint.generate_static_sealing_key(&KEY_INFO[..]);

        fake_keymint.set_delete_hook(|_| async { Some(DeleteError::FailedDelete) }.boxed());

        let mut fut = pin!(
            async {
                keymint
                    .delete_sealing_key(&key_blob)
                    .await
                    .expect("FIDL error")
                    .expect_err("delete success")
            }
            .fuse()
        );

        futures::select! {
            _ = sealing_keys_service => panic!("service ended"),
            err = fut => {
                assert_eq!(err, DeleteError::FailedDelete);
            }
        }
    }

    #[fuchsia::test]
    async fn set_delete_hook_success() {
        let fake_keymint = FakeKeymint::default();
        let (sealing_keys_client, sealing_keys_stream) =
            create_request_stream::<SealingKeysMarker>();
        let fake_keymint_clone = fake_keymint.clone();
        let mut sealing_keys_service = pin!(
            async { fake_keymint_clone.run_sealing_keys_service(sealing_keys_stream).await }.fuse()
        );
        let keymint = sealing_keys_client.into_proxy();

        const KEY_INFO: [u8; 16] = [1u8; 16];
        let key_blob = fake_keymint.generate_static_sealing_key(&KEY_INFO[..]);

        fake_keymint.set_delete_hook(|_| async { None }.boxed());

        let mut fut = pin!(
            async {
                keymint
                    .delete_sealing_key(&key_blob)
                    .await
                    .expect("FIDL error")
                    .expect("delete error");
            }
            .fuse()
        );

        futures::select! {
            _ = sealing_keys_service => panic!("service ended"),
            _ = fut => {}
        }

        assert!(!fake_keymint.has_key_blob(&key_blob));
    }

    #[fuchsia::test]
    async fn set_delete_hook_hang_before() {
        let fake_keymint = FakeKeymint::default();
        let (sealing_keys_client, sealing_keys_stream) =
            create_request_stream::<SealingKeysMarker>();
        let fake_keymint_clone = fake_keymint.clone();
        let mut sealing_keys_service = pin!(
            async { fake_keymint_clone.run_sealing_keys_service(sealing_keys_stream).await }.fuse()
        );
        let keymint = sealing_keys_client.into_proxy();

        const KEY_INFO: [u8; 16] = [1u8; 16];
        let key_blob = fake_keymint.generate_static_sealing_key(&KEY_INFO[..]);

        let (tx, mut rx) = futures::channel::oneshot::channel();
        let tx = Arc::new(Mutex::new(Some(tx)));
        fake_keymint.set_delete_hook(move |_| {
            let tx_clone = tx.clone();
            async move {
                if let Some(tx) = tx_clone.lock().take() {
                    let _ = tx.send(());
                }
                std::future::pending::<()>().await;
                None
            }
            .boxed()
        });

        let mut fut = pin!(
            async {
                let _ = keymint.delete_sealing_key(&key_blob).await;
            }
            .fuse()
        );

        futures::select! {
            _ = sealing_keys_service => panic!("service ended"),
            _ = fut => panic!("delete should hang"),
            res = rx => {
                res.expect("channel dropped");
            }
        }

        assert!(fake_keymint.has_key_blob(&key_blob));
    }

    #[fuchsia::test]
    async fn bump_epoch_and_upgrade() {
        let fake_keymint = FakeKeymint::default();
        let (sealing_keys_client, sealing_keys_stream) =
            create_request_stream::<SealingKeysMarker>();
        let fake_keymint_clone = fake_keymint.clone();
        let mut sealing_keys_service = pin!(
            async { fake_keymint_clone.run_sealing_keys_service(sealing_keys_stream).await }.fuse()
        );
        let keymint = sealing_keys_client.into_proxy();

        const KEY_INFO: [u8; 16] = [1u8; 16];
        let key_blob_v1 = fake_keymint.generate_static_sealing_key(&KEY_INFO[..]);
        const SECRET: [u8; 16] = [0xffu8; 16];
        let sealed = fake_keymint.generate_static_sealed_data(&KEY_INFO[..], &key_blob_v1, &SECRET);

        fake_keymint.bump_epoch();

        let mut fut = pin!(
            async {
                let err = keymint
                    .unseal(&KEY_INFO[..], &key_blob_v1, &sealed)
                    .await
                    .expect("FIDL error")
                    .expect_err("should require upgrade");
                assert_eq!(err, UnsealError::KeyRequiresUpgrade);

                let key_blob_v2 = keymint
                    .upgrade_sealing_key(&KEY_INFO[..], &key_blob_v1)
                    .await
                    .expect("FIDL error")
                    .expect("upgrade error");
                // Check that epoch bumping is reflected in blob presence natively
                let unsealed = keymint
                    .unseal(&KEY_INFO[..], &key_blob_v2, &sealed)
                    .await
                    .expect("FIDL error")
                    .expect("unseal error");
                assert_eq!(unsealed, SECRET);
                key_blob_v2
            }
            .fuse()
        );

        let key_blob_v2 = futures::select! {
            _ = sealing_keys_service => panic!("service ended"),
            res = fut => res,
        };

        assert!(fake_keymint.has_key_blob(&key_blob_v2));
        assert!(fake_keymint.has_key_blob(&key_blob_v1));
    }

    #[fuchsia::test]
    async fn insert_sealing_key_test() {
        let fake_keymint = FakeKeymint::default();
        const KEY_INFO: [u8; 16] = [2u8; 16];
        let blob1 = vec![1, 2, 3];
        let blob2 = vec![4, 5, 6];
        fake_keymint.insert_sealing_key(&KEY_INFO[..], vec![blob1.clone(), blob2.clone()]);
        assert!(fake_keymint.has_key_blob(&blob1));
        assert!(fake_keymint.has_key_blob(&blob2));
        assert!(!fake_keymint.has_key_blob(&vec![7, 8, 9]));
    }

    #[fuchsia::test]
    async fn insert_sealing_key_append_test() {
        let fake_keymint = FakeKeymint::default();
        const KEY_INFO: [u8; 16] = [2u8; 16];
        let blob1 = vec![1, 2, 3];
        let blob2 = vec![4, 5, 6];
        fake_keymint.insert_sealing_key(&KEY_INFO[..], vec![blob1.clone()]);
        fake_keymint.insert_sealing_key(&KEY_INFO[..], vec![blob2.clone()]);
        assert!(fake_keymint.has_key_blob(&blob1));
        assert!(fake_keymint.has_key_blob(&blob2));
    }

    #[fuchsia::test]
    async fn bump_epoch_seal_requires_upgrade() {
        let fake_keymint = FakeKeymint::default();
        const KEY_INFO: [u8; 16] = [1u8; 16];
        let key_blob_v1 = fake_keymint.generate_static_sealing_key(&KEY_INFO[..]);

        fake_keymint.bump_epoch();

        const SECRET: [u8; 16] = [0xffu8; 16];
        let (sealing_keys_client, sealing_keys_stream) =
            create_request_stream::<SealingKeysMarker>();
        let mut sealing_keys_service =
            pin!(async { fake_keymint.run_sealing_keys_service(sealing_keys_stream).await }.fuse());
        let keymint = sealing_keys_client.into_proxy();

        let mut fut = pin!(
            async {
                let err = keymint
                    .seal(&KEY_INFO[..], &key_blob_v1, &SECRET)
                    .await
                    .expect("FIDL error")
                    .expect_err("should require upgrade");
                assert_eq!(err, SealError::KeyRequiresUpgrade);
            }
            .fuse()
        );

        futures::select! {
            _ = sealing_keys_service => panic!("service ended"),
            _ = fut => {},
        }
    }

    #[fuchsia::test]
    async fn upgrade_wrong_key_blob() {
        let fake_keymint = FakeKeymint::default();
        let (sealing_keys_client, sealing_keys_stream) =
            create_request_stream::<SealingKeysMarker>();
        let fake_keymint_clone = fake_keymint.clone();
        let mut sealing_keys_service = pin!(
            async { fake_keymint_clone.run_sealing_keys_service(sealing_keys_stream).await }.fuse()
        );
        let keymint = sealing_keys_client.into_proxy();

        const KEY_INFO: [u8; 16] = [1u8; 16];
        let _key_blob = fake_keymint.generate_static_sealing_key(&KEY_INFO[..]);

        let mut fut = pin!(
            async {
                let err = keymint
                    .upgrade_sealing_key(&KEY_INFO[..], &[9u8; 16])
                    .await
                    .expect("FIDL error")
                    .expect_err("should fail to upgrade");
                assert_eq!(err, UpgradeError::FailedUpgrade);
            }
            .fuse()
        );

        futures::select! {
            _ = sealing_keys_service => panic!("service ended"),
            _ = fut => {},
        }
    }
}
