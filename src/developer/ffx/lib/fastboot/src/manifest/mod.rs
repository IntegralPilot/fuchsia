// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::{Boot, Flash, Unlock};
use crate::error::FfxFastbootError;
use crate::file_resolver::FileResolver;
use crate::file_resolver::resolvers::Resolver;
use crate::manifest::resolvers::{
    ArchiveResolver, FlashManifestResolver, FlashManifestTarResolver, ManifestResolver,
};
use crate::util::Event;

type Result<T> = std::result::Result<T, FfxFastbootError>;
use assembly_partitions_config::UploadMethod;
use async_trait::async_trait;
use camino::Utf8Path;
use ffx_config::EnvironmentContext;
use ffx_fastboot_interface::fastboot_interface::FastbootInterface;
use ffx_flash_manifest::{BootParams, Command, FlashManifestVersion, ManifestParams};
use pbms::load_product_bundle;
use product_bundle::ProductBundle;
use serde::Deserialize;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use tokio::sync::mpsc::Sender;

pub mod resolvers;
pub mod v1;
pub mod v2;
pub mod v3;
pub mod v4;

#[derive(Default, Deserialize)]
pub struct Image {
    pub name: String,
    pub path: String,
    // Ignore the rest of the fields
}

#[async_trait]
impl Flash for FlashManifestVersion {
    async fn flash<F, T>(
        &self,
        messenger: &Sender<Event>,
        file_resolver: &mut F,
        fastboot_interface: &mut T,
        cmd: ManifestParams,
        ssh_key_upload_method: Option<&UploadMethod>,
    ) -> Result<()>
    where
        F: FileResolver + Sync + Send,
        T: FastbootInterface,
    {
        match self {
            Self::V1(v) => {
                v.flash(messenger, file_resolver, fastboot_interface, cmd, ssh_key_upload_method)
                    .await?
            }
            Self::V2(v) => {
                v.flash(messenger, file_resolver, fastboot_interface, cmd, ssh_key_upload_method)
                    .await?
            }
            Self::V3(v) => {
                v.flash(messenger, file_resolver, fastboot_interface, cmd, ssh_key_upload_method)
                    .await?
            }
            Self::V4(v) => {
                v.flash(messenger, file_resolver, fastboot_interface, cmd, ssh_key_upload_method)
                    .await?
            }
        };
        Ok(())
    }
}

#[async_trait]
impl Unlock for FlashManifestVersion {
    async fn unlock<F, T>(
        &self,
        messenger: &Sender<Event>,
        file_resolver: &mut F,
        fastboot_interface: &mut T,
    ) -> Result<()>
    where
        F: FileResolver + Sync + Send,
        T: FastbootInterface,
    {
        match self {
            Self::V1(v) => v.unlock(messenger, file_resolver, fastboot_interface).await?,
            Self::V2(v) => v.unlock(messenger, file_resolver, fastboot_interface).await?,
            Self::V3(v) => v.unlock(messenger, file_resolver, fastboot_interface).await?,
            Self::V4(v) => v.unlock(messenger, file_resolver, fastboot_interface).await?,
        };
        Ok(())
    }
}

#[async_trait]
impl Boot for FlashManifestVersion {
    async fn boot<F, T>(
        &self,
        messenger: Sender<Event>,
        file_resolver: &mut F,
        slot: String,
        fastboot_interface: &mut T,
        cmd: ManifestParams,
    ) -> Result<()>
    where
        F: FileResolver + Sync + Send,
        T: FastbootInterface,
    {
        match self {
            Self::V1(v) => v.boot(messenger, file_resolver, slot, fastboot_interface, cmd).await?,
            Self::V2(v) => v.boot(messenger, file_resolver, slot, fastboot_interface, cmd).await?,
            Self::V3(v) => v.boot(messenger, file_resolver, slot, fastboot_interface, cmd).await?,
            Self::V4(v) => v.boot(messenger, file_resolver, slot, fastboot_interface, cmd).await?,
        };
        Ok(())
    }
}

