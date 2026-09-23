// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use assembly_config_schema::BuildType;
use assembly_constants::FileEntry;
use assembly_images_config::{FilesystemImageMode, ImagesConfig};
use assembly_release_info::SystemReleaseInfo;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

/// The set of information that defines a fuchsia product.  This is capable of
/// being a complete configuration (it at least has a kernel).
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageAssemblyConfig {
    /// The packages whose files get added to the base package. The
    /// packages themselves are not added, but their individual files are
    /// extracted and added to the base package. These files are needed
    /// to bootstrap pkgfs.
    #[serde(default)]
    pub system: Vec<Utf8PathBuf>,

    /// The packages that are in the base package list, excluding drivers
    /// which are added to the base package (data/static_packages).
    /// These packages get updated by flashing and OTAing, and cannot
    /// be garbage collected.
    #[serde(default)]
    pub base: Vec<Utf8PathBuf>,

    /// The packages that are in the cache package list, which is added
    /// to the base package (data/cache_packages). These packages get
    /// updated by flashing and OTAing, but can be garbage collected.
    #[serde(default)]
    pub cache: Vec<Utf8PathBuf>,

    /// The automatic anchored packages that are in the anchored package list, which is added
    /// to the base package (data/anchored_packages.json).
    /// These packages are not part of the image itself.
    #[serde(default)]
    pub anchored_automatic: Vec<Utf8PathBuf>,

    /// The on-demand anchored packages that are in the anchored package list, which is added
    /// to the base package (data/anchored_packages.json).
    /// These packages are not part of the image itself.
    #[serde(default)]
    pub anchored_on_demand: Vec<Utf8PathBuf>,

    /// The packages that are in the on_demand package list, which is not part
    /// of the image itself, but is to be published alongside those packages in
    /// the image. Unlike the anchored_on_demand packages, the list of these packages is not
    /// added to the base package in `data/`.
    #[serde(default)]
    pub on_demand: Vec<Utf8PathBuf>,

    /// The parameters that specify which kernel to put into the ZBI.
    pub kernel: KernelConfig,

    /// The qemu kernel to use when starting the emulator.
    pub qemu_kernel: Utf8PathBuf,

    /// The list of additional boot args to add.
    #[serde(default)]
    pub boot_args: Vec<String>,

    /// The set of files to be placed in BOOTFS in the ZBI.
    #[serde(default)]
    pub bootfs_files: Vec<FileEntry<String>>,

    /// The packages that are in the bootfs package list, which are
    /// added to the BOOTFS in the ZBI.
    #[serde(default)]
    pub bootfs_packages: Vec<Utf8PathBuf>,

    /// Which images to produce and how.
    #[serde(default)]
    pub images_config: ImagesConfig,

    /// The name of the board that these images can be flashed to.
    pub board_name: String,

    /// The partitions that this assembly can be flashed to.
    pub partitions_config: Option<Utf8PathBuf>,

    /// Optionally-provided data to pass to the board's Board Driver via a ZBI
    /// item.
    pub board_driver_arguments: Option<BoardDriverArguments>,

    /// ZBI non bootable image containing extra items to add to the ZBI.
    pub zbi_extra_items: Option<Utf8PathBuf>,

    /// Devicetree binary to be added to zbi
    pub devicetree: Option<Utf8PathBuf>,

    /// Devicetree binary overlay to be added to the update package and flashed
    /// to a dedicated partition.
    pub devicetree_overlay: Option<Utf8PathBuf>,

    /// How to generate the filesystem image.
    #[serde(default)]
    pub image_mode: FilesystemImageMode,

    /// Release information about the assembly inputs contributing to this image.
    #[serde(default)]
    pub system_release_info: SystemReleaseInfo,

    /// The platform build type being assembled for.
    pub build_type: BuildType,
}

