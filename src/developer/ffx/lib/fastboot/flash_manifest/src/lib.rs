// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::v1::FlashManifest as FlashManifestV1;
use crate::v2::FlashManifest as FlashManifestV2;
use crate::v3::{Condition, FlashManifest as FlashManifestV3, Partition, Product};
use crate::v4::FlashManifest as FlashManifestV4;
use assembly_partitions_config::{PartitionAndImage, PartitionImageMapper, Slot};
use product_bundle::{ProductBundle, ProductBundleV2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, from_value, to_value};
use std::default::Default;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub mod v1;
pub mod v2;
pub mod v3;
pub mod v4;

pub const UNKNOWN_VERSION: &str = "Unknown flash manifest version";

// The default OEM command to process the staged SSH keys.
pub const SSH_OEM_COMMAND: &str = "add-staged-bootloader-file ssh.authorized_keys";

#[derive(thiserror::Error, Debug)]
pub enum FlashManifestError {
    #[error(
        "Unrecognized OEM staged file. Expected comma-separated pair: \"<OEM_COMMAND>,<PATH_TO_FILE>\""
    )]
    InvalidOemFile,

    #[error("File does not exist: {0}")]
    FileDoesNotExist(String),

    #[error("JSON serialization error: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("JSON deserialization error: {0}")]
    Deserialize(serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Could not parse flash manifest")]
    ParseManifest,

    #[error("Unknown flash manifest version")]
    UnknownVersion,

    #[error("Product bundle error: {0}")]
    ProductBundle(String),

    #[error("Partition image mapper error: {0}")]
    PartitionMapper(String),
}

#[derive()]
pub struct ManifestParams {
    pub manifest: Option<PathBuf>,
    pub product: String,
    pub product_bundle: Option<String>,
    pub oem_stage: Vec<OemFile>,
    pub no_bootloader_reboot: bool,
    pub skip_verify: bool,
    pub op: Command,
    pub flash_timeout_rate_mb_per_second: f64,
    pub flash_min_timeout_seconds: u64,
    pub ssh_key: Option<String>,
}

impl Default for ManifestParams {
    fn default() -> Self {
        Self {
            manifest: None,
            product: "fuchsia".to_string(),
            product_bundle: None,
            oem_stage: vec![],
            no_bootloader_reboot: false,
            skip_verify: false,
            op: Command::Flash,
            flash_timeout_rate_mb_per_second: 5000.0,
            flash_min_timeout_seconds: 200,
            ssh_key: None,
        }
    }
}

#[derive(Debug)]
pub enum Command {
    Flash,
    Unlock(UnlockParams),
    Boot(BootParams),
}

#[derive(Debug)]
pub struct UnlockParams {
    pub cred: Option<String>,
    pub force: bool,
}

#[derive(Debug)]
pub struct BootParams {
    pub zbi: Option<String>,
    pub vbmeta: Option<String>,
    pub slot: String,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OemFile(String, String);

impl OemFile {
    pub fn new(command: String, path: String) -> Self {
        Self(command, path)
    }

    pub fn command(&self) -> &str {
        self.0.as_str()
    }

    pub fn file(&self) -> &str {
        self.1.as_str()
    }
}

impl std::str::FromStr for OemFile {
    type Err = FlashManifestError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(FlashManifestError::InvalidOemFile);
        }

        let splits: Vec<&str> = s.split(",").collect();

        if splits.len() != 2 {
            return Err(FlashManifestError::InvalidOemFile);
        }

        let file = Path::new(splits[1]);
        if !file.exists() {
            return Err(FlashManifestError::FileDoesNotExist(splits[1].to_string()));
        }

        Ok(Self(splits[0].to_string(), file.to_string_lossy().to_string()))
    }
}

#[derive(Debug)]
pub enum FlashManifestVersion {
    V1(FlashManifestV1),
    V2(FlashManifestV2),
    V3(FlashManifestV3),
    V4(FlashManifestV4),
}

