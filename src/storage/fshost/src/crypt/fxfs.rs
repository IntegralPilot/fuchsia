// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Manages encryption for Fxfs' data volume.
//!
//! The keys for the data volume are stored in the "unencrypted" volume in a keybag.  The contents
//! of "unencrypted" differ, depending on the crypt policy used:
//!
//! # Legacy policies (null / tee):
//!
//! keys/
//!   fxfs-data  # Wrapped keys for the data volume are serialized to this file; see
//!              # [`KeyBagManager`]
//!
//! # Keymint policy (hardware-sealed keys)
//!
//! keys/
//!   keymint.0 # The sealing key ID and sealed keys for each volume are stored in this file; see
//!             # [`KeyManager`]

use crate::device::constants::{DATA_VOLUME_LABEL, UNENCRYPTED_VOLUME_LABEL};
use anyhow::{Context, Error, anyhow};
use crypt_policy::{
    KeyConsumer, KeySource, KeymintSealedData, delete_sealing_key, format_sources, get_policy,
    unseal_sources,
};
use fidl::endpoints::{ClientEnd, Proxy};
use fidl_fuchsia_component::{self as fcomponent, RealmMarker};
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_fs_startup::{CheckOptions, CreateOptions, MountOptions};
use fidl_fuchsia_fxfs::{CryptManagementMarker, CryptMarker, KeyPurpose};
use fidl_fuchsia_io as fio;
use fs_management::filesystem::{ServingMultiVolumeFilesystem, ServingVolume};
use fuchsia_component::client::{
    connect_to_protocol, connect_to_protocol_at_dir_root, open_childs_exposed_directory,
};
use key_bag::{AES128_KEY_SIZE, AES256_KEY_SIZE, Aes256Key, KeyBagManager, WrappingKey};
use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

struct KeyManager {
    dir: fio::DirectoryProxy,
}

const KEYBAG_DIR_NAME: &'static str = "keys";

// The suffix at the end shall be treated as a version number corresponding with
// `PersistentKeymintSealedData`.
const KEYMINT_PERSISTENCE_FILE: &'static str = "keymint.0";

const KEYBAG_FILE_NAME: &str = "fxfs-data";

impl KeyManager {
    async fn load_keymint_data(&self) -> Result<Option<KeymintSealedData>, Error> {
        let contents =
            match fuchsia_fs::directory::read_file(&self.dir, KEYMINT_PERSISTENCE_FILE).await {
                Err(err) if err.is_not_found_error() => return Ok(None),
                res => res?,
            };

        let mut data: KeymintSealedData =
            serde_json::from_slice(&contents).context("deserializing key data")?;

        if let Some(old) = data.old_blob.take() {
            log::info!("Found old keymint state. Cleaning up old sealing key.");
            // We MUST attempt hardware deletion first before scrubbing the recovery file.
            // If we scrub the file first and crash, we permanently leak the hardware slot.
            if let Err(e) = delete_sealing_key(&old).await {
                log::warn!("Failed to delete old sealing key; continuing anyway: {e:?}");
            }
            // If hardware is cleaned, or if it failed (meaning it could have already been deleted
            // on a previous boot before a crash), scrub the recovery marker from memory so we don't
            // try to delete it again repeatedly on future boots, and persist the change.
            if let Err(e) = self.store_keymint_data(&data).await {
                log::warn!("Failed to rewrite keymint data after deleting old key: {e:?}");
            }
        }

        Ok(Some(data))
    }

    async fn store_keymint_data(&self, data: &KeymintSealedData) -> Result<(), Error> {
        let bytes = serde_json::to_vec(data).context("serializing key data")?;
        fuchsia_fs::directory::atomic_write_file(&self.dir, KEYMINT_PERSISTENCE_FILE, &bytes[..])
            .await?;
        self.dir
            .sync()
            .await
            .context("FIDL sync failed")?
            .map_err(zx::Status::err_from_raw)
            .context("Sync failed")?;
        Ok(())
    }

    async fn commit_keymint_upgrade(&self, keymint: &mut KeymintSealedData) -> Result<(), Error> {
        self.store_keymint_data(keymint).await.context("Failed to store upgraded keymint data")?;

        if let Some(old) = keymint.old_blob.take() {
            if let Err(e) = delete_sealing_key(&old).await {
                log::warn!("Failed to delete old sealing key; continuing anyway: {e:?}");
            }
            if let Err(e) = self.store_keymint_data(keymint).await {
                log::warn!("Failed to rewrite keymint data after deleting old key: {e:?}");
            }
        }
        Ok(())
    }

