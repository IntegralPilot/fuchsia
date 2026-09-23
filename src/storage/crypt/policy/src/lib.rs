// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! crypt_policy contains all the key policy logic for the different operations that can be done
//! with hardware keys.  Keeping the policy logic in one place makes it easier to audit.

use std::collections::BTreeMap;

use anyhow::{Context, Error, anyhow, bail};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Policy {
    Null,
    TeeRequired,
    TeeTransitional,
    TeeOpportunistic,
    Keymint,
}

impl TryFrom<String> for Policy {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_ref() {
            "null" => Ok(Policy::Null),
            "tee" => Ok(Policy::TeeRequired),
            "tee-transitional" => Ok(Policy::TeeTransitional),
            "tee-opportunistic" => Ok(Policy::TeeOpportunistic),
            "keymint" => Ok(Policy::Keymint),
            p => bail!("unrecognized key source policy: '{p}'"),
        }
    }
}

impl std::fmt::Display for Policy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Null => f.write_str("null"),
            Self::TeeRequired => f.write_str("tee"),
            Self::TeeTransitional => f.write_str("tee-transitional"),
            Self::TeeOpportunistic => f.write_str("tee-opportunistic"),
            Self::Keymint => f.write_str("keymint"),
        }
    }
}

/// Reads the policy from well-known locations in `/boot`.
pub async fn get_policy() -> Result<Policy, Error> {
    fuchsia_fs::file::read_in_namespace_to_string("/boot/config/zxcrypt").await?.try_into()
}

/// Fxfs and zxcrypt have different null keys, so operations have to indicate which is ultimately
/// going to consume the key we produce.
#[derive(Clone, Copy, Debug)]
pub enum KeyConsumer {
    /// The null key for fxfs is a 128-bit key with the bytes "zxcrypt" at the beginning and then
    /// padded with zeros. This is for legacy reasons - earlier versions of this code picked this
    /// key, so we need to continue to use it to avoid wiping everyone's null-key-encrypted fxfs
    /// data partitions.
    Fxfs,
    /// The null key for zxcrypt is a 256-bit key containing all zeros.
    Zxcrypt,
}

#[derive(Debug)]
pub struct NullKeySource;

impl NullKeySource {
    pub fn get_key(&self, consumer: KeyConsumer) -> Vec<u8> {
        match consumer {
            KeyConsumer::Fxfs => {
                let mut key = b"zxcrypt".to_vec();
                key.resize(16, 0);
                key
            }
            KeyConsumer::Zxcrypt => vec![0u8; 32],
        }
    }
}

#[derive(Debug)]
pub struct TeeDerivedKeySource;

impl TeeDerivedKeySource {
    pub async fn get_key(&self) -> Result<Vec<u8>, Error> {
        // Regardless of the consumer of this key, the key we retrieve with kms is always
        // named "zxcrypt". This is so that old recovery images that might not be aware of
        // fxfs can still wipe the data keys during a factory reset.
        kms_stateless::get_hardware_derived_key(kms_stateless::KeyInfo::new_zxcrypt())
            .await
            .context("failed to get hardware key")
    }
}

/// Bundles together a handle to a Keymint sealing key together with a list of keys sealed by the
/// sealing key.  The contents of this struct can be persistently stored, as it contains no
/// plaintext secrets.
///
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct KeymintSealedData {
    pub sealing_key_info: Vec<u8>,
    pub sealing_key_blob: Vec<u8>,
    pub sealed_keys: BTreeMap<String, Vec<u8>>,
    #[serde(default)]
    pub old_blob: Option<Vec<u8>>,
}

impl KeymintSealedData {
    /// Generates a new hardware-backed sealing key based off of `sealing_key_info` and creates a
    /// new instance of [`KeymintSealedData`] which uses this sealing key.
    ///
    /// Note that repeated calls to this will yield different sealing keys.  The sealing key should
    /// be persisted if it needs to be reused.
    pub async fn new() -> Result<Self, Error> {
        let mut sealing_key_info = vec![0u8; 32];
        zx::cprng_draw(&mut sealing_key_info[..]);
        let sealing_key_blob = kms_stateless::create_sealing_key(&sealing_key_info[..])
            .await
            .context("Failed to create sealing key")?;
        Ok(Self {
            sealing_key_info,
            sealing_key_blob,
            sealed_keys: BTreeMap::default(),
            old_blob: None,
        })
    }