impl FlashManifestVersion {
    pub fn write<W: Write>(&self, writer: W) -> Result<(), FlashManifestError> {
        let manifest = match &self {
            FlashManifestVersion::V1(manifest) => ManifestFile {
                version: 1,
                manifest: to_value(manifest).map_err(FlashManifestError::Serialize)?,
            },
            FlashManifestVersion::V2(manifest) => ManifestFile {
                version: 2,
                manifest: to_value(manifest).map_err(FlashManifestError::Serialize)?,
            },
            FlashManifestVersion::V3(manifest) => ManifestFile {
                version: 3,
                manifest: to_value(manifest).map_err(FlashManifestError::Serialize)?,
            },
            FlashManifestVersion::V4(manifest) => ManifestFile {
                version: 4,
                manifest: to_value(manifest).map_err(FlashManifestError::Serialize)?,
            },
        };
        serde_json::to_writer_pretty(writer, &manifest).map_err(FlashManifestError::Serialize)?;
        Ok(())
    }

    pub fn load<R: Read>(reader: R) -> Result<Self, FlashManifestError> {
        let value: Value =
            serde_json::from_reader::<R, Value>(reader).map_err(FlashManifestError::Deserialize)?;
        // GN generated JSON always comes from a list
        let manifest: ManifestFile = match value {
            Value::Array(v) => from_value(v[0].clone()).map_err(FlashManifestError::Deserialize)?,
            Value::Object(_) => from_value(value).map_err(FlashManifestError::Deserialize)?,
            _ => return Err(FlashManifestError::ParseManifest),
        };
        match manifest.version {
            1 => Ok(Self::V1(
                from_value(manifest.manifest).map_err(FlashManifestError::Deserialize)?,
            )),
            2 => Ok(Self::V2(
                from_value(manifest.manifest).map_err(FlashManifestError::Deserialize)?,
            )),
            3 => Ok(Self::V3(
                from_value(manifest.manifest).map_err(FlashManifestError::Deserialize)?,
            )),
            4 => Ok(Self::V4(
                from_value(manifest.manifest).map_err(FlashManifestError::Deserialize)?,
            )),
            _ => return Err(FlashManifestError::UnknownVersion),
        }
    }

    pub fn from_product_bundle(product_bundle: &ProductBundle) -> Result<Self, FlashManifestError> {
        match product_bundle {
            ProductBundle::V2(product_bundle) => Self::from_product_bundle_v2(product_bundle),
        }
    }

