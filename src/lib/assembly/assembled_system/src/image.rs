// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use assembly_container::{FileType, WalkPaths, WalkPathsFn};
use camino::{Utf8Path, Utf8PathBuf};
use fuchsia_pkg::PackageManifest;
use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utf8_path::path_relative_from;

/// An item in the AssembledSystem.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Image {
    /// Base Package.
    BasePackage(Utf8PathBuf),

    /// Zircon Boot Image.
    ZBI {
        /// Path to the ZBI image.
        path: Utf8PathBuf,
        /// Whether the ZBI is signed.
        signed: bool,
    },

    /// Verified Boot Metadata.
    VBMeta(Utf8PathBuf),

    /// System Verified Boot Metadata.
    VBMetaSystem(Utf8PathBuf),

    /// Device Tree Blob Overlay.
    Dtbo(Utf8PathBuf),

    /// BlobFS image.
    BlobFS {
        /// Path to the BlobFS image.
        path: Utf8PathBuf,
        /// Contents metadata.
        contents: BlobfsContents,
    },

    /// Fuchsia Volume Manager.
    FVM(Utf8PathBuf),

    /// Sparse FVM.
    FVMSparse(Utf8PathBuf),

    /// Fastboot FVM.
    FVMFastboot(Utf8PathBuf),

    /// Fxfs.
    Fxfs(Utf8PathBuf),

    /// Fxfs in the Android Sparse format.
    FxfsSparse {
        /// Path to the Fxfs image.
        path: Utf8PathBuf,
        /// Blob contents metadata.
        contents: BlobfsContents,
    },

    /// Qemu Kernel.
    QemuKernel(Utf8PathBuf),

    /// A test-only ramdisk. This is mutually exclusive with ZBI and signals to
    /// tooling that consumes a product bundle to treat this image as an opaque
    /// ramdisk and not to modify it.
    ///
    /// Corresponds to AssemblyMode::TestRamdisk. See variant for more details.
    TestRamdisk(Utf8PathBuf),
}

impl Image {
    /// Get the path of the image on the host.
    pub fn source(&self) -> &Utf8Path {
        match self {
            Image::BasePackage(s) => s.as_path(),
            Image::ZBI { path, signed: _ } => path.as_path(),
            Image::VBMeta(s) => s.as_path(),
            Image::VBMetaSystem(s) => s.as_path(),
            Image::Dtbo(s) => s.as_path(),
            Image::BlobFS { path, .. } => path.as_path(),
            Image::FVM(s) => s.as_path(),
            Image::FVMSparse(s) => s.as_path(),
            Image::FVMFastboot(s) => s.as_path(),
            Image::Fxfs(s) => s.as_path(),
            Image::FxfsSparse { path, .. } => path.as_path(),
            Image::QemuKernel(s) => s.as_path(),
            Image::TestRamdisk(s) => s.as_path(),
        }
    }

    /// Get the mutable path of the image on the host.
    pub fn mut_source(&mut self) -> &mut Utf8PathBuf {
        match self {
            Image::BasePackage(s) => s,
            Image::ZBI { path, signed: _ } => path,
            Image::VBMeta(s) => s,
            Image::VBMetaSystem(s) => s,
            Image::Dtbo(s) => s,
            Image::BlobFS { path, .. } => path,
            Image::FVM(s) => s,
            Image::FVMSparse(s) => s,
            Image::FVMFastboot(s) => s,
            Image::Fxfs(s) => s,
            Image::FxfsSparse { path, .. } => path,
            Image::QemuKernel(s) => s,
            Image::TestRamdisk(s) => s,
        }
    }

    /// Set the path of the image on the host.
    pub fn set_source(&mut self, source: impl Into<Utf8PathBuf>) {
        let source = source.into();
        match self {
            Image::BasePackage(s) => *s = source,
            Image::ZBI { path, signed: _ } => *path = source,
            Image::VBMeta(s) => *s = source,
            Image::VBMetaSystem(s) => *s = source,
            Image::Dtbo(s) => *s = source,
            Image::BlobFS { path, .. } => *path = source,
            Image::FVM(s) => *s = source,
            Image::FVMSparse(s) => *s = source,
            Image::FVMFastboot(s) => *s = source,
            Image::Fxfs(s) => *s = source,
            Image::FxfsSparse { path, .. } => *path = source,
            Image::QemuKernel(s) => *s = source,
            Image::TestRamdisk(s) => *s = source,
        }
    }

