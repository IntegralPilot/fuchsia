// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{board_filesystem_config as bfc, product_filesystem_config as pfc};
use anyhow::{Context, Result, anyhow, bail};
use bfc::{PostProcessingScript, VBMetaDescriptor, ZbiCompression};
use camino::Utf8PathBuf;
use pfc::BlobfsLayout;
use serde::{Deserialize, Serialize};

use std::fmt;
use std::io::Read;
use std::str::FromStr;

/// The configuration file specifying which images to generate and how.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct ImagesConfig {
    /// A list of images to generate.
    #[serde(default)]
    pub images: Vec<Image>,
}

/// An image to generate.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum Image {
    /// A FVM image.
    #[serde(rename = "fvm")]
    Fvm(Fvm),

    /// A ZBI image.
    #[serde(rename = "zbi")]
    Zbi(Zbi),

    /// A VBMeta image.
    #[serde(rename = "vbmeta")]
    VBMeta(VBMeta),

    /// An Fxfs image.
    #[serde(rename = "fxfs")]
    Fxfs(Fxfs),
}

/// Parameters describing how to generate the ZBI.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Zbi {
    /// The name to give the image file.
    pub name: String,

    /// The compression format for the ZBI.
    pub compression: ZbiCompression,

    /// An optional script to post-process the ZBI.
    /// This is often used to prepare the ZBI for flashing/updating.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postprocessing_script: Option<PostProcessingScript>,
}

impl Default for Zbi {
    fn default() -> Self {
        Self {
            name: default_zbi_name(),
            compression: ZbiCompression::default(),
            postprocessing_script: None,
        }
    }
}

fn default_zbi_name() -> String {
    "fuchsia".into()
}

/// The parameters describing how to create a VBMeta image.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct VBMeta {
    /// The logical name of the VBMeta image.
    #[serde(default = "default_vbmeta_name")]
    pub name: String,

    /// The filename to give the image file. If not specified, defaults to `fuchsia`.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,

    /// The style of VBMeta to assemble.
    pub style: bfc::VBMetaStyle,

    /// Path on host to the key for signing VBMeta.
    pub key: Utf8PathBuf,

    /// Path on host to the key metadata to add to the VBMeta.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_metadata: Option<Utf8PathBuf>,

    /// Optional descriptors to add to the VBMeta image.
    #[serde(default)]
    pub additional_descriptors: Vec<VBMetaDescriptor>,

    /// Whether to include the base merkle root command line descriptor.
    #[serde(default)]
    pub include_base_merkle: bool,
}

fn default_vbmeta_name() -> String {
    "fuchsia".into()
}

/// The parameters describing how to create a FVM image.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Fvm {
    /// The size of a slice within the FVM.
    #[serde(default = "default_fvm_slice_size")]
    pub slice_size: u64,

    /// The list of filesystems to generate that can be added to the outputs.
    pub filesystems: Vec<FvmFilesystem>,

    /// The FVM images to generate.
    pub outputs: Vec<FvmOutput>,
}

fn default_fvm_slice_size() -> u64 {
    8388608
}

/// A single FVM filesystem that can be added to multiple outputs.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum FvmFilesystem {
    /// A blobfs volume for holding blobs.
    #[serde(rename = "blobfs")]
    BlobFS(BlobFS),

    /// An empty data partition.
    /// This reserves the data volume, which will be formatted as fxfs/minfs on boot.
    // TODO(https://fxbug.dev/42166000): Remove empty-minfs alias after updating sdk-integration.
    #[serde(rename = "empty-data", alias = "empty-minfs")]
    EmptyData(EmptyData),

    /// Reserved slices in the FVM.
    #[serde(rename = "reserved")]
    Reserved(Reserved),
}

/// Configuration for building a BlobFS volume.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct BlobFS {
    /// The name of the volume in the FVM.
    #[serde(default = "default_blobfs_name")]
    pub name: String,

    /// Optional deprecated layout.
    #[serde(default)]
    pub layout: BlobfsLayout,

    /// Reserve |minimum_data_bytes| and |minimum_inodes| in the FVM, and ensure
    /// that the final reserved size does not exceed |maximum_bytes|.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_bytes: Option<u64>,

    /// Reserve space for at least this many data bytes.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_data_bytes: Option<u64>,

    /// Reserved space for this many inodes.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_inodes: Option<u64>,

    /// Maximum amount of contents for an assembled blobfs.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_contents_size: Option<u64>,
}