    async fn handle_key_unseal(
        &self,
        keymint: &mut KeymintSealedData,
        label: &str,
    ) -> Result<Vec<u8>, Error> {
        let mut upgraded = false;
        let key = loop {
            match keymint.unseal_key(label).await? {
                crypt_policy::UnsealResult::Success(key) => {
                    break key;
                }
                crypt_policy::UnsealResult::KeyRequiresUpgrade => {
                    // Hardware requested an upgrade. Perform the upgrade logic.
                    if upgraded {
                        return Err(anyhow!("KeyRequiresUpgrade returned twice"));
                    }
                    keymint
                        .upgrade_sealing_blob()
                        .await
                        .context("Failed to upgrade sealing key")?;
                    upgraded = true;
                }
            }
        };

        if upgraded {
            self.commit_keymint_upgrade(keymint).await?;
        }

        Ok(key)
    }

    async fn unseal_keymint_keys(&mut self) -> Result<Option<(Aes256Key, Aes256Key)>, Error> {
        let Some(mut keymint) =
            self.load_keymint_data().await.context("Failed to load keymint data")?
        else {
            return Ok(None);
        };

        let data = self.handle_key_unseal(&mut keymint, "data.data").await?;
        let metadata = self.handle_key_unseal(&mut keymint, "data.metadata").await?;

        Ok(Some((
            Aes256Key::try_from(data).map_err(|_| anyhow!("Invalid data key"))?,
            Aes256Key::try_from(metadata).map_err(|_| anyhow!("Invalid metadata key"))?,
        )))
    }

    async fn create_keymint_keys(&mut self) -> Result<Option<(Aes256Key, Aes256Key)>, Error> {
        // We always defensively wipe keymint when we're creating new keys (which is either after
        // factory reset, or a newly flashed device).  This is a best-effort mechanism to avoid
        // leaking key slots.
        if let Err(err) = delete_keymint_keys().await {
            log::warn!(err:?; "Failed to delete keymint keys.  Proceeding anyways.");
        }

        let mut keymint = KeymintSealedData::new().await?;
        let data = keymint.create_key("data.data").await?;
        let metadata = keymint.create_key("data.metadata").await?;
        self.store_keymint_data(&keymint).await.context("Failed to store keymint data")?;

        Ok(Some((
            Aes256Key::try_from(data).map_err(|_| anyhow!("Invalid data key"))?,
            Aes256Key::try_from(metadata).map_err(|_| anyhow!("Invalid metadata key"))?,
        )))
    }

    // Returns `None` if the keybag was absent when attempting to unwrap.  (`None` is never returned
    // when `create` is set).
    async fn unwrap_or_create_keys(
        &mut self,
        create: bool,
    ) -> Result<Option<(Aes256Key, Aes256Key)>, Error> {
        let policy = get_policy().await?;
        log::info!("unwrap_or_create_keys create: {create} policy: {policy:?}");
        let sources = if create { format_sources(policy) } else { unseal_sources(policy) };

        let mut keybag = None;
        let mut last_err = anyhow!("no keys?");
        for source in sources {
            let key = match source {
                KeySource::Null(null) => null.get_key(KeyConsumer::Fxfs),
                KeySource::TeeDerived(tee) => tee.get_key().await?,
                KeySource::KeymintSealed => {
                    if create {
                        return self.create_keymint_keys().await;
                    } else {
                        return self.unseal_keymint_keys().await;
                    };
                }
            };
            let wrapping_key = match key.len() {
                // unwrap is safe because we know the length of the requested array is the same
                // length as the Vec in both branches.
                AES128_KEY_SIZE => WrappingKey::Aes128(key.try_into().unwrap()),
                AES256_KEY_SIZE => WrappingKey::Aes256(key.try_into().unwrap()),
                _ => {
                    last_err = anyhow!("invalid key size");
                    continue;
                }
            };
            if keybag.is_none() {
                let keybag_dir =
                    fuchsia_fs::directory::clone(&self.dir).context("Failed to clone dir")?;
                let keybag_dir_fd =
                    fdio::create_fd(keybag_dir.into_channel().unwrap().into_zx_channel().into())?;
                keybag = Some(if create {
                    KeyBagManager::create(keybag_dir_fd, Path::new(KEYBAG_FILE_NAME))?
                } else {
                    let Some(bag) =
                        KeyBagManager::open(keybag_dir_fd, Path::new(KEYBAG_FILE_NAME))?
                    else {
                        return Ok(None);
                    };
                    bag
                });
            }

            let mut unwrap_fn = |slot| {
                if create {
                    keybag.as_mut().unwrap().new_key(slot, &wrapping_key).context("new key")
                } else {
                    keybag
                        .as_mut()
                        .unwrap()
                        .unwrap_key(slot, &wrapping_key)
                        .context("unwrapping key")
                }
            };

            let data_unwrapped = match unwrap_fn(0) {
                Ok(data_unwrapped) => data_unwrapped,
                Err(e) => {
                    last_err = e.context("data key");
                    continue;
                }
            };
            let metadata_unwrapped = match unwrap_fn(1) {
                Ok(metadata_unwrapped) => metadata_unwrapped,
                Err(e) => {
                    last_err = e.context("metadata key");
                    continue;
                }
            };
            return Ok(Some((data_unwrapped, metadata_unwrapped)));
        }
        Err(last_err)
    }
}