    fn from_product_bundle_v2(
        product_bundle: &ProductBundleV2,
    ) -> Result<Self, FlashManifestError> {
        log::debug!("Begin loading flash manifest from ProductBundleV2: {:#?}", product_bundle);
        // Copy the unlock credentials from the partitions config to the flash manifest.
        let mut credentials = vec![];
        for c in &product_bundle.partitions.unlock_credentials {
            log::debug!("Adding unlock credential: {}", c);
            credentials.push(c.to_string());
        }

        // Copy the bootloader partitions from the partitions config to the flash manifest.
        let mut bootloader_partitions = vec![];
        for p in &product_bundle.partitions.bootloader_partitions {
            if let Some(name) = &p.name {
                let partition = Partition {
                    name: name.to_string(),
                    path: p.image.to_string(),
                    condition: None,
                };
                log::debug!("Adding bootloader partition: {:#?}", partition);
                bootloader_partitions.push(partition);
            }
        }

        // Copy the bootstrap partitions from the partitions config to the flash manifest.
        let mut bootstrap_partitions = vec![];
        for p in &product_bundle.partitions.bootstrap_partitions {
            let condition = if let Some(c) = &p.condition {
                Some(Condition { variable: c.variable.to_string(), value: c.value.to_string() })
            } else {
                None
            };
            let partition =
                Partition { name: p.name.to_string(), path: p.image.to_string(), condition };
            log::debug!("Adding bootstrap partition: {:#?}", partition);
            bootstrap_partitions.push(partition);
        }
        // Append the bootloader partitions, bootstrapping a device means flashing any initial
        // bootstrap images plus a working bootloader. The bootstrap partitions should always come
        // first as the lowest-level items so that the higher-level bootloader images can depend on
        // bootstrapping being done.
        bootstrap_partitions.extend_from_slice(bootloader_partitions.as_slice());

        // Create a map from slot to available images by name (zbi, vbmeta, fvm).
        let mut image_map = PartitionImageMapper::new(product_bundle.partitions.clone())
            .map_err(|e| FlashManifestError::PartitionMapper(e.to_string()))?;
        if let Some(manifest) = &product_bundle.system_a {
            let slot = Slot::A;
            log::debug!("Mapping images: {:?} to slot: {}", manifest, slot);
            image_map
                .map_images_to_slot(&manifest, slot)
                .map_err(|e| FlashManifestError::PartitionMapper(e.to_string()))?;
        }
        if let Some(manifest) = &product_bundle.system_b {
            let slot = Slot::B;
            log::debug!("Mapping images: {:?} to slot: {}", manifest, slot);
            image_map
                .map_images_to_slot(&manifest, slot)
                .map_err(|e| FlashManifestError::PartitionMapper(e.to_string()))?;
        }
        if let Some(manifest) = &product_bundle.system_r {
            let slot = Slot::R;
            log::debug!("Mapping images: {:?} to slot: {}", manifest, slot);
            image_map
                .map_images_to_slot(&manifest, slot)
                .map_err(|e| FlashManifestError::PartitionMapper(e.to_string()))?;
        }

        // Define the flashable "products".
        let mut products = vec![];
        products.push(Product {
            name: "recovery".into(),
            bootloader_partitions: bootloader_partitions.clone(),
            partitions: get_mapped_partitions(&image_map, /*is_recovery=*/ true),
            oem_files: vec![],
            requires_unlock: false,
        });
        products.push(Product {
            name: "fuchsia_only".into(),
            bootloader_partitions: bootloader_partitions.clone(),
            partitions: get_mapped_partitions(&image_map, /*is_recovery=*/ false),
            oem_files: vec![],
            requires_unlock: false,
        });
        products.push(Product {
            name: "fuchsia".into(),
            bootloader_partitions: bootstrap_partitions.clone(),
            partitions: get_mapped_partitions(&image_map, /*is_recovery=*/ false),
            oem_files: vec![],
            requires_unlock: !product_bundle.partitions.bootstrap_partitions.is_empty(),
        });
        if !product_bundle.partitions.bootstrap_partitions.is_empty() {
            products.push(Product {
                name: "bootstrap".into(),
                bootloader_partitions: bootstrap_partitions.clone(),
                partitions: vec![],
                oem_files: vec![],
                requires_unlock: true,
            });
        }

        let v3 = FlashManifestV3 {
            hw_revision: product_bundle.partitions.hardware_revision.clone(),
            product_matches: product_bundle.partitions.product_matches.clone(),
            credentials,
            products,
        };

        // For backwards compatibility create a V3 manifest if possible, only use V4 if the PB
        // contains the SSH key upload method which didn't exist in V3. This will allow older
        // versions of FFX to use this flash manifest as long as it didn't use V4-only features.
        let ret = match &product_bundle.partitions.ssh_key_upload_method {
            Some(ssh_key_upload_method) => Self::V4(FlashManifestV4 {
                v3,
                ssh_key_upload_method: Some(ssh_key_upload_method.clone()),
            }),
            None => Self::V3(v3),
        };

        log::debug!("Created FlashManifest: {:#?}", ret);

        Ok(ret)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManifestFile {
    manifest: Value,
    version: u64,
}

/// Construct a list of partitions to add to the flash manifest by mapping the partitions to the
/// images. If |is_recovery|, then put the recovery images in every slot.
fn get_mapped_partitions(image_map: &PartitionImageMapper, _is_recovery: bool) -> Vec<Partition> {
    let partition_map = image_map.map();
    partition_map
        .iter()
        .map(|PartitionAndImage { partition, path }| Partition {
            name: partition.name().clone(),
            path: path.to_string(),
            condition: None,
        })
        .collect()
}
