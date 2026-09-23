// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Partition, PartitionsConfig, RecoveryStyle, Slot};
use anyhow::{Context, Result, bail};
use assembled_system::Image;
use assembly_util::write_json_file;
use camino::Utf8PathBuf;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use url::Url;

/// The type of the image used to correlate a partition with an image in the
/// assembly manifest.
///
/// This supports two recovery styles:
///
/// - R-slotted recovery
///     Slot::R
///     ImageType::ZBI
///
/// - A/B-slotted recovery
///     Slot::A | Slot::B
///     ImageType::RecoveryZBI
///
#[derive(Debug, PartialOrd, Ord, PartialEq, Eq)]
pub enum ImageType {
    /// Zircon Boot Image.
    ZBI,
    /// Recovery Zircon Boot Image.
    /// This is used for recoveries that have A/B slots.
    /// R-slotted recoveries should use ImageType::ZBI with Slot::R.
    RecoveryZBI,
    /// Verified Boot Metadata.
    VBMeta,
    /// Recovery Verified Boot Metadata.
    /// This is used for recoveries that have A/B slots.
    /// R-slotted recoveries should use ImageType::VBMeta with Slot::R.
    RecoveryVBMeta,
    /// Fuchsia Volume Manager.
    FVM,
    /// Fuchsia Filesystem.
    Fxfs,
    /// Device Tree Blob Overlay.
    Dtbo,
}

impl fmt::Display for ImageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ZBI => "ZBI",
            Self::RecoveryZBI => "RecoveryZBI",
            Self::VBMeta => "VBMeta",
            Self::RecoveryVBMeta => "RecoveryVBMeta",
            Self::FVM => "FVM",
            Self::Fxfs => "Fxfs",
            Self::Dtbo => "Dtbo",
        };
        write!(f, "{}", message)
    }
}

/// A pair of an image path mapped to a specific partition.
#[derive(Debug, PartialOrd, Ord, PartialEq, Eq)]
pub struct PartitionAndImage {
    /// The partition on hardware.
    pub partition: Partition,
    /// The path to the image to place in the partition.
    pub path: Utf8PathBuf,
}

/// A tool that can map sets of images into hardware partitions.
pub struct PartitionImageMapper {
    partitions: PartitionsConfig,
    images: BTreeMap<Slot, BTreeMap<ImageType, Utf8PathBuf>>,
    recovery_style: RecoveryStyle,
}

impl PartitionImageMapper {
    /// Construct a new mapper that targets the `partitions`.
    pub fn new(partitions: PartitionsConfig) -> Result<Self> {
        let recovery_style = partitions.recovery_style()?;
        Ok(Self { partitions, images: BTreeMap::new(), recovery_style })
    }