/// Unwraps the data volume in `fs`.  Any failures should be treated as fatal and the filesystem
/// should be reformatted and re-initialized.  If Ok(None) is returned, it means the keybag was
/// shredded, so a reformat is required.
/// Returns the name of the data volume as well as a reference to it.
pub async fn unlock_data_volume(
    fs: &mut ServingMultiVolumeFilesystem,
    config: &fshost_config::Config,
) -> Result<Option<(CryptService, String, ServingVolume)>, Error> {
    if config.check_filesystems {
        fs.check_volume(UNENCRYPTED_VOLUME_LABEL, CheckOptions::default())
            .await
            .context("Failed to verify unencrypted")?;
    }
    with_unencrypted_volume(
        fs.open_volume(UNENCRYPTED_VOLUME_LABEL, MountOptions::default())
            .await
            .context("Failed to open unencrypted")?,
        async |unencrypted_volume: &mut ServingVolume| {
            let dir = match fuchsia_fs::directory::open_directory(
                unencrypted_volume.root(),
                KEYBAG_DIR_NAME,
                fio::PERM_READABLE | fio::PERM_WRITABLE,
            )
            .await
            {
                Err(err) if err.is_not_found_error() => return Ok(None),
                res => res?,
            };
            let mut key_manager = KeyManager { dir };
            let Some((data_unwrapped, metadata_unwrapped)) =
                key_manager.unwrap_or_create_keys(false).await?
            else {
                return Ok(None);
            };

            let crypt_service =
                CryptService::new(data_unwrapped, metadata_unwrapped, &config.fxfs_crypt_url)
                    .await
                    .context("init_crypt_service")?;
            if config.check_filesystems {
                fs.check_volume(
                    DATA_VOLUME_LABEL,
                    CheckOptions { crypt: Some(crypt_service.connect()), ..Default::default() },
                )
                .await
                .context("Failed to verify data")?;
            }
            let crypt = Some(crypt_service.connect());

            let volume = fs
                .open_volume(DATA_VOLUME_LABEL, MountOptions { crypt, ..MountOptions::default() })
                .await
                .context("Failed to open data")?;

            Ok(Some((crypt_service, DATA_VOLUME_LABEL.to_string(), volume)))
        },
    )
    .await
}

// We must make sure that the unencrypted volume is properly unmounted for all error paths so that
// it can safely be removed if necessary.
async fn with_unencrypted_volume<R>(
    mut unencrypted_volume: ServingVolume,
    callback: impl AsyncFnOnce(&mut ServingVolume) -> Result<R, Error>,
) -> Result<R, Error> {
    let result = callback(&mut unencrypted_volume).await;
    let _ = unencrypted_volume.shutdown().await;
    result
}

/// Initializes the data volume in `fs`, which should be freshly reformatted.
/// Returns the name of the data volume as well as a reference to it.
pub async fn init_data_volume<'a>(
    fs: &'a mut ServingMultiVolumeFilesystem,
    config: &'a fshost_config::Config,
) -> Result<(CryptService, String, ServingVolume), Error> {
    // Open up the unencrypted volume so that we can access the key-bag for data.
    with_unencrypted_volume(
        fs.create_volume(
            UNENCRYPTED_VOLUME_LABEL,
            CreateOptions::default(),
            MountOptions::default(),
        )
        .await
        .context("Failed to create unencrypted")?,
        async |unencrypted_volume| {
            let dir = fuchsia_fs::directory::create_directory(
                unencrypted_volume.root(),
                KEYBAG_DIR_NAME,
                fio::PERM_READABLE | fio::PERM_WRITABLE,
            )
            .await
            .context("Failed to create keys dir")?;
            let mut key_manager = KeyManager { dir };
            let (data_unwrapped, metadata_unwrapped) =
                key_manager.unwrap_or_create_keys(true).await?.unwrap();

            let crypt_service =
                CryptService::new(data_unwrapped, metadata_unwrapped, &config.fxfs_crypt_url)
                    .await
                    .context("init_crypt_service")?;
            let crypt = Some(crypt_service.connect());

            let volume = fs
                .create_volume(
                    DATA_VOLUME_LABEL,
                    CreateOptions::default(),
                    MountOptions { crypt, ..MountOptions::default() },
                )
                .await
                .context("Failed to create data")?;

            Ok((crypt_service, DATA_VOLUME_LABEL.to_string(), volume))
        },
    )
    .await
}

static FXFS_CRYPT_COLLECTION_NAME: &str = "fxfs-crypt";

