// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, bail};
use assembly_container::{AssemblyContainer, FileType, WalkPaths, assembly_container};
use camino::Utf8PathBuf;
use ring::digest;
use serde::{Deserialize, Serialize};

/// The configuration file specifying where the generated images should be placed when flashing of
/// OTAing. This file lists the partitions used in three different flashing configurations:
///   fuchsia      - primary images in A/B, recovery in R, bootloaders, bootstrap
///   fuchsia_only - primary images in A/B, recovery in R, bootloaders
///   recovery     - recovery in A/B/R, bootloaders
#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, WalkPaths)]
#[serde(deny_unknown_fields)]
#[assembly_container(partitions_config.json)]
pub struct PartitionsConfig {
    /// Partitions that are only flashed in "fuchsia" configurations.
    #[serde(default)]
    #[walk_paths]
    pub bootstrap_partitions: Vec<BootstrapPartition>,

    /// Partitions designated for bootloaders, which are not slot-specific.
    #[serde(default)]
    #[walk_paths]
    pub bootloader_partitions: Vec<BootloaderPartition>,

    /// Non-bootloader partitions, which are slot-specific.
    #[serde(default)]
    pub partitions: Vec<Partition>,

    /// The name of the hardware to assert before flashing images to partitions.
    pub hardware_revision: String,

    /// The names of the products to accept in addition to the default "hw-revision" target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub product_matches: Vec<String>,

    /// Zip files containing the fastboot unlock credentials.
    #[serde(default)]
    #[walk_paths]
    pub unlock_credentials: Vec<Utf8PathBuf>,

    /// Optional configuration for sending SSH keys via fastboot OEM commands.
    ///
    /// If not provided, the configuration will use the default Fuchsia mechanism of passing SSH
    /// keys via `oem add-staged-bootloader-file ssh.authorized_keys`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_key_upload_method: Option<UploadMethod>,
}

/// The different mechanisms that can be used to upload data via fastboot OEM commands.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UploadMethod {
    /// Stage the data in RAM, then issue the OEM `command` to process it.
    Staged {
        /// The OEM command to run after staging the data.
        command: String,
    },
    /// Write the data as Base64 chunks directly embedded in consecutive OEM commands.
    Inline {
        /// The OEM command prefix for each data chunk, e.g. `my_command append=` will result in
        /// a message `oem my_command append=<base64_data>`.
        command_prefix: String,
        /// The maximum command length the device can receive in bytes including "oem ",
        /// `command_prefix`, and Base64 data. The actual message length may be less if the host
        /// transmit buffer is smaller.
        command_max_length: usize,
        /// Optional command to send once before the chunks, e.g. `my_command init`
        #[serde(default, skip_serializing_if = "Option::is_none")]
        init_command: Option<String>,
        /// Optional command to send after all chunks have finished, e.g. `my_command done`
        #[serde(default, skip_serializing_if = "Option::is_none")]
        finalize_command: Option<String>,
    },
}

impl UploadMethod {
    /// Calculates the available bytes for Base64 data in a message.
    ///
    /// # Arguments
    ///
    /// * `buffer_size`: the host TX buffer size.
    ///
    /// # Returns
    ///
    /// Returns the available data length per inline command, or error if either:
    ///
    /// * `command_prefix` takes up the entire buffer, leaving no room for data
    /// * `self` is not [UploadMethod::Inline]
    pub fn command_data_length(&self, buffer_size: usize) -> Result<usize> {
        match self {
            Self::Staged { .. } => bail!("Staged upload methods do not chunk data by length"),
            Self::Inline { command_prefix, command_max_length, .. } => {
                // Reserve 4 bytes for the "oem " command prefix.
                let available = std::cmp::min(*command_max_length, buffer_size).saturating_sub(4);
                if available <= command_prefix.len() {
                    bail!(
                        "Upload method command prefix '{}' is too large for available space ({})",
                        command_prefix.len(),
                        available
                    );
                }
                Ok(available - command_prefix.len())
            }
        }
    }
}