    /// For image types that contain blobs, return the contents.
    pub fn get_blobfs_contents(&self) -> Option<&BlobfsContents> {
        match self {
            Image::BlobFS { contents, .. } => Some(contents),
            Image::FxfsSparse { contents, .. } => Some(contents),
            Image::BasePackage(_)
            | Image::ZBI { .. }
            | Image::VBMeta(_)
            | Image::VBMetaSystem(_)
            | Image::Dtbo(_)
            | Image::FVM(_)
            | Image::FVMSparse(_)
            | Image::FVMFastboot(_)
            | Image::Fxfs(_)
            | Image::QemuKernel(_)
            | Image::TestRamdisk(_) => None,
        }
    }

    /// For image types that contain blobs, return the mutable contents.
    pub fn get_mut_blobfs_contents(&mut self) -> Option<&mut BlobfsContents> {
        match self {
            Image::BlobFS { contents, .. } => Some(contents),
            Image::FxfsSparse { contents, .. } => Some(contents),
            Image::BasePackage(_)
            | Image::ZBI { .. }
            | Image::VBMeta(_)
            | Image::VBMetaSystem(_)
            | Image::Dtbo(_)
            | Image::FVM(_)
            | Image::FVMSparse(_)
            | Image::FVMFastboot(_)
            | Image::Fxfs { .. }
            | Image::QemuKernel(_)
            | Image::TestRamdisk(_) => None,
        }
    }
}

impl WalkPaths for Image {
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> Result<()> {
        if let Some(contents) = self.get_mut_blobfs_contents() {
            contents.walk_paths_with_dest(found, dest.join("contents"))?;
        }
        found(self.mut_source(), dest, FileType::Unknown)
    }
}

/// A specific Image type.

#[derive(Debug, Serialize)]
struct ImageSerializeHelper<'a> {
    #[serde(rename = "type")]
    partition_type: &'a str,
    name: &'a str,
    path: &'a Utf8Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    signed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contents: Option<ImageContentsSerializeHelper<'a>>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ImageContentsSerializeHelper<'a> {
    Blobfs(&'a BlobfsContents),
}

impl Serialize for Image {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let helper = match self {
            Image::BasePackage(path) => ImageSerializeHelper {
                partition_type: "far",
                name: "base-package",
                path,
                signed: None,
                contents: None,
            },
            Image::ZBI { path, signed } => ImageSerializeHelper {
                partition_type: "zbi",
                name: "zircon-a",
                path,
                signed: Some(*signed),
                contents: None,
            },
            Image::VBMeta(path) => ImageSerializeHelper {
                partition_type: "vbmeta",
                name: "zircon-a",
                path,
                signed: None,
                contents: None,
            },
            Image::VBMetaSystem(path) => ImageSerializeHelper {
                partition_type: "vbmeta",
                name: "vbmeta_system",
                path,
                signed: None,
                contents: None,
            },
            Image::Dtbo(path) => ImageSerializeHelper {
                partition_type: "dtbo",
                name: "dtbo-a",
                path,
                signed: None,
                contents: None,
            },
            Image::BlobFS { path, contents } => ImageSerializeHelper {
                partition_type: "blk",
                name: "blob",
                path,
                signed: None,
                contents: Some(ImageContentsSerializeHelper::Blobfs(contents)),
            },
            Image::FVM(path) => ImageSerializeHelper {
                partition_type: "blk",
                name: "storage-full",
                path,
                signed: None,
                contents: None,
            },
            Image::FVMSparse(path) => ImageSerializeHelper {
                partition_type: "blk",
                name: "storage-sparse",
                path,
                signed: None,
                contents: None,
            },
            Image::FVMFastboot(path) => ImageSerializeHelper {
                partition_type: "blk",
                name: "fvm.fastboot",
                path,
                signed: None,
                contents: None,
            },
            Image::Fxfs(path) => ImageSerializeHelper {
                partition_type: "fxfs-blk",
                name: "storage-full",
                path,
                signed: None,
                contents: None,
            },
            Image::FxfsSparse { path, contents } => ImageSerializeHelper {
                partition_type: "blk",
                name: "fxfs.fastboot",
                path,
                signed: None,
                contents: Some(ImageContentsSerializeHelper::Blobfs(contents)),
            },
            Image::QemuKernel(path) => ImageSerializeHelper {
                partition_type: "kernel",
                name: "qemu-kernel",
                path,
                signed: None,
                contents: None,
            },
            Image::TestRamdisk(path) => ImageSerializeHelper {
                partition_type: "test-ramdisk",
                name: "test-ramdisk",
                path,
                signed: None,
                contents: None,
            },
        };
        helper.serialize(serializer)
    }
}

