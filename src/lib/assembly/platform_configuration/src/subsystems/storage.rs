// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::{Context, bail, ensure};
use assembly_config_capabilities::{Config, ConfigValueType};
use assembly_config_schema::platform_settings::development_support_config::StorageToolsConfig;
use assembly_config_schema::platform_settings::recovery_config::RecoveryConfig;
use assembly_config_schema::platform_settings::storage_config::StorageConfig;
use assembly_constants::{BoardFeature, BootfsDestination, FileEntry};
use assembly_images_config::{
    BlobfsLayout, DataFilesystemFormat, FilesystemImageMode, FlexibleSize, FvmConfig, GptMode,
    VolumeConfig,
};

pub(crate) struct StorageSubsystemConfig;
impl DefineSubsystemConfiguration<(&StorageConfig, &StorageToolsConfig, &RecoveryConfig)>
    for StorageSubsystemConfig
{
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        configs: &(&StorageConfig, &StorageToolsConfig, &RecoveryConfig),
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        let (storage_config, storage_tools_config, recovery_config) = *configs;
        if matches!(
            context.feature_set_level,
            FeatureSetLevel::Bootstrap | FeatureSetLevel::Embeddable
        ) {
            ensure!(
                storage_config.filesystems.image_mode == FilesystemImageMode::NoImage,
                "Bootstrap and Embeddable products must use filesystems.image_mode='no_image'"
            );
        }

        // Include legacy paver implementation in all feature sets above "embeddable" if the board
        // doesn't include it. Embeddable doesn't support paving.
        if *context.feature_set_level != FeatureSetLevel::Embeddable
            && !context.board_config.provides_feature(BoardFeature::Paver)
        {
            builder.platform_bundle("paver_legacy")?;
        }
        // `paver` goes in the base package set on builds that have it, which saves memory compared
        // to bootfs because bootfs always keeps its blobs resident in memory. This means that
        // paver's position in the component topology differs between bootstrap and higher feature
        // sets.
        match *context.feature_set_level {
            FeatureSetLevel::Embeddable => {
                // Embeddable doesn't support paving.
            }
            FeatureSetLevel::Bootstrap => builder.platform_bundle("paver_shards_bootstrap")?,
            FeatureSetLevel::Utility | FeatureSetLevel::Standard => {
                builder.platform_bundle("paver_shards_core")?;
            }
        }

        // Add auto-slot-committer if requested and supported by the board. This unconditionally
        // marks the current slot as successful on boot, to avoid triggering bootloader slot
        // failure logic after consecutive boots.
        if storage_config.auto_slot_committer
            && context.board_config.provides_feature(BoardFeature::SlotManagement)
        {
            builder.platform_bundle("auto_slot_committer")?;
        }

        // Include fuchsia.fshost/Recovery capabilities if this configuration supports recovery
        // (e.g. userspace fastboot or FDR), or if we are including partitioning tools that require
        // recovery functionality (e.g. to reset the device partition tables or flash a new system).
        if recovery_config.system_recovery.is_some()
            || storage_tools_config.enable_partitioning_tools
        {
            builder.platform_bundle("fshost_recovery")?;
        } else {
            builder.platform_bundle("fshost_non_recovery")?;
        }

        // Fetch a custom gen directory for placing temporary files. We get this
        // from the context, so that it can create unique gen directories for
        // each subsystem under the top-level assembly gen directory.
        let gendir = context.get_gendir().context("Getting gendir for storage subsystem")?;

        // Set the storage security policy/configuration for data encryption.
        // The filename "zxcrypt" is used for legacy reasons.
        let data_encryption_config_path = gendir.join("zxcrypt");

        // Use "null" encryption if:
        // 1. The filesystem image runs entirely out of an ephemeral ramdisk (such as
        //    recovery.zbi), where data only exists in volatile RAM and hardware keys
        //    must not be allocated or sealed.
        // 2. The feature set level is not Standard (e.g. Utility/Bootstrap/Embeddable),
        //    because non-Standard builds do not include the userland KeyMint service
        //    stack or TEE realm in `#core` (see `tee.rs`).
        let policy = if storage_config.filesystems.image_mode == FilesystemImageMode::Ramdisk
            || *context.feature_set_level != FeatureSetLevel::Standard
        {
            "null"
        } else if context.board_config.filesystems.keymint_enabled {
            ensure!(
                context.board_config.provides_feature(BoardFeature::Keymint),
                "fuchsia::keymint is not provided by the board, can't use keymint."
            );
            "keymint"
        } else if context.board_config.provides_feature(BoardFeature::KeysafeTa) {
            "tee"
        } else {
            "null"
        };
        std::fs::write(&data_encryption_config_path, policy)
            .context("Could not write data encryption configuration")?;

        let inline_crypto = Config::new_bool(
            context.board_config.provides_feature(BoardFeature::StorageInlineCrypto),
        );

        let block_config_path = gendir.join("fshost_block_config.json");
        let block_config_json =
            serde_json::to_string(&context.board_config.filesystems.block_devices)
                .context("Serializing devices config")?;
        std::fs::write(&block_config_path, &block_config_json)
            .context("Writing serialized devices config")?;
        builder
            .bootfs()
            .file(FileEntry { source: block_config_path, destination: BootfsDestination::Fshost })
            .context("Adding fshost config to bootfs")?;

        builder
            .bootfs()
            .file(FileEntry {
                source: data_encryption_config_path,
                destination: BootfsDestination::Zxcrypt,
            })
            .context("Adding zxcrypt config to bootfs")?;

        if *context.feature_set_level == FeatureSetLevel::Embeddable {
            // We don't need fshost in embeddable.
            return Ok(());
        }

        if storage_config.factory_data.enabled {
            builder.platform_bundle("factory_data")?;
        }

        if storage_config.mutable_storage_garbage_collection {
            context.ensure_feature_set_level(
                &[FeatureSetLevel::Standard],
                "Mutable storage garbage collection",
            )?;
            builder.platform_bundle("storage_cache_manager")?;
        }

        // Collect the arguments from the board.
        let blobfs_initial_inodes =
            context.board_config.filesystems.fvm.blobfs.minimum_inodes.unwrap_or(0);
        let fvm_slice_size = context.board_config.filesystems.fvm.slice_size.0;
        let gpt = context.board_config.filesystems.gpt.enabled();
        let gpt_all = context.board_config.filesystems.gpt_all
            || context.board_config.filesystems.gpt == GptMode::AllowMultiple;
        let merge_super_and_userdata = context.board_config.filesystems.merge_super_and_userdata;

        // Collect the arguments from the product.
        let ramdisk_image = storage_config.filesystems.image_mode == FilesystemImageMode::Ramdisk;
        let no_zxcrypt = storage_config.filesystems.no_zxcrypt;
        let format_data_on_corruption = storage_config.filesystems.format_data_on_corruption.0;
        let provision_fxfs = storage_config.provision_fxfs;
        let sdmmc_command_queueing = context.board_config.provides_feature(BoardFeature::SdmmcCqe);

        // Apply limits.
        let (blob_max_bytes, data_max_bytes) = match &storage_config.filesystems.volume {
            VolumeConfig::Fxfs => {
                let resolve_size = |flexible_size: &Option<FlexibleSize>| -> u64 {
                    flexible_size
                        .as_ref()
                        .map(|size| match size {
                            FlexibleSize::Uniform(u) => *u,
                            FlexibleSize::BuildSpecific(b) => match context.build_type {
                                BuildType::Eng => b.eng.unwrap_or(0),
                                BuildType::UserDebug => b.userdebug.unwrap_or(0),
                                BuildType::User => b.user.unwrap_or(0),
                            },
                        })
                        .unwrap_or(0)
                };
                (
                    resolve_size(&context.board_config.filesystems.fxfs.blob_maximum_bytes),
                    resolve_size(&context.board_config.filesystems.fxfs.data_maximum_bytes),
                )
            }
            VolumeConfig::Fvm(_) => (
                context.board_config.filesystems.fvm.blobfs.maximum_bytes.unwrap_or(0),
                context.board_config.filesystems.fvm.minfs.maximum_bytes.unwrap_or(0),
            ),
        };

        // Prepare some default arguments that may get overridden by the product config.
        let mut blob_deprecated_padded = false;
        let mut data_filesystem_format_str = "fxfs";
        let mut fxfs_blob = false;

        // Add all the AIBs and collect some argument values.
        builder.platform_bundle("fshost_common")?;
        builder.platform_bundle("fshost_storage")?;
        match &storage_config.filesystems.volume {
            VolumeConfig::Fxfs => {
                ensure!(gpt, "GPT required for Fxfs-based product assemblies");
                fxfs_blob = true;
                builder.platform_bundle("fshost_fxfs")?;
                if provision_fxfs {
                    builder.platform_bundle("fshost_provision_fxfs")?;
                }
            }
            VolumeConfig::Fvm(FvmConfig { blob, data, .. }) => {
                blob_deprecated_padded = blob.blob_layout == BlobfsLayout::DeprecatedPadded;
                match data.data_filesystem_format {
                    DataFilesystemFormat::Fxfs => {
                        bail!("Fxfs-in-FVM isn't supported");
                    }
                    DataFilesystemFormat::F2fs => {
                        context.ensure_build_type(&[BuildType::Eng], "GPT with FVM and F2FS")?;
                        data_filesystem_format_str = "f2fs";
                        if gpt {
                            builder.platform_bundle("fshost_gpt_fvm_f2fs")?;
                        } else {
                            // NOTE: There is no technical reason that this can't be supported,
                            // but there is no need for it at this time, as no products use f2fs
                            // without GPT.
                            bail!("f2fs without GPT is not supported");
                        }
                    }
                    DataFilesystemFormat::Minfs => {
                        data_filesystem_format_str = "minfs";
                        if gpt {
                            builder.platform_bundle("fshost_gpt_fvm_minfs")?;
                        } else {
                            builder.platform_bundle("fshost_fvm_minfs")?;
                        }
                    }
                }
            }
        }

        if context.build_type == &BuildType::Eng {
            builder.platform_bundle("fshost_eng")?;
        } else {
            builder.platform_bundle("fshost_non_eng")?;
        }

        let disable_automount =
            Config::new(ConfigValueType::Bool, storage_config.disable_automount.into());

        let starnix_volume_name = match &storage_config.starnix_volume.name {
            Some(name) => {
                if !fxfs_blob {
                    return Err(anyhow::anyhow!(
                        "Cannot have a starnix volume set for a non-fxblob configuration"
                    ));
                }
                Config::new(ConfigValueType::String { max_size: 64 }, name.clone().into())
            }
            None => Config::new(ConfigValueType::String { max_size: 64 }, "".into()),
        };
        let watch_deprecated_v1_drivers =
            context.board_config.filesystems.watch_deprecated_v1_drivers;

        let configs = [
            ("fuchsia.fshost.Blobfs", Config::new_bool(true)),
            ("fuchsia.fshost.BlobMaxBytes", Config::new_uint64(blob_max_bytes)),
            ("fuchsia.fshost.CheckFilesystems", Config::new_bool(true)),
            ("fuchsia.fshost.Data", Config::new_bool(true)),
            ("fuchsia.fshost.DataMaxBytes", Config::new_uint64(data_max_bytes)),
            ("fuchsia.fshost.DisableBlockWatcher", Config::new_bool(false)),
            ("fuchsia.fshost.Factory", Config::new_bool(false)),
            ("fuchsia.fshost.Fvm", Config::new_bool(true)),
            ("fuchsia.fshost.RamdiskImage", Config::new_bool(ramdisk_image)),
            ("fuchsia.fshost.Gpt", Config::new_bool(gpt)),
            ("fuchsia.fshost.GptAll", Config::new_bool(gpt_all)),
            ("fuchsia.fshost.MergeSuperAndUserdata", Config::new_bool(merge_super_and_userdata)),
            ("fuchsia.fshost.NoZxcrypt", Config::new_bool(no_zxcrypt)),
            ("fuchsia.fshost.FormatDataOnCorruption", Config::new_bool(format_data_on_corruption)),
            ("fuchsia.fshost.BlobfsInitialInodes", Config::new_uint64(blobfs_initial_inodes)),
            (
                "fuchsia.fshost.BlobfsUseDeprecatedPaddedFormat",
                Config::new_bool(blob_deprecated_padded),
            ),
            ("fuchsia.fshost.FxfsBlob", Config::new_bool(fxfs_blob)),
            ("fuchsia.fshost.FvmSliceSize", Config::new_uint64(fvm_slice_size)),
            (
                "fuchsia.fshost.DataFilesystemFormat",
                Config::new(
                    ConfigValueType::String { max_size: 64 },
                    data_filesystem_format_str.into(),
                ),
            ),
            (
                "fuchsia.fshost.FxfsCryptUrl",
                Config::new(
                    ConfigValueType::String { max_size: 64 },
                    "fuchsia-boot:///fxfs-crypt#meta/fxfs-crypt.cm".into(),
                ),
            ),
            ("fuchsia.fshost.DisableAutomount", disable_automount),
            ("fuchsia.fshost.StarnixVolumeName", starnix_volume_name),
            ("fuchsia.fshost.InlineCrypto", inline_crypto),
            ("fuchsia.fshost.ProvisionFxfs", Config::new_bool(provision_fxfs)),
            (
                "fuchsia.fshost.WatchDeprecatedV1Drivers",
                Config::new_bool(watch_deprecated_v1_drivers),
            ),
            (
                "fuchsia.storage.SdmmcCommandQueueingEnabled",
                Config::new_bool(sdmmc_command_queueing),
            ),
        ];
        for config in configs {
            builder.set_config_capability(config.0, config.1)?;
        }

        // Include SDHCI driver through a platform AIB.
        if context.board_config.provides_feature(BoardFeature::Sdhci) {
            builder.platform_bundle("sdhci_driver")?;
        }

        // Include CQHCI driver through a platform AIB.
        if sdmmc_command_queueing {
            builder.platform_bundle("cqhci_driver")?;
        }

        // Include UFS PCI driver through a platform AIB.
        if context.board_config.provides_feature(BoardFeature::UfsPci) {
            builder.platform_bundle("ufs_pci_driver")?;
        }

        // Include UFS PDev driver through a platform AIB.
        if context.board_config.provides_feature(BoardFeature::UfsPdev) {
            builder.platform_bundle("ufs_pdev_driver")?;
        }

        // In engineering builds, include the ufsutil CLI tool when UFS device
        // support is enabled.
        if (context.board_config.provides_feature(BoardFeature::UfsPci)
            || context.board_config.provides_feature(BoardFeature::UfsPdev))
            && context.build_type == &BuildType::Eng
        {
            builder.platform_bundle("ufsutil")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsystems::ConfigurationBuilderImpl;
    use assembly_config_schema::board_config::BoardConfig;
    use assembly_images_config::BoardFilesystemConfig;
    use camino::Utf8Path;
    use tempfile::tempdir;

    fn setup_test_context<'a>(
        board_config: &'a BoardConfig,
        gendir_path: &'a Utf8Path,
    ) -> ConfigurationContext<'a> {
        ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config,
            gendir: gendir_path.to_path_buf(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        }
    }

    #[test]
    fn test_keymint_enabled() {
        let board_config = BoardConfig {
            provided_features: vec![BoardFeature::Keymint.as_ref().to_string()],
            filesystems: BoardFilesystemConfig { keymint_enabled: true, ..Default::default() },
            ..Default::default()
        };
        let gendir = tempdir().unwrap();
        let gendir_path = Utf8Path::from_path(gendir.path()).unwrap();
        let context = setup_test_context(&board_config, gendir_path);
        let storage_config = StorageConfig::default();
        let mut builder: ConfigurationBuilderImpl = Default::default();

        let result = StorageSubsystemConfig::define_configuration(
            &context,
            &(&storage_config, &StorageToolsConfig::default(), &RecoveryConfig::default()),
            &mut builder,
        );
        assert!(result.is_ok());
        let zxcrypt_content = std::fs::read_to_string(gendir_path.join("zxcrypt")).unwrap();
        assert_eq!(zxcrypt_content, "keymint");
    }

    #[test]
    fn test_keymint_disabled() {
        let board_config = BoardConfig {
            provided_features: vec![BoardFeature::Keymint.as_ref().to_string()],
            filesystems: BoardFilesystemConfig { keymint_enabled: false, ..Default::default() },
            ..Default::default()
        };
        let gendir = tempdir().unwrap();
        let gendir_path = Utf8Path::from_path(gendir.path()).unwrap();
        let context = setup_test_context(&board_config, gendir_path);
        let storage_config = StorageConfig::default();
        let mut builder: ConfigurationBuilderImpl = Default::default();

        let result = StorageSubsystemConfig::define_configuration(
            &context,
            &(&storage_config, &StorageToolsConfig::default(), &RecoveryConfig::default()),
            &mut builder,
        );
        assert!(result.is_ok());
        let zxcrypt_content = std::fs::read_to_string(gendir_path.join("zxcrypt")).unwrap();
        assert_eq!(zxcrypt_content, "null");
    }

    #[test]
    fn test_keymint_enabled_missing_board_feature_fails() {
        let board_config = BoardConfig {
            filesystems: BoardFilesystemConfig { keymint_enabled: true, ..Default::default() },
            ..Default::default()
        };
        let gendir = tempdir().unwrap();
        let gendir_path = Utf8Path::from_path(gendir.path()).unwrap();
        let context = setup_test_context(&board_config, gendir_path);
        let storage_config = StorageConfig::default();
        let mut builder: ConfigurationBuilderImpl = Default::default();

        let result = StorageSubsystemConfig::define_configuration(
            &context,
            &(&storage_config, &StorageToolsConfig::default(), &RecoveryConfig::default()),
            &mut builder,
        );
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("fuchsia::keymint is not provided by the board")
        );
    }

    #[test]
    fn test_keymint_ramdisk_image_uses_null_encryption() {
        let board_config = BoardConfig {
            provided_features: vec![BoardFeature::Keymint.as_ref().to_string()],
            filesystems: BoardFilesystemConfig { keymint_enabled: true, ..Default::default() },
            ..Default::default()
        };
        let gendir = tempdir().unwrap();
        let gendir_path = Utf8Path::from_path(gendir.path()).unwrap();
        let context = setup_test_context(&board_config, gendir_path);
        let mut storage_config = StorageConfig::default();
        storage_config.filesystems.image_mode = FilesystemImageMode::Ramdisk;
        let mut builder: ConfigurationBuilderImpl = Default::default();

        let result = StorageSubsystemConfig::define_configuration(
            &context,
            &(&storage_config, &StorageToolsConfig::default(), &RecoveryConfig::default()),
            &mut builder,
        );
        assert!(result.is_ok());
        let zxcrypt_content = std::fs::read_to_string(gendir_path.join("zxcrypt")).unwrap();
        assert_eq!(zxcrypt_content, "null");
    }

    #[test]
    fn test_tee_ramdisk_image_uses_null_encryption() {
        let board_config = BoardConfig {
            provided_features: vec![BoardFeature::KeysafeTa.as_ref().to_string()],
            ..Default::default()
        };
        let gendir = tempdir().unwrap();
        let gendir_path = Utf8Path::from_path(gendir.path()).unwrap();
        let context = setup_test_context(&board_config, gendir_path);
        let mut storage_config = StorageConfig::default();
        storage_config.filesystems.image_mode = FilesystemImageMode::Ramdisk;
        let mut builder: ConfigurationBuilderImpl = Default::default();

        let result = StorageSubsystemConfig::define_configuration(
            &context,
            &(&storage_config, &StorageToolsConfig::default(), &RecoveryConfig::default()),
            &mut builder,
        );
        assert!(result.is_ok());
        let zxcrypt_content = std::fs::read_to_string(gendir_path.join("zxcrypt")).unwrap();
        assert_eq!(zxcrypt_content, "null");
    }

    #[test]
    fn test_keymint_utility_feature_set_level_uses_null_encryption() {
        let board_config = BoardConfig {
            provided_features: vec![BoardFeature::Keymint.as_ref().to_string()],
            filesystems: BoardFilesystemConfig { keymint_enabled: true, ..Default::default() },
            ..Default::default()
        };
        let gendir = tempdir().unwrap();
        let gendir_path = Utf8Path::from_path(gendir.path()).unwrap();
        let mut context = setup_test_context(&board_config, gendir_path);
        context.feature_set_level = &FeatureSetLevel::Utility;
        let storage_config = StorageConfig::default();
        let mut builder: ConfigurationBuilderImpl = Default::default();

        let result = StorageSubsystemConfig::define_configuration(
            &context,
            &(&storage_config, &StorageToolsConfig::default(), &RecoveryConfig::default()),
            &mut builder,
        );
        assert!(result.is_ok());
        let zxcrypt_content = std::fs::read_to_string(gendir_path.join("zxcrypt")).unwrap();
        assert_eq!(zxcrypt_content, "null");
    }

    #[test]
    fn test_tee_utility_feature_set_level_uses_null_encryption() {
        let board_config = BoardConfig {
            provided_features: vec![BoardFeature::KeysafeTa.as_ref().to_string()],
            ..Default::default()
        };
        let gendir = tempdir().unwrap();
        let gendir_path = Utf8Path::from_path(gendir.path()).unwrap();
        let mut context = setup_test_context(&board_config, gendir_path);
        context.feature_set_level = &FeatureSetLevel::Utility;
        let storage_config = StorageConfig::default();
        let mut builder: ConfigurationBuilderImpl = Default::default();

        let result = StorageSubsystemConfig::define_configuration(
            &context,
            &(&storage_config, &StorageToolsConfig::default(), &RecoveryConfig::default()),
            &mut builder,
        );
        assert!(result.is_ok());
        let zxcrypt_content = std::fs::read_to_string(gendir_path.join("zxcrypt")).unwrap();
        assert_eq!(zxcrypt_content, "null");
    }
}