impl PartitionsConfig {
    /// Determine which recovery style we will be using, and throw an error if we find both AB and R
    /// style recoveries.
    pub fn recovery_style(&self) -> Result<RecoveryStyle> {
        let mut recovery_style = RecoveryStyle::NoRecovery;
        for partition in &self.partitions {
            if partition.slot() == Some(&Slot::R) {
                if recovery_style == RecoveryStyle::AB {
                    bail!("Partitions config cannot contain both AB and R slotted recoveries.");
                }
                recovery_style = RecoveryStyle::R;
            }

            if matches!(partition, Partition::RecoveryZBI { .. }) {
                if recovery_style == RecoveryStyle::R {
                    bail!("Partitions config cannot contain both AB and R slotted recoveries.");
                }
                recovery_style = RecoveryStyle::AB;
            }
        }
        Ok(recovery_style)
    }

    /// Compare self with `other`, ignoring file paths, and instead comparing
    /// the contents of the file paths.
    pub fn contents_eq(&self, other: &Self) -> Result<bool> {
        let start_time = std::time::Instant::now();

        // Clone so that we can replace the paths with digests for comparison.
        let mut one = self.clone();
        let mut two = other.clone();

        // Replace the path with the digest of the contents so that we compare
        // the contents rather than the file paths.
        let replace_paths_with_digest = |config: &mut Self| {
            config.walk_paths(&mut |path: &mut Utf8PathBuf,
                                    _dest: Utf8PathBuf,
                                    filetype: FileType| {
                match filetype {
                    FileType::Unknown => {
                        let bytes = std::fs::read(&path)
                            .with_context(|| format!("Reading contents of {}", path))?;
                        let digest = digest::digest(&digest::SHA256, &bytes);
                        let digest_string = hex::encode(digest.as_ref());
                        *path = digest_string.into();
                        Ok(())
                    }
                    FileType::PackageManifest => {
                        bail!("contents_eq does not support package manifests")
                    }
                    FileType::Directory => bail!("contents_eq does not support directories"),
                }
            })
        };
        replace_paths_with_digest(&mut one)?;
        replace_paths_with_digest(&mut two)?;

        // Compare.
        // Note: At the time of writing, this is taking less than 3ms for arm64.
        let equal = one.eq(&two);
        let duration = start_time.elapsed();
        log::info!("Comparing partitions config took: {:?}", duration);
        Ok(equal)
    }
}

/// A partition to flash in "fuchsia" configurations.
#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, WalkPaths)]
pub struct BootstrapPartition {
    /// The name of the partition known to fastboot.
    pub name: String,

    /// The path on host to the bootloader image.
    #[walk_paths]
    pub image: Utf8PathBuf,

    /// The condition that must be met before attempting to flash.
    pub condition: Option<BootstrapCondition>,
}

/// The fastboot variable condition that must equal the value before a bootstrap partition should
/// be flashed.
#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct BootstrapCondition {
    /// The name of the fastboot variable.
    pub variable: String,

    /// The expected value.
    pub value: String,
}

/// A single bootloader partition, which is not slot-specific.
#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, WalkPaths)]
pub struct BootloaderPartition {
    /// The firmware type provided to the update system.
    /// See documentation here:
    ///     https://fuchsia.dev/fuchsia-src/concepts/packages/update_pkg
    #[serde(rename = "type")]
    pub partition_type: String,

    /// The name of the partition known to fastboot.
    /// If the name is not provided, then the partition should not be flashed.
    pub name: Option<String>,

    /// The path on host to the bootloader image.
    #[walk_paths]
    pub image: Utf8PathBuf,
}

/// A non-bootloader partition which
#[derive(Clone, Debug, Deserialize, PartialOrd, Ord, Eq, PartialEq, Hash, Serialize)]
#[serde(tag = "type")]
pub enum Partition {
    /// A partition prepared for the Zircon Boot Image (ZBI).
    ZBI {
        /// The partition name.
        name: String,
        /// The slot of the partition.
        slot: Slot,
        /// An optional size constraint in bytes for the partition.
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },

    /// A partition prepared for the Recovery Zircon Boot Image (ZBI).
    /// This is for AB-slotted recovery images.
    /// R-slotted recovery images should use Partition::ZBI and Slot::R.
    RecoveryZBI {
        /// The partition name.
        name: String,
        /// The slot of the partition.
        slot: Slot,
        /// An optional size constraint in bytes for the partition.
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },

    /// A partition prepared for the Verified Boot Metadata (VBMeta).
    VBMeta {
        /// The partition name.
        name: String,
        /// The slot of the partition.
        slot: Slot,
        /// An optional size constraint for the partition.
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },

    /// A partition prepared for the Recovery Verified Boot Metadata (VBMeta).
    RecoveryVBMeta {
        /// The partition name.
        name: String,
        /// The slot of the partition.
        slot: Slot,
        /// An optional size constraint for the partition.
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },

    /// A partition prepared for the Fuchsia Volume Manager (FVM).
    FVM {
        /// The partition name.
        name: String,
        /// An optional size constraint for the partition.
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },

    /// A partition prepared for Fxfs.
    Fxfs {
        /// The partition name.
        name: String,
        /// An optional size constraint for the partition.
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },

    /// A partition preparted for a device tree binary overlay.
    Dtbo {
        /// The partition name.
        name: String,
        /// The slot of the partition.
        slot: Slot,
        /// An optional size constraint for the partition.
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },
}

impl Partition {
    /// The name of the partition.
    pub fn name(&self) -> &String {
        match &self {
            Self::ZBI { name, .. } => name,
            Self::RecoveryZBI { name, .. } => name,
            Self::VBMeta { name, .. } => name,
            Self::RecoveryVBMeta { name, .. } => name,
            Self::FVM { name, .. } => name,
            Self::Fxfs { name, .. } => name,
            Self::Dtbo { name, .. } => name,
        }
    }

    /// The slot of the partition, if applicable.
    pub fn slot(&self) -> Option<&Slot> {
        match &self {
            Self::ZBI { slot, .. } => Some(slot),
            Self::RecoveryZBI { slot, .. } => Some(slot),
            Self::VBMeta { slot, .. } => Some(slot),
            Self::RecoveryVBMeta { slot, .. } => Some(slot),
            Self::FVM { .. } => None,
            Self::Fxfs { .. } => None,
            Self::Dtbo { slot, .. } => Some(slot),
        }
    }

    /// The size budget of the partition, if supplied.
    pub fn size(&self) -> Option<&u64> {
        match &self {
            Self::ZBI { size, .. } => size.as_ref(),
            Self::RecoveryZBI { size, .. } => size.as_ref(),
            Self::VBMeta { size, .. } => size.as_ref(),
            Self::RecoveryVBMeta { size, .. } => size.as_ref(),
            Self::FVM { size, .. } => size.as_ref(),
            Self::Fxfs { size, .. } => size.as_ref(),
            Self::Dtbo { size, .. } => size.as_ref(),
        }
    }
}

/// The slots available for flashing or OTAing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum Slot {
    /// Primary slot.
    A,

    /// Alternate slot.
    B,

    /// Recovery slot.
    R,
}

impl std::fmt::Display for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::A => "A",
            Self::B => "B",
            Self::R => "R",
        };
        write!(f, "{}", message)
    }
}

/// The style of recovery.
#[derive(Debug, PartialEq)]
pub enum RecoveryStyle {
    /// No recovery images are present.
    NoRecovery,
    /// Recovery lives in a separate R slot.
    R,
    /// Recovery is updated alongside the "main" images in AB slots.
    AB,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8Path;
    use serde_json;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_partition_config(
        json: &str,
        additional_files: &[&str],
        contents_are_filename: bool,
    ) -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let mut partitions_config = File::create(base_path.join("partitions_config.json")).unwrap();
        partitions_config.write_all(json.as_bytes()).unwrap();