/// Detailed metadata on the contents of a particular image output.
#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, WalkPaths)]
pub struct BlobfsContents {
    /// Information about packages included in the image.
    #[walk_paths]
    pub packages: PackagesMetadata,
    /// Maximum total size of all the blobs stored in this image.
    pub maximum_contents_size: Option<u64>,
}

impl BlobfsContents {
    /// Add base package info into BlobfsContents
    pub fn add_base_package(
        &mut self,
        path: impl AsRef<Utf8Path>,
        merkle_size_map: &HashMap<String, u64>,
    ) -> anyhow::Result<()> {
        Self::add_package(&mut self.packages.base, path, merkle_size_map)?;
        Ok(())
    }

    /// Add cache package info into BlobfsContents
    pub fn add_cache_package(
        &mut self,
        path: impl AsRef<Utf8Path>,
        merkle_size_map: &HashMap<String, u64>,
    ) -> anyhow::Result<()> {
        Self::add_package(&mut self.packages.cache, path, merkle_size_map)?;
        Ok(())
    }

    fn add_package_blobs(
        subpackage_name: Option<String>,
        package_manifest: PackageManifest,
        package_blobs: &mut Vec<PackageBlob>,
        merkle_size_map: &HashMap<String, u64>,
    ) -> anyhow::Result<()> {
        let (blobs, subpackages) = package_manifest.into_blobs_and_subpackages();
        for blob in blobs {
            let path = if let Some(subpackage) = &subpackage_name {
                format!("{}/{}", subpackage, blob.path)
            } else {
                blob.path.to_string()
            };
            package_blobs.push(PackageBlob {
                merkle: blob.merkle.to_string(),
                path,
                used_space_in_blobfs: *merkle_size_map.get(&blob.merkle.to_string()).with_context(
                    || format!("Blob merkle not found. {} {}", blob.path, blob.merkle),
                )?,
            });
        }
        for subpackage in subpackages {
            let subpackage_name = if let Some(parent_subpackage) = &subpackage_name {
                format!("{}/{}", parent_subpackage, subpackage.name)
            } else {
                subpackage.name
            };
            Self::add_package_blobs(
                Some(subpackage_name),
                PackageManifest::try_load_from(subpackage.manifest_path)?,
                package_blobs,
                merkle_size_map,
            )?;
        }
        Ok(())
    }

    fn add_package(
        package_set: &mut PackageSetMetadata,
        path: impl AsRef<Utf8Path>,
        merkle_size_map: &HashMap<String, u64>,
    ) -> anyhow::Result<()> {
        let manifest = path.as_ref().to_owned();
        let package_manifest = PackageManifest::try_load_from(&manifest)?;
        let name = package_manifest.name().to_string();
        let abi_revision = package_manifest.abi_revision();
        let mut package_blobs: Vec<PackageBlob> = vec![];
        Self::add_package_blobs(None, package_manifest, &mut package_blobs, merkle_size_map)
            .with_context(|| format!("adding package: {manifest}"))?;
        package_blobs.sort();
        package_set.metadata.push(PackageMetadata {
            name,
            manifest,
            abi_revision,
            blobs: package_blobs,
        });
        Ok(())
    }