    /// Map a set images that are intended for a specific slot to partitions.
    pub fn map_images_to_slot(&mut self, images: &Vec<Image>, slot: Slot) -> Result<()> {
        log::debug!("Mapping images: {:#?} to slot: {}", images, slot);

        // AB-slotted recoveries get redirected to slot A.
        let slot_entry = if slot == Slot::R && self.recovery_style == RecoveryStyle::AB {
            self.images.entry(Slot::A).or_insert(BTreeMap::new())
        } else {
            self.images.entry(slot).or_insert(BTreeMap::new())
        };

        for image in images.iter() {
            match image {
                Image::ZBI { path, .. } => {
                    let image_type = if slot == Slot::R && self.recovery_style == RecoveryStyle::AB
                    {
                        ImageType::RecoveryZBI
                    } else {
                        ImageType::ZBI
                    };
                    log::debug!("Adding image type: {} from path {}", image_type, path.clone());
                    slot_entry.insert(image_type, path.clone());
                }
                Image::VBMeta(path) => {
                    let image_type = if slot == Slot::R && self.recovery_style == RecoveryStyle::AB
                    {
                        ImageType::RecoveryVBMeta
                    } else {
                        ImageType::VBMeta
                    };
                    log::debug!("Adding image type: {} from path {}", image_type, path.clone());
                    slot_entry.insert(image_type, path.clone());
                }
                Image::VBMetaSystem(path) => {
                    if let Slot::R = slot {
                        // Recovery should not include a system vbmeta.
                        log::debug!(
                            "Skipping image at path: {} as recovery should not include a system vbmeta",
                            path.clone()
                        );
                        continue;
                    } else {
                        let image_type = ImageType::VBMeta;
                        log::debug!("Adding image type: {} from path {}", image_type, path.clone());
                        slot_entry.insert(image_type, path.clone());
                    }
                }
                Image::FVMFastboot(path) => {
                    if let Slot::R = slot {
                        // Recovery should not include a separate FVM, because it is embedded into the
                        // ZBI as a ramdisk.
                        log::debug!(
                            "Skipping image at path: {} as recovery should not include a separate FVM",
                            path.clone()
                        );
                        continue;
                    } else {
                        let image_type = ImageType::FVM;
                        log::debug!("Adding image type: {} from path {}", image_type, path.clone());
                        slot_entry.insert(image_type, path.clone());
                    }
                }
                Image::FxfsSparse { path, .. } => {
                    if let Slot::R = slot {
                        // Recovery should not include a separate FVM, because it is embedded into the
                        // ZBI as a ramdisk.
                        log::debug!(
                            "Skipping image at path: {} as recovery should not include a separate FVM",
                            path.clone()
                        );
                        continue;
                    } else {
                        let image_type = ImageType::Fxfs;
                        log::debug!("Adding image type: {} from path {}", image_type, path.clone());
                        slot_entry.insert(image_type, path.clone());
                    }
                }
                Image::Dtbo(path) => {
                    let image_type = ImageType::Dtbo;
                    log::debug!("Adding image type: {} from path {}", image_type, path.clone());

                    // If multiple assemblies are adding the DTBO to the same partition, ensure
                    // that the contents of the image are equal.
                    if let Some(existing) = slot_entry.insert(image_type, path.clone()) {
                        let existing_digest = try_digest(&existing)
                            .with_context(|| format!("Hashing existing dtbo: {}", existing))?;
                        let new_digest = try_digest(path)
                            .with_context(|| format!("Hashing new dtbo: {}", path))?;
                        if existing_digest != new_digest {
                            bail!(
                                "Two different dtbo images were mapped to the same partition\nprevious: {}\nnew: {}",
                                existing,
                                path
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Return the mappings of images to partitions.
    pub fn map(&self) -> Vec<PartitionAndImage> {
        let mut mapped_partitions = vec![];

        // Assign the images to particular partitions.
        for p in &self.partitions.partitions {
            let (image_type, slot) = match &p {
                Partition::ZBI { slot, .. } => (ImageType::ZBI, slot),
                Partition::RecoveryZBI { slot, .. } => (ImageType::RecoveryZBI, slot),
                Partition::VBMeta { slot, .. } => (ImageType::VBMeta, slot),
                Partition::RecoveryVBMeta { slot, .. } => (ImageType::RecoveryVBMeta, slot),
                Partition::Dtbo { slot, .. } => (ImageType::Dtbo, slot),

                // Arbitrarily, take the fvm from the slot A system.
                Partition::FVM { .. } => (ImageType::FVM, &Slot::A),

                // Arbitrarily, take Fxfs from the slot A system.
                Partition::Fxfs { .. } => (ImageType::Fxfs, &Slot::A),
            };

            if let Some(slot) = self.images.get(slot) {
                if let Some(path) = slot.get(&image_type) {
                    mapped_partitions
                        .push(PartitionAndImage { partition: p.clone(), path: path.clone() });
                }
            }
        }

        mapped_partitions
    }

    /// Generate a size report that indicates the partition size and the size of the image mapped
    /// to it.
    pub fn generate_gerrit_size_report(
        &self,
        report_path: &Utf8PathBuf,
        prefix: &String,
    ) -> Result<()> {
        let mut report = BTreeMap::new();

        let mappings = self.map();
        for mapping in mappings {
            let PartitionAndImage { partition, path } = mapping;
            if let (Some(size), name) = (partition.size(), partition.name()) {
                let metadata = std::fs::metadata(&path)
                    .with_context(|| format!("Getting image metadata: {}", path))?;
                let measured_size = metadata.len();
                report.insert(format!("{}-{}", prefix, name), json!(measured_size));
                report.insert(format!("{}-{}.budget", prefix, name), json!(size));
                report.insert(format!("{}-{}.creepBudget", prefix, name), json!(200 * 1024));
                let url = Url::parse_with_params(
                    "http://go/fuchsia-size-stats/single_component/",
                    &[("f", format!("component:in:{}-{}", prefix, name))],
                )?;
                report.insert(format!("{}-{}.owner", prefix, name), json!(url.as_str()));
            }
        }

        write_json_file(&report_path, &report).context("Writing gerrit size report")
    }
}

fn try_digest(path: &Utf8PathBuf) -> Result<Vec<u8>> {
    // TODO(https://fxbug.dev/535596550): Find a better API making a custom Write impl.
    struct WriteHasher<'a, D>(&'a mut D);
    impl<D: sha2::Digest> std::io::Write for WriteHasher<'_, D> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.update(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut hasher = Sha256::new();
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    std::io::copy(&mut reader, &mut WriteHasher(&mut hasher))
        .with_context(|| format!("Hashing file: {}", path))?;
    Ok(hasher.finalize().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembled_system::{AssembledSystem, BlobfsContents};
    use assembly_release_info::SystemReleaseInfo;
    use assembly_util::read_config;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[test]
    fn test_map_fvm() {
        let partitions = PartitionsConfig {
            partitions: vec![
                Partition::ZBI { name: "zbi_a".into(), slot: Slot::A, size: None },
                Partition::VBMeta { name: "vbmeta_a".into(), slot: Slot::A, size: None },
                Partition::Dtbo { name: "dtbo_a".into(), slot: Slot::A, size: None },
                Partition::ZBI { name: "zbi_b".into(), slot: Slot::B, size: None },
                Partition::VBMeta { name: "vbmeta_b".into(), slot: Slot::B, size: None },
                Partition::Dtbo { name: "dtbo_b".into(), slot: Slot::B, size: None },
                Partition::ZBI { name: "zbi_r".into(), slot: Slot::R, size: None },
                Partition::VBMeta { name: "vbmeta_r".into(), slot: Slot::R, size: None },
                Partition::FVM { name: "fvm".into(), size: None },
                Partition::Fxfs { name: "fxfs".into(), size: None },
            ],
            ..Default::default()
        };
        let images_a = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/a/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/a/fuchsia.vbmeta".into()),
                Image::FVM("path/to/a/fvm.blk".into()),
                Image::FVMFastboot("path/to/a/fvm.fastboot.blk".into()),
                Image::Dtbo("path/to/a/dtbo".into()),
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let images_b = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/b/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/b/fuchsia.vbmeta".into()),
                Image::FVM("path/to/b/fvm.blk".into()),
                Image::FVMFastboot("path/to/b/fvm.fastboot.blk".into()),
                Image::Dtbo("path/to/b/dtbo".into()),
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let images_r = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/r/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/r/fuchsia.vbmeta".into()),
                Image::FVM("path/to/r/fvm.blk".into()),
                Image::FVMFastboot("path/to/r/fvm.fastboot.blk".into()),
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let mut mapper = PartitionImageMapper::new(partitions).unwrap();
        mapper.map_images_to_slot(&images_a.images, Slot::A).unwrap();
        mapper.map_images_to_slot(&images_b.images, Slot::B).unwrap();
        mapper.map_images_to_slot(&images_r.images, Slot::R).unwrap();

        let expected = vec![
            PartitionAndImage {
                partition: Partition::ZBI { name: "zbi_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/fuchsia.zbi".into(),
            },
            PartitionAndImage {
                partition: Partition::VBMeta { name: "vbmeta_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/fuchsia.vbmeta".into(),
            },
            PartitionAndImage {
                partition: Partition::Dtbo { name: "dtbo_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/dtbo".into(),
            },
            PartitionAndImage {
                partition: Partition::ZBI { name: "zbi_b".into(), slot: Slot::B, size: None },
                path: "path/to/b/fuchsia.zbi".into(),
            },
            PartitionAndImage {
                partition: Partition::VBMeta { name: "vbmeta_b".into(), slot: Slot::B, size: None },
                path: "path/to/b/fuchsia.vbmeta".into(),
            },
            PartitionAndImage {
                partition: Partition::Dtbo { name: "dtbo_b".into(), slot: Slot::B, size: None },
                path: "path/to/b/dtbo".into(),
            },
            PartitionAndImage {
                partition: Partition::ZBI { name: "zbi_r".into(), slot: Slot::R, size: None },
                path: "path/to/r/fuchsia.zbi".into(),
            },
            PartitionAndImage {
                partition: Partition::VBMeta { name: "vbmeta_r".into(), slot: Slot::R, size: None },
                path: "path/to/r/fuchsia.vbmeta".into(),
            },
            PartitionAndImage {
                partition: Partition::FVM { name: "fvm".into(), size: None },
                path: "path/to/a/fvm.fastboot.blk".into(),
            },
        ];
        assert_eq!(expected, mapper.map());
    }

    #[test]
    fn test_map_fxfs() {
        let partitions = PartitionsConfig {
            partitions: vec![
                Partition::ZBI { name: "zbi_a".into(), slot: Slot::A, size: None },
                Partition::VBMeta { name: "vbmeta_a".into(), slot: Slot::A, size: None },
                Partition::Dtbo { name: "dtbo_a".into(), slot: Slot::A, size: None },
                Partition::ZBI { name: "zbi_b".into(), slot: Slot::B, size: None },
                Partition::VBMeta { name: "vbmeta_b".into(), slot: Slot::B, size: None },
                Partition::Dtbo { name: "dtbo_b".into(), slot: Slot::B, size: None },
                Partition::ZBI { name: "zbi_r".into(), slot: Slot::R, size: None },
                Partition::VBMeta { name: "vbmeta_r".into(), slot: Slot::R, size: None },
                Partition::FVM { name: "fvm".into(), size: None },
                Partition::Fxfs { name: "fxfs".into(), size: None },
            ],
            ..Default::default()
        };
        let images_a = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/a/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/a/fuchsia.vbmeta".into()),
                Image::Dtbo("path/to/a/dtbo".into()),
                Image::FxfsSparse {
                    path: "path/to/a/fxfs.blk".into(),
                    contents: BlobfsContents::default(),
                },
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let images_b = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/b/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/b/fuchsia.vbmeta".into()),
                Image::Dtbo("path/to/b/dtbo".into()),
                Image::FxfsSparse {
                    path: "path/to/b/fxfs.blk".into(),
                    contents: BlobfsContents::default(),
                },
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let images_r = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/r/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/r/fuchsia.vbmeta".into()),
                Image::FxfsSparse {
                    path: "path/to/r/fxfs.blk".into(),
                    contents: BlobfsContents::default(),
                },
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let mut mapper = PartitionImageMapper::new(partitions).unwrap();
        mapper.map_images_to_slot(&images_a.images, Slot::A).unwrap();
        mapper.map_images_to_slot(&images_b.images, Slot::B).unwrap();
        mapper.map_images_to_slot(&images_r.images, Slot::R).unwrap();

        let expected = vec![
            PartitionAndImage {
                partition: Partition::ZBI { name: "zbi_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/fuchsia.zbi".into(),
            },
            PartitionAndImage {
                partition: Partition::VBMeta { name: "vbmeta_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/fuchsia.vbmeta".into(),
            },
            PartitionAndImage {
                partition: Partition::Dtbo { name: "dtbo_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/dtbo".into(),
            },
            PartitionAndImage {
                partition: Partition::ZBI { name: "zbi_b".into(), slot: Slot::B, size: None },
                path: "path/to/b/fuchsia.zbi".into(),
            },
            PartitionAndImage {
                partition: Partition::VBMeta { name: "vbmeta_b".into(), slot: Slot::B, size: None },
                path: "path/to/b/fuchsia.vbmeta".into(),
            },
            PartitionAndImage {
                partition: Partition::Dtbo { name: "dtbo_b".into(), slot: Slot::B, size: None },
                path: "path/to/b/dtbo".into(),
            },
            PartitionAndImage {
                partition: Partition::ZBI { name: "zbi_r".into(), slot: Slot::R, size: None },
                path: "path/to/r/fuchsia.zbi".into(),
            },
            PartitionAndImage {
                partition: Partition::VBMeta { name: "vbmeta_r".into(), slot: Slot::R, size: None },
                path: "path/to/r/fuchsia.vbmeta".into(),
            },
            PartitionAndImage {
                partition: Partition::Fxfs { name: "fxfs".into(), size: None },
                path: "path/to/a/fxfs.blk".into(),
            },
        ];
        assert_eq!(expected, mapper.map());
    }

    #[test]
    fn test_no_slots() {
        let partitions = PartitionsConfig { partitions: vec![], ..Default::default() };
        let images_a = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/a/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/a/fuchsia.vbmeta".into()),
                Image::Dtbo("path/to/a/dtbo".into()),
                Image::FVM("path/to/a/fvm.blk".into()),
                Image::FVMFastboot("path/to/a/fvm.fastboot.blk".into()),
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let mut mapper = PartitionImageMapper::new(partitions).unwrap();
        mapper.map_images_to_slot(&images_a.images, Slot::A).unwrap();
        assert!(mapper.map().is_empty());
    }

    #[test]
    fn test_slot_a_only() {
        let partitions = PartitionsConfig {
            partitions: vec![
                Partition::ZBI { name: "zbi_a".into(), slot: Slot::A, size: None },
                Partition::VBMeta { name: "vbmeta_a".into(), slot: Slot::A, size: None },
                Partition::Dtbo { name: "dtbo_a".into(), slot: Slot::A, size: None },
                Partition::FVM { name: "fvm".into(), size: None },
            ],
            ..Default::default()
        };
        let images_a = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/a/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/a/fuchsia.vbmeta".into()),
                Image::Dtbo("path/to/a/dtbo".into()),
                Image::FVM("path/to/a/fvm.blk".into()),
                Image::FVMFastboot("path/to/a/fvm.fastboot.blk".into()),
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let images_b = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/b/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/b/fuchsia.vbmeta".into()),
                Image::Dtbo("path/to/b/dtbo".into()),
                Image::FVM("path/to/b/fvm.blk".into()),
                Image::FVMFastboot("path/to/b/fvm.fastboot.blk".into()),
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let images_r = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/r/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/r/fuchsia.vbmeta".into()),
                Image::FVM("path/to/r/fvm.blk".into()),
                Image::FVMFastboot("path/to/r/fvm.fastboot.blk".into()),
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let mut mapper = PartitionImageMapper::new(partitions).unwrap();
        mapper.map_images_to_slot(&images_a.images, Slot::A).unwrap();
        mapper.map_images_to_slot(&images_b.images, Slot::B).unwrap();
        mapper.map_images_to_slot(&images_r.images, Slot::R).unwrap();

        let expected = vec![
            PartitionAndImage {
                partition: Partition::ZBI { name: "zbi_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/fuchsia.zbi".into(),
            },
            PartitionAndImage {
                partition: Partition::VBMeta { name: "vbmeta_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/fuchsia.vbmeta".into(),
            },
            PartitionAndImage {
                partition: Partition::Dtbo { name: "dtbo_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/dtbo".into(),
            },
            PartitionAndImage {
                partition: Partition::FVM { name: "fvm".into(), size: None },
                path: "path/to/a/fvm.fastboot.blk".into(),
            },
        ];
        assert_eq!(expected, mapper.map());
    }

    #[test]
    fn test_missing_slot() {
        let partitions = PartitionsConfig {
            partitions: vec![
                Partition::ZBI { name: "zbi_a".into(), slot: Slot::A, size: None },
                Partition::VBMeta { name: "vbmeta_a".into(), slot: Slot::A, size: None },
                Partition::Dtbo { name: "dtbo_a".into(), slot: Slot::A, size: None },
                Partition::ZBI { name: "zbi_b".into(), slot: Slot::B, size: None },
                Partition::VBMeta { name: "vbmeta_b".into(), slot: Slot::B, size: None },
                Partition::Dtbo { name: "dtbo_b".into(), slot: Slot::B, size: None },
                Partition::ZBI { name: "zbi_r".into(), slot: Slot::R, size: None },
                Partition::VBMeta { name: "vbmeta_r".into(), slot: Slot::R, size: None },
                Partition::FVM { name: "fvm".into(), size: None },
                Partition::Fxfs { name: "fxfs".into(), size: None },
            ],
            ..Default::default()
        };
        let images_a = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/a/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/a/fuchsia.vbmeta".into()),
                Image::Dtbo("path/to/a/dtbo".into()),
                Image::FVM("path/to/a/fvm.blk".into()),
                Image::FVMFastboot("path/to/a/fvm.fastboot.blk".into()),
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let images_r = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/r/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/r/fuchsia.vbmeta".into()),
                Image::FVM("path/to/r/fvm.blk".into()),
                Image::FVMFastboot("path/to/r/fvm.fastboot.blk".into()),
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let mut mapper = PartitionImageMapper::new(partitions).unwrap();
        mapper.map_images_to_slot(&images_a.images, Slot::A).unwrap();
        mapper.map_images_to_slot(&images_r.images, Slot::R).unwrap();

        let expected = vec![
            PartitionAndImage {
                partition: Partition::ZBI { name: "zbi_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/fuchsia.zbi".into(),
            },
            PartitionAndImage {
                partition: Partition::VBMeta { name: "vbmeta_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/fuchsia.vbmeta".into(),
            },
            PartitionAndImage {
                partition: Partition::Dtbo { name: "dtbo_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/dtbo".into(),
            },
            PartitionAndImage {
                partition: Partition::ZBI { name: "zbi_r".into(), slot: Slot::R, size: None },
                path: "path/to/r/fuchsia.zbi".into(),
            },
            PartitionAndImage {
                partition: Partition::VBMeta { name: "vbmeta_r".into(), slot: Slot::R, size: None },
                path: "path/to/r/fuchsia.vbmeta".into(),
            },
            PartitionAndImage {
                partition: Partition::FVM { name: "fvm".into(), size: None },
                path: "path/to/a/fvm.fastboot.blk".into(),
            },
        ];
        assert_eq!(expected, mapper.map());
    }

    #[test]
    fn test_multiple_dtbos_must_be_equal() {
        let temp_dir = TempDir::new().unwrap();
        let temp_dir_path = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).unwrap();
        let dtbo_one = temp_dir_path.join("dtbo");
        let dtbo_two = temp_dir_path.join("dtbo");
        let dtbo_three = temp_dir_path.join("dtbo_other");
        std::fs::write(&dtbo_one, "dtbo_same").unwrap();
        std::fs::write(&dtbo_two, "dtbo_same").unwrap();
        std::fs::write(&dtbo_three, "dtbo_diff").unwrap();

        let partitions = PartitionsConfig {
            partitions: vec![Partition::Dtbo { name: "dtbo_a".into(), slot: Slot::A, size: None }],
            ..Default::default()
        };
        let images_one = AssembledSystem {
            images: vec![Image::Dtbo(dtbo_one)],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let images_two = AssembledSystem {
            images: vec![Image::Dtbo(dtbo_two)],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let images_three = AssembledSystem {
            images: vec![Image::Dtbo(dtbo_three)],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let mut mapper = PartitionImageMapper::new(partitions).unwrap();
        assert!(mapper.map_images_to_slot(&images_one.images, Slot::A).is_ok());
        assert!(mapper.map_images_to_slot(&images_two.images, Slot::A).is_ok());
        assert!(mapper.map_images_to_slot(&images_three.images, Slot::A).is_err());
    }

    #[test]
    fn test_size_report() {
        let temp_dir = TempDir::new().unwrap();
        let temp_dir_path = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).unwrap();
        let zbi_path = temp_dir_path.join("zbi");
        let vbmeta_path = temp_dir_path.join("vbmeta");
        let dtbo_path = temp_dir_path.join("dtbo");
        let size_report_path = temp_dir_path.join("report.json");

        std::fs::write(&zbi_path, "zbi").unwrap();
        std::fs::write(&vbmeta_path, "vbmeta").unwrap();
        std::fs::write(&dtbo_path, "dtbo").unwrap();

        let partitions = PartitionsConfig {
            partitions: vec![
                Partition::ZBI { name: "zbi_a".into(), slot: Slot::A, size: Some(100) },
                Partition::VBMeta { name: "vbmeta_a".into(), slot: Slot::A, size: Some(200) },
                Partition::Dtbo { name: "dtbo_a".into(), slot: Slot::A, size: Some(300) },
                Partition::FVM { name: "fvm".into(), size: None },
            ],
            ..Default::default()
        };
        let images_a = AssembledSystem {
            images: vec![
                Image::ZBI { path: zbi_path, signed: false },
                Image::VBMeta(vbmeta_path),
                Image::Dtbo(dtbo_path),
                Image::FVM("path/to/a/fvm.blk".into()),
                Image::FVMFastboot("path/to/a/fvm.fastboot.blk".into()),
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let mut mapper = PartitionImageMapper::new(partitions).unwrap();
        mapper.map_images_to_slot(&images_a.images, Slot::A).unwrap();
        mapper.generate_gerrit_size_report(&size_report_path, &"prefix".to_string()).unwrap();

        let result: serde_json::Value = read_config(&size_report_path).unwrap();
        let expected = serde_json::json!({
            "prefix-dtbo_a": 4,
            "prefix-dtbo_a.budget": 300,
            "prefix-dtbo_a.creepBudget": 200 * 1024,
            "prefix-dtbo_a.owner": "http://go/fuchsia-size-stats/single_component/?f=component%3Ain%3Aprefix-dtbo_a",
            "prefix-vbmeta_a": 6,
            "prefix-vbmeta_a.budget": 200,
            "prefix-vbmeta_a.creepBudget": 200 * 1024,
            "prefix-vbmeta_a.owner": "http://go/fuchsia-size-stats/single_component/?f=component%3Ain%3Aprefix-vbmeta_a",
            "prefix-zbi_a": 3,
            "prefix-zbi_a.budget": 100,
            "prefix-zbi_a.creepBudget": 200 * 1024,
            "prefix-zbi_a.owner": "http://go/fuchsia-size-stats/single_component/?f=component%3Ain%3Aprefix-zbi_a",
        });
        assert_eq!(expected, result);
    }

    #[test]
    fn test_ab_slotted_recovery() {
        let partitions = PartitionsConfig {
            partitions: vec![
                Partition::ZBI { name: "zbi_a".into(), slot: Slot::A, size: None },
                Partition::VBMeta { name: "vbmeta_a".into(), slot: Slot::A, size: None },
                Partition::RecoveryZBI { name: "recovery_a".into(), slot: Slot::A, size: None },
                Partition::RecoveryVBMeta {
                    name: "vbmeta_recovery_a".into(),
                    slot: Slot::A,
                    size: None,
                },
            ],
            ..Default::default()
        };
        let images_a = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/a/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/a/fuchsia.vbmeta".into()),
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let images_r = AssembledSystem {
            images: vec![
                Image::ZBI { path: "path/to/r/fuchsia.zbi".into(), signed: false },
                Image::VBMeta("path/to/r/fuchsia.vbmeta".into()),
            ],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let mut mapper = PartitionImageMapper::new(partitions).unwrap();
        assert_eq!(RecoveryStyle::AB, mapper.recovery_style);
        mapper.map_images_to_slot(&images_a.images, Slot::A).unwrap();
        mapper.map_images_to_slot(&images_r.images, Slot::R).unwrap();

        let expected = vec![
            PartitionAndImage {
                partition: Partition::ZBI { name: "zbi_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/fuchsia.zbi".into(),
            },
            PartitionAndImage {
                partition: Partition::VBMeta { name: "vbmeta_a".into(), slot: Slot::A, size: None },
                path: "path/to/a/fuchsia.vbmeta".into(),
            },
            PartitionAndImage {
                partition: Partition::RecoveryZBI {
                    name: "recovery_a".into(),
                    slot: Slot::A,
                    size: None,
                },
                path: "path/to/r/fuchsia.zbi".into(),
            },
            PartitionAndImage {
                partition: Partition::RecoveryVBMeta {
                    name: "vbmeta_recovery_a".into(),
                    slot: Slot::A,
                    size: None,
                },
                path: "path/to/r/fuchsia.vbmeta".into(),
            },
        ];
        assert_eq!(expected, mapper.map());
    }

    #[test]
    fn test_invalid_ab_and_r_slotted_recovery() {
        let partitions = PartitionsConfig {
            partitions: vec![
                Partition::RecoveryZBI { name: "recovery_a".into(), slot: Slot::A, size: None },
                Partition::ZBI { name: "recovery_r".into(), slot: Slot::R, size: None },
            ],
            ..Default::default()
        };
        assert!(PartitionImageMapper::new(partitions).is_err());
    }
}