impl ImageAssemblyConfig {
    /// Helper function for constructing a ImageAssemblyConfig in tests in this
    /// and other modules within the crate.
    pub fn new_for_testing(kernel_path: impl AsRef<Utf8Path>) -> Self {
        Self {
            system: Vec::default(),
            base: Vec::default(),
            cache: Vec::default(),
            anchored_automatic: Vec::default(),
            anchored_on_demand: Vec::default(),
            on_demand: Vec::default(),
            boot_args: Vec::default(),
            partitions_config: None,
            bootfs_files: Vec::default(),
            bootfs_packages: Vec::default(),
            kernel: KernelConfig { path: kernel_path.as_ref().into(), args: Vec::default() },
            qemu_kernel: "path/to/qemu/kernel".into(),
            images_config: ImagesConfig::default(),
            board_name: "my_board".into(),
            board_driver_arguments: None,
            zbi_extra_items: Some("path/to/zbi_extra_items/image".into()),
            devicetree: Some("path/to/devicetree/binary".into()),
            devicetree_overlay: Some("path/to/devicetree/binary/overlay".into()),
            image_mode: Default::default(),
            system_release_info: SystemReleaseInfo::new_for_testing(),
            build_type: BuildType::Eng,
        }
    }
}

/// The information required to specify a kernel and its arguments.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KernelConfig {
    /// The path to the prebuilt kernel.
    pub path: Utf8PathBuf,

    /// The list of command line arguments to pass to the kernel on startup.
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoardDriverArguments {
    /// The vendor id to add to a PLATFORM_ID ZBI Item.
    pub vendor_id: u32,

    /// The product id to add to a PLATFORM_ID ZBI Item.
    pub product_id: u32,

    /// The board name to add to a PLATFORM_ID ZBI Item.
    pub name: String,

    /// The board revision to add to a BOARD_INFO ZBI Item.
    pub revision: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use assembly_util::from_reader;

    fn from_json_str<T>(json: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let mut cursor = std::io::Cursor::new(json);
        from_reader(&mut cursor)
    }

    #[test]
    fn product_from_json_file() {
        let json = r#"
            {
              "system": ["package0"],
              "base": ["package1", "package2"],
              "cache": ["package3", "package4"],
              "on_demand": ["package5", "package6"],
              "anchored_automatic": ["package7", "package8"],
              "anchored_on_demand": ["package9", "package10"],
              "kernel": {
                "path": "path/to/kernel",
                "args": ["arg1", "arg2"],
              },
              "qemu_kernel": "path/to/qemu/kernel",
              "bootfs_files": [
                {
                    "source": "path/to/source",
                    "destination": "path/to/destination"
                }
              ],
              "bootfs_packages": ["package5", "package6"],
              "board_name": "my_board",
              "build_type": "eng"
            }
        "#;

        let config: ImageAssemblyConfig = from_json_str(json).unwrap();
        assert_eq!(
            config.kernel,
            KernelConfig {
                path: "path/to/kernel".into(),
                args: vec!["arg1".into(), "arg2".into()],
            }
        );
    }

    #[test]
    fn product_from_json5_file() {
        let json = r#"
            {
              // json5 files can have comments in them.
              system: ["package0"],
              base: ["package1", "package2"],
              cache: ["package3", "package4"],
              on_demand: ["package5", "package6"],
              anchored_automatic: ["package7", "package8"],
              "anchored_on_demand": ["package9", "package10"],
              kernel: {
                path: "path/to/kernel",
                args: ["arg1", "arg2"],
              },
              qemu_kernel: "path/to/qemu/kernel",
              // and lists can have trailing commas
              boot_args: ["arg1", "arg2", ],
              bootfs_files: [
                {
                    source: "path/to/source",
                    destination: "path/to/destination",
                }
              ],
              bootfs_packages: ["package5", "package6"],
              board_name: "my_board",
              build_type: "user",
            }
        "#;

        let config: ImageAssemblyConfig = from_json_str(json).unwrap();
        assert_eq!(
            config.kernel,
            KernelConfig {
                path: "path/to/kernel".into(),
                args: vec!["arg1".into(), "arg2".into()],
            }
        );
    }

    #[test]
    fn product_from_invalid_json_file() {
        let json = r#"
            {
                "invalid": "data"
            }
        "#;

        let config: Result<ImageAssemblyConfig> = from_json_str(json);
        assert!(config.is_err());
    }
}