    /// Relativize all manifest locations in the BlobFsContents to the output file's location
    pub fn relativize(mut self, base_path: impl AsRef<Utf8Path>) -> Result<BlobfsContents> {
        // Modify in-place so we can add fields to the struct without updating this code
        for package in self.packages.base.metadata.iter_mut() {
            package.manifest = path_relative_from(&package.manifest, &base_path)?
        }
        for package in self.packages.cache.metadata.iter_mut() {
            package.manifest = path_relative_from(&package.manifest, &base_path)?
        }
        Ok(self)
    }
}

/// Metadata on packages included in a given image.
#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, WalkPaths)]
pub struct PackagesMetadata {
    /// Paths to package manifests for the base package set.
    #[walk_paths]
    pub base: PackageSetMetadata,
    /// Paths to package manifests for the cache package set.
    #[walk_paths]
    pub cache: PackageSetMetadata,
}

/// Metadata for a certain package set (e.g. base or cache).
#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, WalkPaths)]
#[serde(transparent)]
pub struct PackageSetMetadata {
    /// Inner set of metadata.
    #[walk_paths]
    pub metadata: Vec<PackageMetadata>,
}

/// Metadata on a single package included in a given image.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct PackageMetadata {
    /// The package's name.
    pub name: String,
    /// Path to the package's manifest.
    pub manifest: Utf8PathBuf,
    /// ABI revision stamped into the package.
    pub abi_revision: Option<version_history::AbiRevision>,
    /// List of blobs in this package.
    pub blobs: Vec<PackageBlob>,
}

impl WalkPaths for PackageMetadata {
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> Result<()> {
        found(&mut self.manifest, dest, FileType::PackageManifest)
    }
}

#[derive(Clone, Debug, Ord, PartialOrd, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct PackageBlob {
    // Merkle hash of this blob
    pub merkle: String,
    // Path of blob in package
    pub path: String,
    // Space used by this blob in blobfs
    pub used_space_in_blobfs: u64,
}