        additional_files.iter().for_each(|&file_name| {
            let mut file = File::create(base_path.join(file_name)).unwrap();
            if contents_are_filename {
                file.write_all(file_name.as_bytes()).unwrap();
            } else {
                file.write_all(b"").unwrap();
            }
        });

        temp_dir
    }

    #[test]
    fn from_json() {
        let json = r#"
            {
                bootloader_partitions: [
                    {
                        type: "tpl",
                        name: "firmware_tpl",
                        image: "tpl_image",
                    }
                ],
                partitions: [
                    {
                        type: "ZBI",
                        name: "zircon_a",
                        slot: "A",
                    },
                    {
                        type: "RecoveryZBI",
                        name: "recovery_a",
                        slot: "A",
                    },
                    {
                        type: "VBMeta",
                        name: "vbmeta_b",
                        slot: "B",
                    },
                    {
                        type: "RecoveryVBMeta",
                        name: "vbmeta_recovery_b",
                        slot: "B",
                    },
                    {
                        type: "FVM",
                        name: "fvm",
                    },
                    {
                        type: "Fxfs",
                        name: "fxfs",
                    },
                    {
                        type: "Dtbo",
                        name: "dtbo_a",
                        slot: "A",
                    },
                    {
                        type: "Dtbo",
                        name: "dtbo_b",
                        slot: "B",
                    },
                ],
                hardware_revision: "hw",
                unlock_credentials: [
                    "unlock_credentials.zip",
                ],
            }
        "#;
        let temp_dir =
            write_partition_config(json, &["tpl_image", "unlock_credentials.zip"], false);
        let test_dir = Utf8Path::from_path(temp_dir.path()).unwrap();

        let config = PartitionsConfig::from_dir(test_dir).unwrap();

        assert_eq!(config.bootloader_partitions[0].image, test_dir.join("tpl_image"));
        assert_eq!(config.unlock_credentials[0], test_dir.join("unlock_credentials.zip"));
        assert_eq!(config.partitions.len(), 8);
        assert_eq!(config.hardware_revision, "hw");
        assert_eq!(config.ssh_key_upload_method, None);
    }

    #[test]
    fn from_json_with_ssh_upload_method() {
        let json = r#"
            {
                bootloader_partitions: [],
                partitions: [],
                hardware_revision: "hw",
                ssh_key_upload_method: {
                    type: "inline",
                    init_command: "oem foo init",
                    command_prefix: "oem foo data=",
                    command_max_length: 64,
                    finalize_command: "oem foo finish"
                }
            }
        "#;
        let temp_dir =
            write_partition_config(json, &["tpl_image", "unlock_credentials.zip"], false);
        let test_dir = Utf8Path::from_path(temp_dir.path()).unwrap();

        let config = PartitionsConfig::from_dir(test_dir).unwrap();
        assert_eq!(
            config.ssh_key_upload_method,
            Some(UploadMethod::Inline {
                command_prefix: "oem foo data=".to_string(),
                command_max_length: 64,
                init_command: Some("oem foo init".to_string()),
                finalize_command: Some("oem foo finish".to_string())
            })
        );
    }

    #[test]
    fn from_json_with_ssh_upload_method_omit_init_and_finalize() {
        let json = r#"
            {
                bootloader_partitions: [],
                partitions: [],
                hardware_revision: "hw",
                ssh_key_upload_method: {
                    type: "inline",
                    command_prefix: "oem foo data=",
                    command_max_length: 64,
                }
            }
        "#;
        let temp_dir =
            write_partition_config(json, &["tpl_image", "unlock_credentials.zip"], false);
        let test_dir = Utf8Path::from_path(temp_dir.path()).unwrap();

        let config = PartitionsConfig::from_dir(test_dir).unwrap();
        assert_eq!(
            config.ssh_key_upload_method,
            Some(UploadMethod::Inline {
                command_prefix: "oem foo data=".to_string(),
                command_max_length: 64,
                init_command: None,
                finalize_command: None,
            })
        );
    }

    #[test]
    fn upload_method_command_data_length_max_length_bottleneck() {
        let method = UploadMethod::Inline {
            command_prefix: "1234567890".to_string(),
            command_max_length: 64,
            init_command: None,
            finalize_command: None,
        };
        // `command_max_length` bottleneck: 64 bytes - 10 `command_prefix` - 4 "oem " = 50.
        assert_eq!(method.command_data_length(256).unwrap(), 50);
    }

    #[test]
    fn upload_method_command_data_length_buffer_size_bottleneck() {
        let method = UploadMethod::Inline {
            command_prefix: "1234567890".to_string(),
            command_max_length: 50,
            init_command: None,
            finalize_command: None,
        };
        // Buffer size bottleneck: 40 bytes - 10 `command_prefix` - 4 "oem " = 26.
        assert_eq!(method.command_data_length(40).unwrap(), 26);
    }

    #[test]
    fn upload_method_command_data_length_error() {
        let method_overflow = UploadMethod::Inline {
            command_prefix: "1234567890".to_string(),
            command_max_length: 12,
            init_command: None,
            finalize_command: None,
        };
        // Can't fit 10-byte `command_prefix` + 4-byte "oem " in 12 bytes.
        assert!(method_overflow.command_data_length(64).is_err());
    }

    #[test]
    fn serialize_product_matches() {
        // Empty `product_matches` should be skipped when serializing, so that older FFX builds
        // can continue to use newer configs as long as they don't specify a product match.
        let config = PartitionsConfig {
            hardware_revision: "hw".to_string(),
            product_matches: vec![],
            ..Default::default()
        };
        let json = serde_json::to_value(&config).unwrap();
        assert!(json.get("product_matches").is_none());

        // Non-empty `product_matches` should exist in the serialization, older FFX builds will not
        // be able to flash these configs.
        let config_with_matches = PartitionsConfig {
            hardware_revision: "hw".to_string(),
            product_matches: vec!["product_test".to_string()],
            ..Default::default()
        };
        let json_with_matches = serde_json::to_value(&config_with_matches).unwrap();
        assert!(json_with_matches.get("product_matches").is_some());
        assert_eq!(
            json_with_matches.get("product_matches").unwrap(),
            &serde_json::json!(["product_test"])
        );
    }

    #[test]
    fn invalid_partition_type() {
        let json = r#"
            {
                bootloader_partitions: [],
                partitions: [
                    {
                        type: "Invalid",
                        name: "zircon",
                        slot: "SlotA",
                    }
                ],
                "hardware_revision": "hw",
            }
        "#;
        let temp_dir = write_partition_config(json, &[], false);
        let test_dir = Utf8Path::from_path(temp_dir.path()).unwrap();

        let config = PartitionsConfig::from_dir(test_dir);

        assert!(config.is_err());
    }

    #[test]
    fn compare_equal() {
        let json1 = r#"
            {
                bootloader_partitions: [
                    {
                        type: "tpl",
                        name: "firmware_tpl",
                        image: "tpl_image1",
                    }
                ],
                partitions: [
                    {
                        type: "ZBI",
                        name: "zircon_a",
                        slot: "A",
                    },
                ],
                hardware_revision: "hw",
                unlock_credentials: [
                    "unlock_credentials1.zip",
                ],
            }
        "#;
        let temp_dir1 =
            write_partition_config(json1, &["tpl_image1", "unlock_credentials1.zip"], false);
        let test_dir1 = Utf8Path::from_path(temp_dir1.path()).unwrap();
        let config1 = PartitionsConfig::from_dir(test_dir1).unwrap();

        let json2 = r#"
            {
                bootloader_partitions: [
                    {
                        type: "tpl",
                        name: "firmware_tpl",
                        image: "tpl_image2",
                    }
                ],
                partitions: [
                    {
                        type: "ZBI",
                        name: "zircon_a",
                        slot: "A",
                    },
                ],
                hardware_revision: "hw",
                unlock_credentials: [
                    "unlock_credentials2.zip",
                ],
            }
        "#;
        let temp_dir2 =
            write_partition_config(json2, &["tpl_image2", "unlock_credentials2.zip"], false);
        let test_dir2 = Utf8Path::from_path(temp_dir2.path()).unwrap();
        let config2 = PartitionsConfig::from_dir(test_dir2).unwrap();

        assert!(config1.contents_eq(&config2).unwrap());
    }

    #[test]
    fn compare_not_equal_partition_names() {
        let json1 = r#"
            {
                bootloader_partitions: [
                    {
                        type: "tpl",
                        name: "firmware_tpl",
                        image: "tpl_image1",
                    }
                ],
                partitions: [
                    {
                        type: "ZBI",
                        name: "zircon_a",
                        slot: "A",
                    },
                ],
                hardware_revision: "hw",
                unlock_credentials: [
                    "unlock_credentials1.zip",
                ],
            }
        "#;
        let temp_dir1 =
            write_partition_config(json1, &["tpl_image1", "unlock_credentials1.zip"], false);
        let test_dir1 = Utf8Path::from_path(temp_dir1.path()).unwrap();
        let config1 = PartitionsConfig::from_dir(test_dir1).unwrap();

        let json2 = r#"
            {
                bootloader_partitions: [
                    {
                        type: "tpl",
                        name: "firmware_tpl",
                        image: "tpl_image2",
                    }
                ],
                partitions: [
                    {
                        type: "ZBI",
                        name: "zircon_a_different",
                        slot: "A",
                    },
                ],
                hardware_revision: "hw",
                unlock_credentials: [
                    "unlock_credentials2.zip",
                ],
            }
        "#;
        let temp_dir2 =
            write_partition_config(json2, &["tpl_image2", "unlock_credentials2.zip"], false);
        let test_dir2 = Utf8Path::from_path(temp_dir2.path()).unwrap();
        let config2 = PartitionsConfig::from_dir(test_dir2).unwrap();

        assert!(!config1.contents_eq(&config2).unwrap());
    }

    #[test]
    fn compare_not_equal_contents() {
        let json1 = r#"
            {
                bootloader_partitions: [
                    {
                        type: "tpl",
                        name: "firmware_tpl",
                        image: "tpl_image1",
                    }
                ],
                partitions: [
                    {
                        type: "ZBI",
                        name: "zircon_a",
                        slot: "A",
                    },
                ],
                hardware_revision: "hw",
                unlock_credentials: [
                    "unlock_credentials1.zip",
                ],
            }
        "#;
        let temp_dir1 =
            write_partition_config(json1, &["tpl_image1", "unlock_credentials1.zip"], true);
        let test_dir1 = Utf8Path::from_path(temp_dir1.path()).unwrap();
        let config1 = PartitionsConfig::from_dir(test_dir1).unwrap();

        let json2 = r#"
            {
                bootloader_partitions: [
                    {
                        type: "tpl",
                        name: "firmware_tpl",
                        image: "tpl_image2",
                    }
                ],
                partitions: [
                    {
                        type: "ZBI",
                        name: "zircon_a",
                        slot: "A",
                    },
                ],
                hardware_revision: "hw",
                unlock_credentials: [
                    "unlock_credentials2.zip",
                ],
            }
        "#;
        let temp_dir2 =
            write_partition_config(json2, &["tpl_image2", "unlock_credentials2.zip"], true);
        let test_dir2 = Utf8Path::from_path(temp_dir2.path()).unwrap();
        let config2 = PartitionsConfig::from_dir(test_dir2).unwrap();

        assert!(!config1.contents_eq(&config2).unwrap());
    }
}