pub async fn from_sdk<F: FastbootInterface>(
    context: &EnvironmentContext,
    messenger: &Sender<Event>,
    fastboot_interface: &mut F,
    cmd: ManifestParams,
) -> Result<()> {
    log::debug!("fastboot manifest from_sdk");
    match cmd.product_bundle.as_ref() {
        Some(b) => {
            let product_bundle = load_product_bundle(context, b).await?.into();
            FlashManifest {
                resolver: Resolver::new(PathBuf::from(b))?,
                version: FlashManifestVersion::from_product_bundle(&product_bundle)?,
            }
            .flash(messenger, fastboot_interface, cmd)
            .await
        }
        None => Err(FfxFastbootError::ProductBundleRequired),
    }
}

pub async fn from_local_product_bundle<F: FastbootInterface>(
    messenger: &Sender<Event>,
    path: PathBuf,
    fastboot_interface: &mut F,
    cmd: ManifestParams,
) -> Result<()> {
    log::debug!("fastboot manifest from_local_product_bundle");
    let path = Utf8Path::from_path(&*path).ok_or(FfxFastbootError::NonUtf8Path)?;

    match (path.is_file(), path.extension()) {
        (true, Some("zip")) => {
            // This is an awkward hack we've had to introduce thanks to
            // dtbo partition verifying in our image assembly.
            // When we load the product bundle `try_load_from` we call the
            // ImageMapper's verify functions, this requires the files to be
            // on disk to SHA them.
            let temp_dir = tempfile::tempdir()?;
            let tdir_path = temp_dir.path().to_owned();
            let file = File::open(path)?;
            let mut archive =
                zip::read::ZipArchive::new(file).map_err(FfxFastbootError::ZipArchiveOpen)?;
            for i in 0..archive.len() {
                let mut file = archive.by_index(i).map_err(FfxFastbootError::ZipArchiveRead)?;
                if file.is_file() {
                    let ofile_path = tdir_path.join(file.mangled_name());
                    if let Some(parent) = ofile_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let mut o_file = File::create(ofile_path)?;
                    std::io::copy(&mut file, &mut o_file)?;
                }
            }
            let tdir_path =
                Utf8Path::from_path(&*tdir_path).ok_or(FfxFastbootError::NonUtf8Path)?;
            let pb_path = if std::fs::exists(tdir_path.join("product_bundle"))? {
                tdir_path.join("product_bundle")
            } else {
                tdir_path.to_owned()
            };
            let product_bundle = ProductBundle::try_load_from(&pb_path)
                .map_err(FfxFastbootError::ProductBundleLoad)?;
            let flash_manifest_version =
                FlashManifestVersion::from_product_bundle(&product_bundle)?;
            FlashManifest {
                resolver: Resolver::new(tdir_path.into())?,
                version: flash_manifest_version,
            }
            .flash(messenger, fastboot_interface, cmd)
            .await
        }
        (true, extension) => Err(FfxFastbootError::UnsupportedProductBundleExtension {
            extension: extension.unwrap_or_default().to_string(),
        }),
        (false, _) => {
            let product_bundle =
                ProductBundle::try_load_from(path).map_err(FfxFastbootError::ProductBundleLoad)?;
            let flash_manifest_version =
                FlashManifestVersion::from_product_bundle(&product_bundle)?;
            FlashManifest { resolver: Resolver::new(path.into())?, version: flash_manifest_version }
                .flash(messenger, fastboot_interface, cmd)
                .await
        }
    }
}

pub async fn from_in_tree<T: FastbootInterface>(
    context: &EnvironmentContext,
    messenger: &Sender<Event>,
    fastboot_interface: &mut T,
    cmd: ManifestParams,
) -> Result<()> {
    log::debug!("fastboot manifest from_in_tree");
    if cmd.product_bundle.is_some() {
        log::debug!("in tree, but product bundle specified, use in-tree sdk");
        from_sdk(context, messenger, fastboot_interface, cmd).await
    } else {
        Err(FfxFastbootError::ManifestOrProductBundleRequired)
    }
}

pub async fn from_path<T: FastbootInterface>(
    messenger: &Sender<Event>,
    path: PathBuf,
    fastboot_interface: &mut T,
    cmd: ManifestParams,
) -> Result<()> {
    log::debug!("fastboot manifest from_path");
    let ext = path.extension().and_then(|e| e.to_str());
    let is_tar_gz = path.to_string_lossy().ends_with(".tar.gz") || ext == Some("tgz");
    let is_tar = ext == Some("tar");
    let is_zip = ext == Some("zip");

    if is_zip {
        let r = ArchiveResolver::new(path)?;
        load_flash_manifest(r).await?.flash(messenger, fastboot_interface, cmd).await
    } else if is_tar_gz || is_tar {
        let r = FlashManifestTarResolver::new(path)?;
        load_flash_manifest(r).await?.flash(messenger, fastboot_interface, cmd).await
    } else {
        let r = FlashManifestResolver::new(path)?;
        load_flash_manifest(r).await?.flash(messenger, fastboot_interface, cmd).await
    }
}