#[derive(Debug, Deserialize)]
struct ImageDeserializeHelper {
    #[serde(rename = "type")]
    partition_type: String,
    name: String,
    path: Utf8PathBuf,
    signed: Option<bool>,
    contents: Option<ImageContentsDeserializeHelper>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ImageContentsDeserializeHelper {
    Blobfs(BlobfsContents),
}

impl<'de> Deserialize<'de> for Image {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Due to historical reasons, there are images where "name" has come to
        // parameterize the image type itself. Outside of those cases, the name
        // is just a human-readable bit of metadata identifying a given
        // instance of that image and so we disregard such incoming values.
        // These images are renamed with with their canonical Assembly names
        // (e.g., "zircon-a") at serialization-time.
        let helper = ImageDeserializeHelper::deserialize(deserializer)?;
        match (&helper.partition_type[..], &helper.name[..], &helper.signed) {
            ("far", "base-package", None) => Ok(Image::BasePackage(helper.path)),
            ("zbi", _, signed) => {
                Ok(Image::ZBI { path: helper.path, signed: signed.is_some_and(|s| s) })
            }
            ("vbmeta", "vbmeta_system", None) => Ok(Image::VBMetaSystem(helper.path)),
            ("vbmeta", _, None) => Ok(Image::VBMeta(helper.path)),
            ("dtbo", _, None) => Ok(Image::Dtbo(helper.path)),
            ("blk", "blob", None) => {
                if let Some(contents) = helper.contents {
                    let ImageContentsDeserializeHelper::Blobfs(contents) = contents;
                    Ok(Image::BlobFS { path: helper.path, contents })
                } else {
                    Err(de::Error::missing_field("contents"))
                }
            }
            ("blk", "storage-full", None) => Ok(Image::FVM(helper.path)),
            ("fxfs-blk", "storage-full", None) => Ok(Image::Fxfs(helper.path)),
            ("blk", "fxfs.fastboot", None) => {
                if let Some(contents) = helper.contents {
                    let ImageContentsDeserializeHelper::Blobfs(contents) = contents;
                    Ok(Image::FxfsSparse { path: helper.path, contents })
                } else {
                    Err(de::Error::missing_field("contents"))
                }
            }
            ("blk", "storage-sparse", None) => Ok(Image::FVMSparse(helper.path)),
            ("blk", "fvm.fastboot", None) => Ok(Image::FVMFastboot(helper.path)),
            ("kernel", _, None) => Ok(Image::QemuKernel(helper.path)),
            ("test-ramdisk", _, None) => Ok(Image::TestRamdisk(helper.path)),
            (partition_type, name, _) => Err(de::Error::unknown_variant(
                &format!("({partition_type}, {name})"),
                &[
                    "(far, base-package)",
                    "(zbi, _)",
                    "(vbmeta, _)",
                    "(blk, blob)",
                    "(blk, storage-full)",
                    "(blk, storage-sparse)",
                    "(blk, fvm.fastboot)",
                    "(kernel, _)",
                    "(test-ramdisk, _)",
                ],
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::io::Write;
    use tempfile::{NamedTempFile, tempdir};

    #[test]
    fn image_source() {
        let mut image = Image::FVM("path/to/fvm.blk".into());
        assert_eq!(image.source(), &Utf8PathBuf::from("path/to/fvm.blk"));
        image.set_source("path/to/fvm2.blk");
        assert_eq!(image.source(), &Utf8PathBuf::from("path/to/fvm2.blk"));
    }

    #[test]
    fn serialize() {
        let images = vec![
            Image::BasePackage("path/to/base.far".into()),
            Image::ZBI { path: "path/to/fuchsia.zbi".into(), signed: true },
            Image::VBMeta("path/to/fuchsia.vbmeta".into()),
            Image::Dtbo("path/to/dtbo".into()),
            Image::BlobFS { path: "path/to/blob.blk".into(), contents: Default::default() },
            Image::FVM("path/to/fvm.blk".into()),
            Image::FVMSparse("path/to/fvm.sparse.blk".into()),
            Image::FVMFastboot("path/to/fvm.fastboot.blk".into()),
            Image::QemuKernel("path/to/qemu/kernel".into()),
            Image::TestRamdisk("path/to/test/ramdisk".into()),
        ];
        assert_eq!(generate_test_value(), serde_json::to_value(images).unwrap());
    }

    #[test]
    fn serialize_fxfs() {
        let images = vec![
            Image::BasePackage("path/to/base.far".into()),
            Image::ZBI { path: "path/to/fuchsia.zbi".into(), signed: true },
            Image::VBMeta("path/to/fuchsia.vbmeta".into()),
            Image::Dtbo("path/to/dtbo".into()),
            Image::Fxfs("path/to/fxfs.blk".into()),
            Image::FxfsSparse {
                path: "path/to/fxfs.sparse.blk".into(),
                contents: Default::default(),
            },
            Image::QemuKernel("path/to/qemu/kernel".into()),
            Image::TestRamdisk("path/to/test/ramdisk".into()),
        ];

        assert_eq!(generate_test_value_fxfs(), serde_json::to_value(images).unwrap());
    }

    #[test]
    fn serialize_unsigned_zbi() {
        let images = vec![Image::ZBI { path: "path/to/fuchsia.zbi".into(), signed: false }];

        let value = json!([
            {
                "type": "zbi",
                "name": "zircon-a",
                "path": "path/to/fuchsia.zbi",
                "signed": false,
            }
        ]);
        assert_eq!(value, serde_json::to_value(images).unwrap());
    }

    #[test]
    fn deserialize() {
        let images: Vec<Image> = serde_json::from_value(generate_test_value()).unwrap();
        assert_eq!(images.len(), 10);

        for image in &images {
            let (expected, actual) = match image {
                Image::BasePackage(path) => ("path/to/base.far", path),
                Image::ZBI { path, signed } => {
                    assert!(signed);
                    ("path/to/fuchsia.zbi", path)
                }
                Image::VBMeta(path) => ("path/to/fuchsia.vbmeta", path),
                Image::Dtbo(path) => ("path/to/dtbo", path),
                Image::BlobFS { path, contents } => {
                    assert_eq!(contents, &BlobfsContents::default());
                    ("path/to/blob.blk", path)
                }
                Image::FVM(path) => ("path/to/fvm.blk", path),
                Image::FVMSparse(path) => ("path/to/fvm.sparse.blk", path),
                Image::FVMFastboot(path) => ("path/to/fvm.fastboot.blk", path),
                Image::QemuKernel(path) => ("path/to/qemu/kernel", path),
                Image::TestRamdisk(path) => ("path/to/test/ramdisk", path),
                _ => panic!("Unexpected item {image:?}"),
            };
            assert_eq!(&Utf8PathBuf::from(expected), actual);
        }
    }

    #[test]
    fn deserialize_fxfs() {
        let images: Vec<Image> = serde_json::from_value(generate_test_value_fxfs()).unwrap();
        assert_eq!(images.len(), 8);

        for image in &images {
            let (expected, actual) = match image {
                Image::BasePackage(path) => ("path/to/base.far", path),
                Image::ZBI { path, signed } => {
                    assert!(signed);
                    ("path/to/fuchsia.zbi", path)
                }
                Image::VBMeta(path) => ("path/to/fuchsia.vbmeta", path),
                Image::Dtbo(path) => ("path/to/dtbo", path),
                Image::Fxfs(path) => ("path/to/fxfs.blk", path),
                Image::FxfsSparse { path, contents } => {
                    assert_eq!(contents, &BlobfsContents::default());
                    ("path/to/fxfs.sparse.blk", path)
                }
                Image::QemuKernel(path) => ("path/to/qemu/kernel", path),
                Image::TestRamdisk(path) => ("path/to/test/ramdisk", path),
                _ => panic!("Unexpected item {image:?}"),
            };
            assert_eq!(&Utf8PathBuf::from(expected), actual);
        }
    }

    #[test]
    fn deserialize_invalid() {
        let invalid = json!([
            {
                "type": "far-invalid",
                "name": "base-package",
                "path": "path/to/base.far",
            },
        ]);
        let result: Result<Vec<Image>, _> = serde_json::from_value(invalid);
        assert!(result.unwrap_err().is_data());
    }

    #[test]
    fn deserialize_zbi_with_arbitrary_name() {
        let value = json!([
            {
                "type": "zbi",
                "name": "my-zbi",
                "path": "path/to/my.zbi",
            }
        ]);

        let expected = vec![Image::ZBI { path: "path/to/my.zbi".into(), signed: false }];
        let result: Result<Vec<Image>, _> = serde_json::from_value(value);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected,);
    }

    #[test]
    fn deserialize_vbmeta_with_arbitrary_name() {
        let value = json!([
            {
                "type": "vbmeta",
                "name": "my-vbmeta",
                "path": "path/to/my.vbmeta",
            }
        ]);

        let expected = vec![Image::VBMeta("path/to/my.vbmeta".into())];
        let result: Result<Vec<Image>, _> = serde_json::from_value(value);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected,);
    }

    #[test]
    fn deserialize_qemu_kernel_with_arbitrary_name() {
        let value = json!([
            {
                "type": "kernel",
                "name": "my-qemu-kernel",
                "path": "path/to/my-qemu-kernel.bin",
            }
        ]);

        let expected = vec![Image::QemuKernel("path/to/my-qemu-kernel.bin".into())];
        let result: Result<Vec<Image>, _> = serde_json::from_value(value);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected,);
    }