pub struct CryptService {
    component_name: String,
    exposed_dir: fio::DirectoryProxy,
}

impl CryptService {
    async fn new(
        data_key: Aes256Key,
        metadata_key: Aes256Key,
        fxfs_crypt_url: &str,
    ) -> Result<Self, Error> {
        static INSTANCE: AtomicU64 = AtomicU64::new(1);

        let collection_ref = fdecl::CollectionRef { name: FXFS_CRYPT_COLLECTION_NAME.to_string() };

        let component_name = format!("fxfs-crypt.{}", INSTANCE.fetch_add(1, Ordering::SeqCst));

        let child_decl = fdecl::Child {
            name: Some(component_name.clone()),
            url: Some(fxfs_crypt_url.to_string()),
            startup: Some(fdecl::StartupMode::Lazy),
            ..Default::default()
        };

        let realm_proxy = connect_to_protocol::<RealmMarker>()?;

        realm_proxy
            .create_child(&collection_ref, &child_decl, fcomponent::CreateChildArgs::default())
            .await?
            .map_err(|e| anyhow!("create_child failed: {:?}", e))?;

        let exposed_dir = open_childs_exposed_directory(
            component_name.clone(),
            Some(FXFS_CRYPT_COLLECTION_NAME.to_string()),
        )
        .await?;

        let crypt_management =
            connect_to_protocol_at_dir_root::<CryptManagementMarker>(&exposed_dir)?;
        let wrapping_key_id_0 = [0; 16];
        let mut wrapping_key_id_1 = [0; 16];
        wrapping_key_id_1[0] = 1;
        crypt_management
            .add_wrapping_key(&wrapping_key_id_0, data_key.deref())
            .await?
            .map_err(zx::Status::err_from_raw)?;
        crypt_management
            .add_wrapping_key(&wrapping_key_id_1, metadata_key.deref())
            .await?
            .map_err(zx::Status::err_from_raw)?;
        crypt_management
            .set_active_key(KeyPurpose::Data, &wrapping_key_id_0)
            .await?
            .map_err(zx::Status::err_from_raw)?;
        crypt_management
            .set_active_key(KeyPurpose::Metadata, &wrapping_key_id_1)
            .await?
            .map_err(zx::Status::err_from_raw)?;

        Ok(CryptService { component_name, exposed_dir })
    }

    fn connect(&self) -> ClientEnd<CryptMarker> {
        // The assumption is if the crypt service child exists at all, the exposed directory will
        // have the crypt protocol, so we `expect` it.
        connect_to_protocol_at_dir_root::<CryptMarker>(&self.exposed_dir)
            .expect("Unable to connect to Crypt service")
            .into_channel()
            .unwrap()
            .into_zx_channel()
            .into()
    }
}

impl Drop for CryptService {
    fn drop(&mut self) {
        if let Ok(realm_proxy) = connect_to_protocol::<RealmMarker>() {
            let _ = realm_proxy.destroy_child(&fdecl::ChildRef {
                name: self.component_name.clone(),
                collection: Some(FXFS_CRYPT_COLLECTION_NAME.to_string()),
            });
        }
    }
}

async fn delete_keymint_keys() -> Result<(), Error> {
    let policy = crypt_policy::get_policy().await;
    if matches!(policy, Ok(crypt_policy::Policy::Keymint)) {
        log::info!("Deleting all keymint keys!");
        crypt_policy::delete_all_keymint_keys().await
    } else {
        Ok(())
    }
}

/// Attempts to shred the key bag stored in the unencrypted volume of `fs`, if it exists.
/// If we fail to find the key bag, we log a warning and return success, since the keys may have
/// already been shredded.
pub async fn shred_key_bag(fs: &ServingMultiVolumeFilesystem) -> Result<(), Error> {
    if let Err(err) = delete_keymint_keys().await {
        log::warn!(err:?; "Failed to delete keymint keys.  Proceeding with best-effort FDR.");
    }

    if !fs.has_volume(UNENCRYPTED_VOLUME_LABEL).await.context("checking for unencrypted volume")? {
        // If the unencrypted volume is missing, the keys are already gone.
        log::warn!("Unencrypted volume not present");
        return Ok(());
    }

    with_unencrypted_volume(
        fs.open_volume(UNENCRYPTED_VOLUME_LABEL, MountOptions::default())
            .await
            .context("Failed to open unencrypted volume")?,
        async |vol| {
            if !fuchsia_fs::directory::dir_contains(vol.root(), KEYBAG_DIR_NAME).await? {
                log::info!("Directory {KEYBAG_DIR_NAME} not present; not shredding");
                return Ok(());
            }
            fuchsia_fs::directory::remove_dir_recursive(vol.root(), KEYBAG_DIR_NAME)
                .await
                .context("Faild to shred keys")?;
            Ok(())
        },
    )
    .await
}