fn default_blobfs_name() -> String {
    "blob".into()
}

fn default_data_name() -> String {
    "data".into()
}

/// Configuration for building an EmptyData volume.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct EmptyData {
    /// The name of the volume in the FVM.
    #[serde(default = "default_data_name")]
    pub name: String,
}

/// Configuration for building a Reserved volume.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Reserved {
    /// The name of the volume in the FVM.
    #[serde(default = "default_reserved_name")]
    pub name: String,

    /// The number of slices to reserve.
    pub slices: u64,
}

fn default_reserved_name() -> String {
    "internal".into()
}

impl FromStr for BlobfsLayout {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        blobfs_layout_from_str(s)
    }
}

impl TryFrom<&str> for BlobfsLayout {
    type Error = anyhow::Error;
    fn try_from(s: &str) -> Result<Self> {
        blobfs_layout_from_str(s)
    }
}

impl fmt::Display for BlobfsLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                BlobfsLayout::Compact => "compact",
                BlobfsLayout::DeprecatedPadded => "deprecated_padded",
            }
        )
    }
}

fn blobfs_layout_from_str(s: &str) -> Result<BlobfsLayout> {
    match s {
        "compact" => Ok(BlobfsLayout::Compact),
        "deprecated_padded" => Ok(BlobfsLayout::DeprecatedPadded),
        _ => Err(anyhow!("invalid blobfs layout")),
    }
}

/// A FVM image to generate with a list of filesystems.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum FvmOutput {
    /// The default FVM type with no modifications.
    #[serde(rename = "standard")]
    Standard(StandardFvm),

    /// A FVM that is compressed sparse.
    #[serde(rename = "sparse")]
    Sparse(SparseFvm),

    /// A FVM prepared for a Nand partition.
    #[serde(rename = "nand")]
    Nand(NandFvm),
}

impl std::fmt::Display for FvmOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (fvm_type, name) = match self {
            FvmOutput::Standard(fvm) => ("Standard", &fvm.name),
            FvmOutput::Sparse(fvm) => ("Sparse", &fvm.name),
            FvmOutput::Nand(fvm) => ("Nand", &fvm.name),
        };
        f.write_fmt(format_args!("Fvm::{}(\"{}\")", fvm_type, name))
    }
}

/// The default FVM type with no modifications.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct StandardFvm {
    /// The name to give the file.
    pub name: String,

    /// The filesystems to include in the FVM.
    pub filesystems: Vec<String>,

    /// Whether to compress the FVM.
    #[serde(default)]
    pub compress: bool,

    /// Shrink the FVM to fit exactly the contents.
    #[serde(default)]
    pub resize_image_file_to_fit: bool,

    /// After the optional resize, truncate the file to this length.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncate_to_length: Option<u64>,
}

/// A FVM that is compressed sparse.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SparseFvm {
    /// The name to give the file.
    pub name: String,

    /// The filesystems to include in the FVM.
    pub filesystems: Vec<String>,

    /// The maximum size the FVM can expand to at runtime.
    /// This sets the amount of slice metadata to allocate during construction,
    /// which cannot be modified at runtime.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_disk_size: Option<u64>,
}

/// A FVM prepared for a Nand partition.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct NandFvm {
    /// The name to give the file.
    pub name: String,

    /// The filesystems to include in the FVM.
    #[serde(default)]
    pub filesystems: Vec<String>,

    /// The maximum size the FVM can expand to at runtime.
    /// This sets the amount of slice metadata to allocate during construction,
    /// which cannot be modified at runtime.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_disk_size: Option<u64>,

    /// Whether to compress the FVM.
    #[serde(default)]
    pub compress: bool,

    /// The number of blocks.
    pub block_count: u64,

    /// The out of bound size.
    pub oob_size: u64,

    /// Page size as perceived by the FTL.
    pub page_size: u64,

    /// Number of pages per erase block unit.
    pub pages_per_block: u64,
}