    #[test]
    fn deserialize_test_ramdisk_with_arbitrary_name() {
        let value = json!([
            {
                "type": "test-ramdisk",
                "name": "my-test-ramdisk",
                "path": "path/to/my-test-ramdisk.bin",
            }
        ]);

        let expected = vec![Image::TestRamdisk("path/to/my-test-ramdisk.bin".into())];
        let result: Result<Vec<Image>, _> = serde_json::from_value(value);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected,);
    }

    #[test]
    fn test_blobfs_contents_add_base_or_cache_package() -> anyhow::Result<()> {
        let content = generate_test_package_manifest_content();
        let super_content =
            generate_test_super_package_manifest_content("package_manifest_temp_file.json");

        let mut package_manifest_temp_file = NamedTempFile::new()?;
        let mut super_package_manifest_temp_file = NamedTempFile::new()?;
        let dir = tempdir().unwrap();
        let root = Utf8Path::from_path(dir.path()).unwrap();
        write!(package_manifest_temp_file, "{content}")?;
        write!(super_package_manifest_temp_file, "{super_content}")?;
        let path = package_manifest_temp_file.into_temp_path();
        path.persist(dir.path().join("package_manifest_temp_file.json"))?;
        let path = super_package_manifest_temp_file.into_temp_path();
        path.persist(dir.path().join("super_package_manifest_temp_file.json"))?;

        let mut contents = BlobfsContents::default();
        let mut merkle_size_map: HashMap<String, u64> = HashMap::new();
        merkle_size_map.insert(
            "7ddff816740d5803358dd4478d8437585e8d5c984b4361817d891807a16ff581".to_string(),
            10,
        );
        merkle_size_map.insert(
            "8cb3466c6e66592c8decaeaa3e399652fbe71dad5c3df1a5e919743a33815567".to_string(),
            20,
        );
        merkle_size_map.insert(
            "eabdb84d26416c1821fd8972e0d835eedaf7468e5a9ebe01e5944462411aec70".to_string(),
            30,
        );
        contents
            .add_base_package(root.join("package_manifest_temp_file.json"), &merkle_size_map)?;
        contents.add_base_package(
            root.join("super_package_manifest_temp_file.json"),
            &merkle_size_map,
        )?;
        contents
            .add_cache_package(root.join("package_manifest_temp_file.json"), &merkle_size_map)?;
        let actual_package_blobs_base = &(contents.packages.base.metadata[0].blobs);
        let actual_package_blobs_super_base = &(contents.packages.base.metadata[1].blobs);
        let actual_package_blobs_cache = &(contents.packages.cache.metadata[0].blobs);

        let package_blob1 = PackageBlob {
            merkle: "7ddff816740d5803358dd4478d8437585e8d5c984b4361817d891807a16ff581".to_string(),
            path: "bin/def".to_string(),
            used_space_in_blobfs: 10,
        };

        let package_blob2 = PackageBlob {
            merkle: "8cb3466c6e66592c8decaeaa3e399652fbe71dad5c3df1a5e919743a33815567".to_string(),
            path: "lib/ghi".to_string(),
            used_space_in_blobfs: 20,
        };

        let package_blob3 = PackageBlob {
            merkle: "eabdb84d26416c1821fd8972e0d835eedaf7468e5a9ebe01e5944462411aec70".to_string(),
            path: "abc/".to_string(),
            used_space_in_blobfs: 30,
        };

        let super_package_blob1 = PackageBlob {
            merkle: "7ddff816740d5803358dd4478d8437585e8d5c984b4361817d891807a16ff581".to_string(),
            path: "my-subpackage/bin/def".to_string(),
            used_space_in_blobfs: 10,
        };

        let super_package_blob2 = PackageBlob {
            merkle: "8cb3466c6e66592c8decaeaa3e399652fbe71dad5c3df1a5e919743a33815567".to_string(),
            path: "my-subpackage/lib/ghi".to_string(),
            used_space_in_blobfs: 20,
        };

        let super_package_blob3 = PackageBlob {
            merkle: "eabdb84d26416c1821fd8972e0d835eedaf7468e5a9ebe01e5944462411aec70".to_string(),
            path: "my-subpackage/abc/".to_string(),
            used_space_in_blobfs: 30,
        };

        let expected_blobs = vec![package_blob1, package_blob2, package_blob3];
        let expected_super_blobs =
            vec![super_package_blob1, super_package_blob2, super_package_blob3];
        // Verify if all 3 blobs are available and also if they are sorted.
        assert_eq!(actual_package_blobs_base, &expected_blobs);
        assert_eq!(actual_package_blobs_cache, &expected_blobs);
        assert_eq!(actual_package_blobs_super_base, &expected_super_blobs);

        Ok(())
    }