async fn load_flash_manifest<F: ManifestResolver + FileResolver + Sync>(
    resolver: F,
) -> Result<FlashManifest<impl FileResolver + Sync>> {
    let reader = File::open(resolver.get_manifest_path().await).map(BufReader::new)?;
    Ok(FlashManifest { resolver, version: FlashManifestVersion::load(reader)? })
}

pub struct FlashManifest<F: FileResolver + Sync> {
    resolver: F,
    version: FlashManifestVersion,
}

impl<F: FileResolver + Sync + Send> FlashManifest<F> {
    pub async fn flash<T: FastbootInterface>(
        &mut self,
        messenger: &Sender<Event>,
        fastboot_interface: &mut T,
        cmd: ManifestParams,
    ) -> Result<()> {
        match &cmd.op {
            Command::Flash => {
                self.version
                    .flash(messenger, &mut self.resolver, fastboot_interface, cmd, None)
                    .await
            }
            Command::Unlock(_) => {
                // Using the manifest, don't need the unlock credential from the UnlockCommand
                // here.
                self.version.unlock(messenger, &mut self.resolver, fastboot_interface).await
            }
            Command::Boot(BootParams { slot, .. }) => {
                self.version
                    .boot(
                        messenger.clone(),
                        &mut self.resolver,
                        slot.to_owned(),
                        fastboot_interface,
                        cmd,
                    )
                    .await
            }
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    type Result<T> = std::result::Result<T, anyhow::Error>;
    use assembly_partitions_config::{BootloaderPartition, BootstrapPartition, PartitionsConfig};
    use camino::Utf8PathBuf;
    use ffx_flash_manifest::ManifestFile;
    use ffx_flash_manifest::v3::{FlashManifest as FlashManifestV3, Partition};
    use product_bundle::ProductBundleV2;
    use serde_json::from_str;

    const UNKNOWN_VERSION: &'static str = r#"{
        "version": 99999,
        "manifest": "test"
    }"#;

    const MANIFEST: &'static str = r#"{
        "version": 1,
        "manifest": []
    }"#;

