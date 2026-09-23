// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{Container, Filesystem, FilesystemLauncher};
use crate::device::constants::{DATA_TYPE_GUID, DEFAULT_F2FS_MIN_BYTES};
use anyhow::{Context as _, Error};
use async_trait::async_trait;
use crypt_policy::get_policy;
use fidl_fuchsia_fs_startup::{CheckOptions, CreateOptions, MountOptions};
use fidl_fuchsia_fvm::ResetMarker;
use fs_management::filesystem::{ServingMultiVolumeFilesystem, ServingVolume};
use fs_management::format::DiskFormat;
use fs_management::format::constants::{
    BLOBFS_PARTITION_LABEL, DATA_PARTITION_LABEL, LEGACY_DATA_PARTITION_LABEL,
};
use fuchsia_component::client::connect_to_protocol_at_dir_root;
use zxcrypt_crypt::with_crypt_service;

pub struct FvmContainer(ServingMultiVolumeFilesystem, bool);

impl FvmContainer {
    pub fn new(fs: ServingMultiVolumeFilesystem, is_ramdisk: bool) -> Self {
        Self(fs, is_ramdisk)
    }
}

#[async_trait]
impl Container for FvmContainer {
    fn fs(&mut self) -> &mut ServingMultiVolumeFilesystem {
        &mut self.0
    }

    fn into_fs(self: Box<Self>) -> ServingMultiVolumeFilesystem {
        self.0
    }

    fn blobfs_volume_label(&self) -> &'static str {
        BLOBFS_PARTITION_LABEL
    }

    async fn serve_data(&mut self, launcher: &FilesystemLauncher) -> Result<Filesystem, Error> {
        fn find_data_volume(volumes: Vec<String>) -> Option<String> {
            let mut found_blobfs = false;
            let mut data_label = None;
            for volume in volumes {
                match volume.as_str() {
                    BLOBFS_PARTITION_LABEL if !found_blobfs => found_blobfs = true,
                    DATA_PARTITION_LABEL | LEGACY_DATA_PARTITION_LABEL if data_label.is_none() => {
                        data_label = Some(volume.clone());
                    }
                    // Don't do anything if we find unexpected volumes.
                    _ => (),
                }
            }
            if found_blobfs && data_label.is_some() { data_label } else { None }
        }

        if let Some(data_label) = find_data_volume(self.get_volumes().await?) {
            match self.check_and_mount_data(&data_label, launcher).await {
                Ok(data_volume) => {
                    return Ok(Filesystem::ServingVolumeInMultiVolume(None, data_volume));
                }
                Err(error) => match error.root_cause().downcast_ref::<zx::Status>() {
                    Some(status) if status == &zx::Status::WRONG_TYPE => {
                        // Assume that it's more likely that this is because of FDR, or after
                        // flashing rather than due to corruption, so avoid the crash report.
                        log::info!("Data volume unexpected type. Reformatting.");
                    }
                    _ => {
                        let format = &launcher.config.data_filesystem_format;
                        launcher.report_corruption(format, &error);
                        log::error!(
                            error:?;
                            "Unable to mount {format} data volume. Reformatting the data volumes."
                        );
                    }
                },
            };
        }

        self.format_data(launcher).await
    }

    async fn format_data(&mut self, launcher: &FilesystemLauncher) -> Result<Filesystem, Error> {
        self.remove_all_non_blob_volumes().await?;

        let uri = format!("#meta/{}.cm", launcher.config.data_filesystem_format);
        let initial_size =
            matches!(launcher.config.data_filesystem_format.as_str().into(), DiskFormat::F2fs)
                .then(|| {
                    std::cmp::max(launcher.config.data_max_bytes, DEFAULT_F2FS_MIN_BYTES)
                        .next_multiple_of(launcher.config.fvm_slice_size)
                });
        let create_volume = |crypt| async {
            self.0
                .create_volume(
                    DATA_PARTITION_LABEL,
                    CreateOptions {
                        initial_size,
                        type_guid: Some(DATA_TYPE_GUID),
                        ..CreateOptions::default()
                    },
                    MountOptions { crypt, uri: Some(uri), ..Default::default() },
                )
                .await
                .map(|data_volume| Filesystem::ServingVolumeInMultiVolume(None, data_volume))
        };

        if self.data_requires_zxcrypt(launcher) {
            let policy = get_policy().await?;
            with_crypt_service(policy, |crypt| create_volume(Some(crypt))).await
        } else {
            create_volume(None).await
        }
    }

    async fn shred_data(&mut self) -> Result<(), Error> {
        let reset_proxy = connect_to_protocol_at_dir_root::<ResetMarker>(self.0.exposed_dir())?;
        reset_proxy
            .shred_encrypted_volumes()
            .await
            .context("shred encrypted volumes fidl failure")?
            .map_err(zx::Status::err_from_raw)
            .context("shred encrypted volumes returned error")?;
        Ok(())
    }

    fn data_requires_zxcrypt(&self, launcher: &FilesystemLauncher) -> bool {
        launcher.requires_zxcrypt(launcher.config.data_filesystem_format.as_str().into(), self.1)
    }
}

impl FvmContainer {
    async fn check_and_mount_data(
        &mut self,
        data_label: &str,
        launcher: &FilesystemLauncher,
    ) -> Result<ServingVolume, Error> {
        let format = launcher.config.data_filesystem_format.as_str();
        let uri = format!("#meta/{format}.cm");
        if launcher.requires_zxcrypt(launcher.config.data_filesystem_format.as_str().into(), self.1)
        {
            let policy = get_policy().await?;
            if launcher.config.check_filesystems {
                with_crypt_service(policy, |crypt| {
                    self.0.check_volume(
                        data_label,
                        CheckOptions {
                            uri: Some(uri.clone()),
                            crypt: Some(crypt),
                            ..Default::default()
                        },
                    )
                })
                .await?;
            }
            with_crypt_service(policy, |crypt| {
                self.0.open_volume(
                    data_label,
                    MountOptions {
                        uri: Some(uri.clone()),
                        crypt: Some(crypt),
                        ..MountOptions::default()
                    },
                )
            })
            .await
        } else {
            if launcher.config.check_filesystems {
                self.0
                    .check_volume(
                        data_label,
                        CheckOptions { uri: Some(uri.clone()), ..Default::default() },
                    )
                    .await?;
            }
            self.0
                .open_volume(
                    data_label,
                    MountOptions { uri: Some(uri), crypt: None, ..MountOptions::default() },
                )
                .await
        }
    }
}