    fn generate_test_package_manifest_content() -> String {
        let content = r#"{
            "package": {
                "name": "test_package",
                "version": "0"
            },
            "blobs": [
                {
                    "path": "abc/",
                    "merkle": "eabdb84d26416c1821fd8972e0d835eedaf7468e5a9ebe01e5944462411aec70",
                    "size": 2048,
                    "source_path": "../../blobs/eabdb84d26416c1821fd8972e0d835eedaf7468e5a9ebe01e5944462411aec78"
                },
                {
                    "path": "bin/def",
                    "merkle": "7ddff816740d5803358dd4478d8437585e8d5c984b4361817d891807a16ff581",
                    "size": 188416,
                    "source_path": "../../blobs/7ddff816740d5803358dd4478d8437585e8d5c984b4361817d891807a16ff581"
                },
                {
                    "path": "lib/ghi",
                    "merkle": "8cb3466c6e66592c8decaeaa3e399652fbe71dad5c3df1a5e919743a33815567",
                    "size": 692224,
                    "source_path": "../../blobs/8cb3466c6e66592c8decaeaa3e399652fbe71dad5c3df1a5e919743a33815567"
                }
            ],
            "version": "1",
            "blob_sources_relative": "file",
            "repository": "fuchsia.com"
        }
        "#;
        content.to_string()
    }

    fn generate_test_super_package_manifest_content(subpackage_manifest_path: &str) -> String {
        format!(
            r#"{{
            "package": {{
                "name": "super_package",
                "version": "0"
            }},
            "blobs": [],
            "subpackages": [
                {{
                    "name": "my-subpackage",
                    "merkle": "eabdb84d26416c1821fd8972e0d835eedaf7468e5a9ebe01e5944462411aec70",
                    "manifest_path": "{subpackage_manifest_path}"
                }}
            ],
            "version": "1",
            "blob_sources_relative": "file",
            "repository": "fuchsia.com"
        }}
        "#
        )
    }

    fn generate_test_value() -> Value {
        json!([
                {
                    "type": "far",
                    "name": "base-package",
                    "path": "path/to/base.far",
                },
                {
                    "type": "zbi",
                    "name": "zircon-a",
                    "path": "path/to/fuchsia.zbi",
                    "signed": true,
                },
                {
                    "type": "vbmeta",
                    "name": "zircon-a",
                    "path": "path/to/fuchsia.vbmeta",
                },
                {
                    "type": "dtbo",
                    "name": "dtbo-a",
                    "path": "path/to/dtbo",
                },
                {
                    "type": "blk",
                    "name": "blob",
                    "path": "path/to/blob.blk",
                    "contents": {
                        "packages": {
                            "base": [],
                            "cache": [],
                        },
                        "maximum_contents_size": None::<u64>
                    },
                },
                {
                    "type": "blk",
                    "name": "storage-full",
                    "path": "path/to/fvm.blk",
                },
                {
                    "type": "blk",
                    "name": "storage-sparse",
                    "path": "path/to/fvm.sparse.blk",
                },
                {
                    "type": "blk",
                    "name": "fvm.fastboot",
                    "path": "path/to/fvm.fastboot.blk",
                },
                {
                    "type": "kernel",
                    "name": "qemu-kernel",
                    "path": "path/to/qemu/kernel",
                },
                {
                    "type": "test-ramdisk",
                    "name": "test-ramdisk",
                    "path": "path/to/test/ramdisk",
                },
        ])
    }

    fn generate_test_value_fxfs() -> Value {
        json!([
            {
                "type": "far",
                "name": "base-package",
                "path": "path/to/base.far",
            },
            {
                "type": "zbi",
                "name": "zircon-a",
                "path": "path/to/fuchsia.zbi",
                "signed": true,
            },
            {
                "type": "vbmeta",
                "name": "zircon-a",
                "path": "path/to/fuchsia.vbmeta",
            },
            {
                "type": "dtbo",
                "name": "dtbo-a",
                "path": "path/to/dtbo",
            },
            {
                "type": "fxfs-blk",
                "name": "storage-full",
                "path": "path/to/fxfs.blk",
            },
            {
                "type": "blk",
                "name": "fxfs.fastboot",
                "path": "path/to/fxfs.sparse.blk",
                "contents": {
                    "packages": {
                        "base": [],
                        "cache": [],
                    },
                    "maximum_contents_size": None::<u64>
                },
            },
            {
                "type": "kernel",
                "name": "qemu-kernel",
                "path": "path/to/qemu/kernel",
            },
            {
                    "type": "test-ramdisk",
                    "name": "test-ramdisk",
                    "path": "path/to/test/ramdisk",
                },
        ])
    }
}