    const ARRAY_MANIFEST: &'static str = r#"[{
        "version": 1,
        "manifest": []
    }]"#;

    #[test]
    fn test_deserialization() -> Result<()> {
        let _manifest: ManifestFile = from_str(MANIFEST)?;
        Ok(())
    }

    #[test]
    fn test_serialization() -> Result<()> {
        let manifest = FlashManifestVersion::V3(FlashManifestV3 {
            hw_revision: "board".into(),
            product_matches: vec![],
            credentials: vec![],
            products: vec![],
        });
        let mut buf = Vec::new();
        manifest.write(&mut buf).unwrap();
        let str = String::from_utf8(buf).unwrap();
        assert_eq!(
            str,
            r#"{
  "manifest": {
    "hw_revision": "board"
  },
  "version": 3
}"#
        );
        Ok(())
    }

    #[test]
    fn test_serialization_v4() -> Result<()> {
        use assembly_partitions_config::UploadMethod;
        use ffx_flash_manifest::v4::FlashManifest as FlashManifestV4;
        let manifest = FlashManifestVersion::V4(FlashManifestV4 {
            v3: FlashManifestV3 {
                hw_revision: "board".into(),
                product_matches: vec![],
                credentials: vec![],
                products: vec![],
            },
            ssh_key_upload_method: Some(UploadMethod::Staged { command: "test".to_string() }),
        });
        let mut buf = Vec::new();
        manifest.write(&mut buf).unwrap();
        let str = String::from_utf8(buf).unwrap();
        assert_eq!(
            str,
            r#"{
  "manifest": {
    "hw_revision": "board",
    "ssh_key_upload_method": {
      "type": "staged",
      "command": "test"
    }
  },
  "version": 4
}"#
        );
        Ok(())
    }

    #[test]
    fn test_loading_unknown_version() {
        let manifest_contents = UNKNOWN_VERSION.to_string();
        let result = FlashManifestVersion::load(BufReader::new(manifest_contents.as_bytes()));
        assert!(result.is_err());
    }

    #[fuchsia::test]
    async fn test_loading_version_1() -> Result<()> {
        let manifest_contents = MANIFEST.to_string();
        FlashManifestVersion::load(BufReader::new(manifest_contents.as_bytes()))?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_loading_version_1_from_array() -> Result<()> {
        let manifest_contents = ARRAY_MANIFEST.to_string();
        FlashManifestVersion::load(BufReader::new(manifest_contents.as_bytes()))?;
        Ok(())
    }

    #[test]
    fn test_from_product_bundle_bootstrap_partitions() {
        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: String::default(),
            product_version: String::default(),
            partitions: PartitionsConfig {
                bootstrap_partitions: vec![BootstrapPartition {
                    name: "bootstrap_part".into(),
                    condition: None,
                    image: Utf8PathBuf::from("bootstrap_image"),
                }],
                bootloader_partitions: vec![BootloaderPartition {
                    name: Some("bootloader_part".into()),
                    image: Utf8PathBuf::from("bootloader_image"),
                    partition_type: "".into(),
                }],
                partitions: vec![],
                hardware_revision: String::default(),
                product_matches: vec![],
                unlock_credentials: vec![],
                ssh_key_upload_method: None,
            },
            sdk_version: String::default(),
            system_a: None,
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });
        let manifest = match FlashManifestVersion::from_product_bundle(&pb).unwrap() {
            FlashManifestVersion::V3(manifest) => manifest,
            _ => panic!("Expected a V3 FlashManifest"),
        };
        let bootstrap_product = manifest.products.iter().find(|&p| p.name == "bootstrap").unwrap();
        // The important piece here is that the bootstrap partition comes first.
        assert_eq!(
            bootstrap_product.bootloader_partitions,
            vec![
                Partition {
                    name: "bootstrap_part".into(),
                    path: "bootstrap_image".into(),
                    condition: None
                },
                Partition {
                    name: "bootloader_part".into(),
                    path: "bootloader_image".into(),
                    condition: None
                },
            ]
        )
    }

    #[test]
    fn test_from_product_bundle_v4() {
        use assembly_partitions_config::UploadMethod;
        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: String::default(),
            product_version: String::default(),
            partitions: PartitionsConfig {
                ssh_key_upload_method: Some(UploadMethod::Staged {
                    command: "oem custom-ssh".into(),
                }),
                ..Default::default()
            },
            sdk_version: String::default(),
            system_a: None,
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });

        let manifest = match FlashManifestVersion::from_product_bundle(&pb).unwrap() {
            FlashManifestVersion::V4(manifest) => manifest,
            _ => panic!("Expected a V4 FlashManifest"),
        };
        assert_eq!(
            manifest.ssh_key_upload_method,
            Some(UploadMethod::Staged { command: "oem custom-ssh".into() })
        );
    }

    #[fuchsia::test]
    async fn test_from_local_product_bundle_path_traversal() {
        use std::io::Write;
        use zip::CompressionMethod;
        use zip::write::SimpleFileOptions as FileOptions;

        let tmp_dir = tempfile::tempdir().unwrap();
        let zip_path = tmp_dir.path().join("traversal.zip");

        let file = File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = FileOptions::default().compression_method(CompressionMethod::Stored);

        let target_file_name = "ffx_fastboot_manifest_test_traversal_target.txt";
        let traversal_path = format!("../{}", target_file_name);

        zip.start_file(traversal_path, options).unwrap();
        zip.write_all(b"malicious content").unwrap();
        zip.finish().unwrap();

        let messenger = tokio::sync::mpsc::channel(1).0;
        let (_state, mut interface) = ffx_fastboot_interface::test::setup();
        let cmd = ManifestParams::default();

        let result = from_local_product_bundle(&messenger, zip_path, &mut interface, cmd).await;

        // This needs to be an error as there is no product_bundle.json
        assert!(result.is_err());

        let escaped_path = std::env::temp_dir().join(target_file_name);
        let escaped_exists = escaped_path.exists();
        if escaped_exists {
            std::fs::remove_file(&escaped_path).unwrap();
        }
        assert!(!escaped_exists, "File escaped to {}", escaped_path.display());
    }
}