    /// Generates and seals a new key named `label`.  Updates this struct to contain the sealed key
    /// (to be retrieved later by [`Self::unseal_key`]), and returns the unsealed key.
    pub async fn create_key(&mut self, label: &str) -> Result<Vec<u8>, Error> {
        let mut key = vec![0u8; 32];
        zx::cprng_draw(&mut key[..]);
        let sealed_data =
            kms_stateless::seal(&self.sealing_key_info[..], &self.sealing_key_blob[..], &key[..])
                .await
                .context("Failed to seal keymint key")?;
        self.sealed_keys.insert(label.to_string(), sealed_data);
        Ok(key)
    }

    /// Unseals a key previously created via [`Self::create_key`].
    /// Returns either the unsealed key, or a `UnsealResult::KeyRequiresUpgrade` if the
    /// hardware determines the sealing protocol format is too old.
    pub async fn unseal_key(&self, label: &str) -> Result<UnsealResult, Error> {
        let sealed = self.sealed_keys.get(label).ok_or_else(|| anyhow!("Key not found"))?;
        match kms_stateless::unseal(
            &self.sealing_key_info[..],
            &self.sealing_key_blob[..],
            &sealed[..],
        )
        .await
        {
            Ok(key) => Ok(UnsealResult::Success(key)),
            Err(e) => {
                if let kms_stateless::SealingKeysError::Unseal(
                    fidl_fuchsia_security_keymint::UnsealError::KeyRequiresUpgrade,
                ) = e
                {
                    return Ok(UnsealResult::KeyRequiresUpgrade);
                }
                Err(e.into())
            }
        }
    }

    /// Attempts to upgrade the hardware sealing blob for the current configuration.
    /// In the event of a successful upgrade, the previous active blob is moved to `old_blob`
    /// for cleanup.
    pub async fn upgrade_sealing_blob(&mut self) -> Result<(), Error> {
        let new_blob = kms_stateless::upgrade_sealing_key(
            &self.sealing_key_info[..],
            &self.sealing_key_blob[..],
        )
        .await?;

        let old_blob = std::mem::replace(&mut self.sealing_key_blob, new_blob);
        if self.old_blob.is_some() {
            tracing::warn!(
                "Overwriting an existing old_blob during upgrade. A Keymint key has been leaked."
            );
        }
        self.old_blob = Some(old_blob);

        Ok(())
    }
}

/// A strictly typed result indicating if the hardware unseal succeeded or required an upgrade.
pub enum UnsealResult {
    Success(Vec<u8>),
    KeyRequiresUpgrade,
}

/// Deletes all keymint-managed keys.
pub async fn delete_all_keymint_keys() -> Result<(), Error> {
    Ok(kms_stateless::delete_all_keys().await?)
}

/// Deletes a specific keymint-managed key.
pub async fn delete_sealing_key(key_blob: &[u8]) -> Result<(), Error> {
    Ok(kms_stateless::delete_sealing_key(key_blob).await?)
}

#[derive(Debug)]
pub enum KeySource {
    /// An insecure static key is used.
    Null(NullKeySource),
    /// A hardware-derived key is used, which is accessed by interacting with the TEE via
    /// kms_stateless.
    TeeDerived(TeeDerivedKeySource),
    /// A hardware-sealed key is used.  The key is stored in sealed format, and Keymint is used to
    /// unseal the key at runtime.  The keymint sealing key is never available in plaintext to the
    /// system.
    KeymintSealed,
}

/// Returns all valid key sources when formatting a volume, based on `policy`.
pub fn format_sources(policy: Policy) -> Vec<KeySource> {
    match policy {
        Policy::Null => vec![KeySource::Null(NullKeySource)],
        Policy::TeeRequired => vec![KeySource::TeeDerived(TeeDerivedKeySource)],
        Policy::TeeTransitional => vec![KeySource::TeeDerived(TeeDerivedKeySource)],
        Policy::TeeOpportunistic => {
            vec![KeySource::TeeDerived(TeeDerivedKeySource), KeySource::Null(NullKeySource)]
        }
        Policy::Keymint => vec![KeySource::KeymintSealed],
    }
}

/// Returns all valid key sources when unsealing a volume, based on `policy`.
pub fn unseal_sources(policy: Policy) -> Vec<KeySource> {
    match policy {
        Policy::Null => vec![KeySource::Null(NullKeySource)],
        Policy::TeeRequired => vec![KeySource::TeeDerived(TeeDerivedKeySource)],
        Policy::TeeTransitional => {
            vec![KeySource::TeeDerived(TeeDerivedKeySource), KeySource::Null(NullKeySource)]
        }
        Policy::TeeOpportunistic => {
            vec![KeySource::TeeDerived(TeeDerivedKeySource), KeySource::Null(NullKeySource)]
        }
        Policy::Keymint => vec![KeySource::KeymintSealed],
    }
}