/// The parameters describing how to create an Fxfs image.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct Fxfs {
    /// The size of Fxfs image to generate.  The base system's contents must not exceed this size.
    /// If unset, there's no limit, and the image will be an arbitrary size greater than or equal to
    /// the space needed for the base system's contents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,

    /// Maximum amount of contents for an assembled Fxfs image.  Must be no greater than
    /// `size_bytes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_contents_size: Option<u64>,

    /// How blobs should be formatted in the base fxfs image during assembly.
    #[serde(skip_serializing_if = "crate::is_default")]
    pub blob_format: bfc::FxfsBlobFormat,
}

impl ImagesConfig {
    /// Parse the config from a reader.
    pub fn from_reader<R>(reader: &mut R) -> Result<Self>
    where
        R: Read,
    {
        let mut data = String::default();
        reader.read_to_string(&mut data).context("Cannot read the config")?;
        serde_json5::from_str(&data).context("Cannot parse the config")
    }

    /// Merge a product and board to construct an ImagesConfig.
    pub fn from_product_and_board(
        product: &pfc::ProductFilesystemConfig,
        board: &bfc::BoardFilesystemConfig,
    ) -> Result<Self> {
        let zbi_only = product.image_mode != pfc::FilesystemImageMode::Partition;
        let mut images = vec![];

        // Add the zbi and optional vbmeta specified in the board.
        images.push(Image::Zbi(Zbi {
            name: product.image_name.0.clone(),
            compression: board.zbi.compression.clone(),
            postprocessing_script: board.zbi.postprocessing_script.clone(),
        }));
        if let Some(vbmeta) = &board.vbmeta {
            let bfc::VBMeta {
                name,
                style,
                key,
                key_metadata,
                additional_descriptors,
                include_base_merkle,
            } = vbmeta.clone();
            images.push(Image::VBMeta(VBMeta {
                name: name.unwrap_or_else(|| product.image_name.0.clone()),
                filename: Some(product.image_name.0.clone()),
                style,
                key: key.into(),
                key_metadata: key_metadata.map(Into::into),
                additional_descriptors,
                include_base_merkle,
            }));
        }

        // Add the filesystems specified in the product.
        if product.image_mode != pfc::FilesystemImageMode::NoImage {
            match &product.volume {
                pfc::VolumeConfig::Fxfs => {
                    let size_bytes = board.fxfs.size_bytes;
                    images.push(Image::Fxfs(Fxfs {
                        size_bytes,
                        maximum_contents_size: board.fxfs.size_checker_maximum_bytes,
                        blob_format: board.fxfs.blob_format,
                    }));
                }
                pfc::VolumeConfig::Fvm(fvm) => {
                    let slice_size = board.fvm.slice_size.0;

                    // Construct the list of FVM filesystems.
                    let mut filesystems = vec![];
                    let mut filesystem_names = vec![];
                    filesystems
                        .push(FvmFilesystem::EmptyData(EmptyData { name: "empty-data".into() }));
                    filesystem_names.push("empty-data".to_string());
                    filesystems.push(FvmFilesystem::BlobFS(BlobFS {
                        name: "blob".into(),
                        layout: fvm.blob.blob_layout.clone(),
                        maximum_bytes: board.fvm.blobfs.build_time_maximum_bytes,
                        minimum_data_bytes: board.fvm.blobfs.minimum_data_bytes,
                        minimum_inodes: board.fvm.blobfs.minimum_inodes,
                        maximum_contents_size: board.fvm.blobfs.size_checker_maximum_bytes,
                    }));
                    filesystem_names.push("blob".to_string());
                    if let Some(pfc::ReservedFvmVolumeConfig { reserved_slices }) = fvm.reserved {
                        filesystems.push(FvmFilesystem::Reserved(Reserved {
                            name: "internal".into(),
                            slices: reserved_slices,
                        }));
                        filesystem_names.push("internal".to_string());
                    }
                    // Construct the list of FVM outputs.
                    let mut outputs = vec![];
                    outputs.push(FvmOutput::Standard(StandardFvm {
                        name: "fvm".into(),
                        filesystems: filesystem_names.clone(),
                        compress: false,
                        resize_image_file_to_fit: false,
                        truncate_to_length: board.fvm.truncate_to_length.clone(),
                    }));
                    if !zbi_only {
                        //  Only create these FVM variants if this isn't a zbi-only image.

                        if let Some(sparse) = &board.fvm.sparse_output {
                            outputs.push(FvmOutput::Sparse(SparseFvm {
                                name: "fvm.sparse".into(),
                                filesystems: filesystem_names.clone(),
                                max_disk_size: sparse.max_disk_size.clone(),
                            }));
                        }
                        if board.fvm.fastboot_output.is_some() && board.fvm.nand_output.is_some() {
                            bail!(
                                "A board may only build either a fastboot or nand FVM but not both"
                            );
                        }
                        if let Some(fastboot) = &board.fvm.fastboot_output {
                            outputs.push(FvmOutput::Standard(StandardFvm {
                                name: "fvm.fastboot".into(),
                                filesystems: filesystem_names.clone(),
                                compress: fastboot.compress,
                                resize_image_file_to_fit: true,
                                truncate_to_length: fastboot.truncate_to_length.clone(),
                            }));
                        } else if let Some(nand) = &board.fvm.nand_output {
                            let bfc::NandFvmConfig {
                                max_disk_size,
                                compress,
                                block_count,
                                oob_size,
                                page_size,
                                pages_per_block,
                            } = nand.clone();
                            outputs.push(FvmOutput::Nand(NandFvm {
                                name: "fvm.fastboot".into(),
                                filesystems: filesystem_names.clone(),
                                max_disk_size,
                                compress,
                                block_count,
                                oob_size,
                                page_size,
                                pages_per_block,
                            }));
                        }
                    }

                    // Add the FVM images.
                    images.push(Image::Fvm(Fvm { slice_size, filesystems, outputs }));
                }
            }
        }

        Ok(Self { images })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn test_board_config_fastboot() -> bfc::BoardFilesystemConfig {
        bfc::BoardFilesystemConfig {
            zbi: bfc::Zbi {
                compression: bfc::ZbiCompression::ZStd,
                postprocessing_script: Some(bfc::PostProcessingScript {
                    path: Some("path/to/script".into()),
                    board_script_path: None,
                    args: vec!["arg1".into(), "arg2".into()],
                    inputs: BTreeMap::default(),
                }),
            },
            vbmeta: Some(bfc::VBMeta {
                style: bfc::VBMetaStyle::Fuchsia,
                key: "path/to/key".into(),
                key_metadata: Some("path/to/metadata".into()),
                additional_descriptors: vec![],
                name: Some("fuchsia".into()),
                include_base_merkle: false,
            }),
            fxfs: bfc::Fxfs {
                size_bytes: Some(1234),
                size_checker_maximum_bytes: Some(5678),
                blob_maximum_bytes: None,
                data_maximum_bytes: None,
                blob_format: bfc::FxfsBlobFormat::Zstd,
            },
            fvm: bfc::Fvm {
                slice_size: bfc::FvmSliceSize(5678),
                truncate_to_length: None,
                blobfs: bfc::Blobfs {
                    size_checker_maximum_bytes: Some(12),
                    build_time_maximum_bytes: Some(34),
                    maximum_bytes: None,
                    minimum_inodes: Some(56),
                    minimum_data_bytes: Some(78),
                },
                minfs: bfc::Minfs { maximum_bytes: Some(90) },
                sparse_output: Some(bfc::SparseFvmConfig { max_disk_size: Some(2345) }),
                nand_output: None,
                fastboot_output: Some(bfc::FastbootFvmConfig {
                    compress: true,
                    truncate_to_length: Some(3456),
                }),
            },
            ..Default::default()
        }
    }

    fn test_board_config_nand() -> bfc::BoardFilesystemConfig {
        bfc::BoardFilesystemConfig {
            zbi: bfc::Zbi {
                compression: bfc::ZbiCompression::ZStd,
                postprocessing_script: Some(bfc::PostProcessingScript {
                    path: Some("path/to/script".into()),
                    board_script_path: None,
                    args: vec!["arg1".into(), "arg2".into()],
                    inputs: BTreeMap::default(),
                }),
            },
            vbmeta: Some(bfc::VBMeta {
                style: bfc::VBMetaStyle::Fuchsia,
                key: "path/to/key".into(),
                key_metadata: Some("path/to/metadata".into()),
                additional_descriptors: vec![],
                name: Some("fuchsia".into()),
                include_base_merkle: false,
            }),
            fxfs: bfc::Fxfs {
                size_bytes: Some(1234),
                size_checker_maximum_bytes: Some(5678),
                blob_maximum_bytes: None,
                data_maximum_bytes: None,
                blob_format: bfc::FxfsBlobFormat::Uncompressed,
            },
            fvm: bfc::Fvm {
                slice_size: bfc::FvmSliceSize(5678),
                truncate_to_length: None,
                blobfs: bfc::Blobfs {
                    size_checker_maximum_bytes: Some(12),
                    build_time_maximum_bytes: Some(34),
                    maximum_bytes: None,
                    minimum_inodes: Some(56),
                    minimum_data_bytes: Some(78),
                },
                minfs: bfc::Minfs { maximum_bytes: Some(90) },
                sparse_output: Some(bfc::SparseFvmConfig { max_disk_size: Some(2345) }),
                nand_output: Some(bfc::NandFvmConfig {
                    max_disk_size: Some(3456),
                    compress: true,
                    block_count: 1,
                    oob_size: 2,
                    page_size: 3,
                    pages_per_block: 4,
                }),
                fastboot_output: None,
            },
            gpt: bfc::GptMode::Disabled,
            ..Default::default()
        }
    }

    #[test]
    fn from_product_and_board_no_images() {
        let board = test_board_config_fastboot();
        let product = pfc::ProductFilesystemConfig {
            image_name: pfc::ImageName("a-product".into()),
            watch_for_nand: false,
            format_data_on_corruption: pfc::FormatDataOnCorruption(true),
            no_zxcrypt: false,
            image_mode: pfc::FilesystemImageMode::NoImage,
            volume: pfc::VolumeConfig::Fxfs,
            ..Default::default()
        };

        let images = ImagesConfig::from_product_and_board(&product, &board).unwrap();
        assert_eq!(
            images,
            ImagesConfig {
                images: vec![
                    Image::Zbi(Zbi {
                        name: "a-product".into(),
                        compression: bfc::ZbiCompression::ZStd,
                        postprocessing_script: Some(bfc::PostProcessingScript {
                            path: Some("path/to/script".into()),
                            board_script_path: None,
                            args: vec!["arg1".into(), "arg2".into()],
                            inputs: BTreeMap::default(),
                        }),
                    }),
                    Image::VBMeta(VBMeta {
                        name: "fuchsia".into(),
                        filename: Some("a-product".into()),
                        style: bfc::VBMetaStyle::Fuchsia,
                        key: "path/to/key".into(),
                        key_metadata: Some("path/to/metadata".into()),
                        additional_descriptors: vec![],
                        include_base_merkle: false,
                    }),
                ],
            }
        );
    }

    #[test]
    fn from_product_and_board_fxfs() {
        let board = test_board_config_fastboot();
        let product = pfc::ProductFilesystemConfig {
            image_name: pfc::ImageName("a-product".into()),
            watch_for_nand: false,
            format_data_on_corruption: pfc::FormatDataOnCorruption(true),
            no_zxcrypt: false,
            image_mode: pfc::FilesystemImageMode::Partition,
            volume: pfc::VolumeConfig::Fxfs,
            ..Default::default()
        };

        let images = ImagesConfig::from_product_and_board(&product, &board).unwrap();
        assert_eq!(
            images,
            ImagesConfig {
                images: vec![
                    Image::Zbi(Zbi {
                        name: "a-product".into(),
                        compression: bfc::ZbiCompression::ZStd,
                        postprocessing_script: Some(bfc::PostProcessingScript {
                            path: Some("path/to/script".into()),
                            board_script_path: None,
                            args: vec!["arg1".into(), "arg2".into()],
                            inputs: BTreeMap::default(),
                        }),
                    }),
                    Image::VBMeta(VBMeta {
                        name: "fuchsia".into(),
                        filename: Some("a-product".into()),
                        style: bfc::VBMetaStyle::Fuchsia,
                        key: "path/to/key".into(),
                        key_metadata: Some("path/to/metadata".into()),
                        additional_descriptors: vec![],
                        include_base_merkle: false,
                    }),
                    Image::Fxfs(Fxfs {
                        size_bytes: Some(1234),
                        maximum_contents_size: Some(5678),
                        blob_format: bfc::FxfsBlobFormat::Zstd,
                    }),
                ],
            }
        );
    }

    #[test]
    fn from_product_and_board_fvm_fastboot() {
        let board = test_board_config_fastboot();
        let product = pfc::ProductFilesystemConfig {
            image_name: pfc::ImageName("a-product".into()),
            watch_for_nand: false,
            format_data_on_corruption: pfc::FormatDataOnCorruption(true),
            no_zxcrypt: false,
            image_mode: pfc::FilesystemImageMode::Partition,
            volume: pfc::VolumeConfig::Fvm(pfc::FvmConfig {
                data: pfc::DataFvmVolumeConfig {
                    data_filesystem_format: pfc::DataFilesystemFormat::Minfs,
                },
                blob: pfc::BlobFvmVolumeConfig { blob_layout: BlobfsLayout::Compact },
                reserved: Some(pfc::ReservedFvmVolumeConfig { reserved_slices: 7 }),
            }),
            ..Default::default()
        };

        let images = ImagesConfig::from_product_and_board(&product, &board).unwrap();
        assert_eq!(
            images,
            ImagesConfig {
                images: vec![
                    Image::Zbi(Zbi {
                        name: "a-product".into(),
                        compression: bfc::ZbiCompression::ZStd,
                        postprocessing_script: Some(bfc::PostProcessingScript {
                            path: Some("path/to/script".into()),
                            board_script_path: None,
                            args: vec!["arg1".into(), "arg2".into()],
                            inputs: BTreeMap::default(),
                        }),
                    }),
                    Image::VBMeta(VBMeta {
                        name: "fuchsia".into(),
                        filename: Some("a-product".into()),
                        style: bfc::VBMetaStyle::Fuchsia,
                        key: "path/to/key".into(),
                        key_metadata: Some("path/to/metadata".into()),
                        additional_descriptors: vec![],
                        include_base_merkle: false,
                    }),
                    Image::Fvm(Fvm {
                        slice_size: 5678,
                        filesystems: vec![
                            FvmFilesystem::EmptyData(EmptyData { name: "empty-data".into() }),
                            FvmFilesystem::BlobFS(BlobFS {
                                name: "blob".into(),
                                layout: BlobfsLayout::Compact,
                                maximum_bytes: Some(34),
                                minimum_inodes: Some(56),
                                minimum_data_bytes: Some(78),
                                maximum_contents_size: Some(12),
                            }),
                            FvmFilesystem::Reserved(Reserved {
                                name: "internal".into(),
                                slices: 7,
                            }),
                        ],
                        outputs: vec![
                            FvmOutput::Standard(StandardFvm {
                                name: "fvm".into(),
                                filesystems: vec![
                                    "empty-data".to_string(),
                                    "blob".to_string(),
                                    "internal".to_string(),
                                ],
                                compress: false,
                                resize_image_file_to_fit: false,
                                truncate_to_length: None,
                            }),
                            FvmOutput::Sparse(SparseFvm {
                                name: "fvm.sparse".into(),
                                filesystems: vec![
                                    "empty-data".to_string(),
                                    "blob".to_string(),
                                    "internal".to_string(),
                                ],
                                max_disk_size: Some(2345),
                            }),
                            FvmOutput::Standard(StandardFvm {
                                name: "fvm.fastboot".into(),
                                filesystems: vec![
                                    "empty-data".to_string(),
                                    "blob".to_string(),
                                    "internal".to_string(),
                                ],
                                compress: true,
                                resize_image_file_to_fit: true,
                                truncate_to_length: Some(3456),
                            }),
                        ],
                    }),
                ],
            }
        );
    }

    #[test]
    fn from_product_and_board_fvm_nand() {
        let board = test_board_config_nand();
        let product = pfc::ProductFilesystemConfig {
            image_name: pfc::ImageName("a-product".into()),
            watch_for_nand: false,
            format_data_on_corruption: pfc::FormatDataOnCorruption(true),
            no_zxcrypt: false,
            image_mode: pfc::FilesystemImageMode::Partition,
            volume: pfc::VolumeConfig::Fvm(pfc::FvmConfig {
                data: pfc::DataFvmVolumeConfig {
                    data_filesystem_format: pfc::DataFilesystemFormat::Minfs,
                },
                blob: pfc::BlobFvmVolumeConfig { blob_layout: BlobfsLayout::Compact },
                reserved: Some(pfc::ReservedFvmVolumeConfig { reserved_slices: 7 }),
            }),
            ..Default::default()
        };

        let images = ImagesConfig::from_product_and_board(&product, &board).unwrap();
        assert_eq!(
            images,
            ImagesConfig {
                images: vec![
                    Image::Zbi(Zbi {
                        name: "a-product".into(),
                        compression: bfc::ZbiCompression::ZStd,
                        postprocessing_script: Some(bfc::PostProcessingScript {
                            path: Some("path/to/script".into()),
                            board_script_path: None,
                            args: vec!["arg1".into(), "arg2".into()],
                            inputs: BTreeMap::default(),
                        }),
                    }),
                    Image::VBMeta(VBMeta {
                        name: "fuchsia".into(),
                        filename: Some("a-product".into()),
                        style: bfc::VBMetaStyle::Fuchsia,
                        key: "path/to/key".into(),
                        key_metadata: Some("path/to/metadata".into()),
                        additional_descriptors: vec![],
                        include_base_merkle: false,
                    }),
                    Image::Fvm(Fvm {
                        slice_size: 5678,
                        filesystems: vec![
                            FvmFilesystem::EmptyData(EmptyData { name: "empty-data".into() }),
                            FvmFilesystem::BlobFS(BlobFS {
                                name: "blob".into(),
                                layout: BlobfsLayout::Compact,
                                maximum_bytes: Some(34),
                                minimum_inodes: Some(56),
                                minimum_data_bytes: Some(78),
                                maximum_contents_size: Some(12),
                            }),
                            FvmFilesystem::Reserved(Reserved {
                                name: "internal".into(),
                                slices: 7,
                            }),
                        ],
                        outputs: vec![
                            FvmOutput::Standard(StandardFvm {
                                name: "fvm".into(),
                                filesystems: vec![
                                    "empty-data".to_string(),
                                    "blob".to_string(),
                                    "internal".to_string(),
                                ],
                                compress: false,
                                resize_image_file_to_fit: false,
                                truncate_to_length: None,
                            }),
                            FvmOutput::Sparse(SparseFvm {
                                name: "fvm.sparse".into(),
                                filesystems: vec![
                                    "empty-data".to_string(),
                                    "blob".to_string(),
                                    "internal".to_string(),
                                ],
                                max_disk_size: Some(2345),
                            }),
                            FvmOutput::Nand(NandFvm {
                                name: "fvm.fastboot".into(),
                                filesystems: vec![
                                    "empty-data".to_string(),
                                    "blob".to_string(),
                                    "internal".to_string(),
                                ],
                                max_disk_size: Some(3456),
                                compress: true,
                                block_count: 1,
                                oob_size: 2,
                                page_size: 3,
                                pages_per_block: 4,
                            }),
                        ],
                    }),
                ],
            }
        );
    }

    #[test]
    fn blobfs_layout_try_from() {
        assert_eq!(BlobfsLayout::Compact, "compact".try_into().unwrap());
        assert_eq!(BlobfsLayout::DeprecatedPadded, "deprecated_padded".try_into().unwrap());
        let layout: Result<BlobfsLayout> = "else".try_into();
        assert!(layout.is_err());
    }

    #[test]
    fn blobfs_layout_from_string() {
        assert_eq!(BlobfsLayout::Compact, BlobfsLayout::from_str("compact").unwrap());
        assert_eq!(
            BlobfsLayout::DeprecatedPadded,
            BlobfsLayout::from_str("deprecated_padded").unwrap()
        );
        let layout: Result<BlobfsLayout> = BlobfsLayout::from_str("else");
        assert!(layout.is_err());
    }

    #[test]
    fn blobfs_layout_to_string() {
        assert_eq!("compact".to_string(), BlobfsLayout::Compact.to_string());
        assert_eq!("deprecated_padded".to_string(), BlobfsLayout::DeprecatedPadded.to_string());
    }

    #[test]
    fn from_json() {
        let json = r#"
            {
                images: [
                    {
                        type: "zbi",
                        name: "fuchsia",
                        compression: "zstd.max",
                        postprocessing_script: {
                            path: "path/to/tool.sh",
                            args: [ "arg1", "arg2" ]
                        }
                    },
                    {
                        type: "vbmeta",
                        style: "fuchsia",
                        name: "fuchsia",
                        key: "path/to/key",
                        key_metadata: "path/to/key/metadata",
                        additional_descriptors: [
                            {
                                name: "zircon",
                                size: 12345,
                                flags: 1,
                                min_avb_version: "1.1"
                            }
                        ]
                    },
                    {
                        type: "fvm",
                        slice_size: 0,
                        filesystems: [
                            {
                                type: "blobfs",
                                name: "blob",
                                layout: "compact",
                                maximum_bytes: 0,
                                minimum_data_bytes: 0,
                                minimum_inodes: 0,
                            },
                            {
                                type: "reserved",
                                name: "internal",
                                slices: 0,
                            },
                        ],
                        outputs: [
                            {
                                type: "standard",
                                name: "fvm",
                                filesystems: [
                                    "blob",
                                    "data",
                                    "internal",
                                ],
                                resize_image_file_to_fit: true,
                                truncate_to_length: 0,
                            },
                            {
                                type: "sparse",
                                name: "fvm.sparse",
                                filesystems: [
                                    "blob",
                                    "data",
                                    "internal",
                                ],
                                max_disk_size: 0,
                            },
                            {
                                type: "nand",
                                name: "fvm.nand",
                                filesystems: [
                                    "blob",
                                    "data",
                                    "internal",
                                ],
                                compress: true,
                                max_disk_size: 0,
                                block_count: 0,
                                oob_size: 0,
                                page_size: 0,
                                pages_per_block: 0,
                            },
                        ],
                    },
                ],
            }
        "#;
        let mut cursor = std::io::Cursor::new(json);
        let config: ImagesConfig = ImagesConfig::from_reader(&mut cursor).unwrap();
        assert_eq!(config.images.len(), 3);

        let mut found_zbi = false;
        let mut found_vbmeta = false;
        let mut found_standard_fvm = false;
        let mut found_fxfs = false;
        for image in config.images {
            match image {
                Image::Zbi(zbi) => {
                    found_zbi = true;
                    assert_eq!(zbi.name, "fuchsia");
                    assert!(matches!(zbi.compression, ZbiCompression::ZStdMax));
                }
                Image::VBMeta(vbmeta) => {
                    found_vbmeta = true;
                    assert_eq!(vbmeta.name, "fuchsia");
                    assert_eq!(vbmeta.key, PathBuf::from("path/to/key"));
                }
                Image::Fvm(fvm) => {
                    assert_eq!(fvm.outputs.len(), 3);

                    for output in fvm.outputs {
                        match output {
                            FvmOutput::Standard(standard) => {
                                found_standard_fvm = true;
                                assert_eq!(standard.name, "fvm");
                                assert_eq!(standard.filesystems.len(), 3);
                            }
                            _ => {}
                        }
                    }
                }
                Image::Fxfs(_) => {
                    found_fxfs = true;
                }
            }
        }
        assert!(found_zbi);
        assert!(found_vbmeta);
        assert!(found_standard_fvm);
        assert!(!found_fxfs);
    }

    #[test]
    fn using_defaults() {
        let json = r#"
            {
                images: [
                    {
                        type: "zbi",
                    },
                    {
                        type: "vbmeta",
                        style: "fuchsia",
                        key: "path/to/key",
                        key_metadata: "path/to/key/metadata",
                    },
                    {
                        type: "fvm",
                        filesystems: [
                            {
                                type: "blobfs",
                                name: "blob",
                            },
                            {
                                type: "reserved",
                                name: "internal",
                                slices: 0,
                            },
                        ],
                        outputs: [
                            {
                                type: "standard",
                                name: "fvm.blk",
                                filesystems: [
                                    "blob",
                                    "data",
                                    "internal",
                                ],
                            },
                            {
                                type: "sparse",
                                name: "fvm.sparse.blk",
                                filesystems: [
                                    "blob",
                                    "data",
                                    "internal",
                                ],
                            },
                            {
                                type: "nand",
                                name: "fvm.fastboot.blk",
                                filesystems: [
                                    "blob",
                                    "data",
                                    "internal",
                                ],
                                block_count: 0,
                                oob_size: 0,
                                page_size: 0,
                                pages_per_block: 0,
                            },
                        ],
                    },
                ],
            }
        "#;
        let mut cursor = std::io::Cursor::new(json);
        let config: ImagesConfig = ImagesConfig::from_reader(&mut cursor).unwrap();
        assert_eq!(config.images.len(), 3);
    }
}
