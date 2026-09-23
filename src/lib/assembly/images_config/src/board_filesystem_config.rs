// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow, bail};
use assembly_container::{WalkPaths, WalkPathsFn};
use camino::Utf8PathBuf;
use fshost_assembly_config::BlockDeviceConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The board options for configuring the filesystem.
/// The options include those derived from the partition table and what the bootloader expects.
/// A board developer can specify options for many different filesystems, and let the product
/// choose which filesystem to actually create.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, WalkPaths)]
#[serde(deny_unknown_fields)]
pub struct BoardFilesystemConfig {
    /// Required board configuration for a zbi. All assemblies must produce a ZBI.
    #[serde(default)]
    #[walk_paths]
    pub zbi: Zbi,

    /// Board configuration for a vbmeta if necessary. The bootloader determines whether a vbmeta
    /// is necessary, therefore this is an optional board-level argument. fxfs and fvm below are
    /// chosen by the product, therefore those variables are always available for all boards.
    #[serde(default)]
    #[walk_paths]
    pub vbmeta: Option<VBMeta>,

    /// Board configuration for a fxfs if requested by the product. If the product does not
    /// request a fxfs, then these values are ignored.
    #[serde(default)]
    pub fxfs: Fxfs,

    /// Board configuration for a fvm if requested by the product. If the product does not
    /// request a fvm, then these values are ignored.
    #[serde(default)]
    pub fvm: Fvm,

    /// Configures how GPT-formatted block devices are handled.
    #[serde(default)]
    pub gpt: GptMode,

    /// Mapping of board-specific partition identifiers to semantic block device labels. These
    /// semantic labels are then used by fshost to route the block devices in the platform.
    #[serde(default)]
    pub block_devices: Vec<BlockDeviceConfig>,

    /// Instruct fshost to watch devfs for dfv1 drivers. This is intended to be used on boards
    /// where some of the drivers are not yet converted to dfv2, in situations when fshost would
    /// otherwise not look for them. This flag is ignored when `storage_host` is disabled.
    /// TODO(https://fxbug.dev/445938525): Remove this flag when all the relevant drivers export
    /// services.
    #[serde(default)]
    pub watch_deprecated_v1_drivers: bool,

    /// DEPRECATED.  Use GptMode::AllowMultiple.
    #[serde(default)]
    pub gpt_all: bool,

    /// Merges the 'super' and 'userdata' partitions in the GPT into a single logical partition, if
    /// they exist and are contiguous.  Note that the underlying GPT is not modified and the
    /// original partitions still exist.
    #[serde(default)]
    pub merge_super_and_userdata: bool,

    /// If set, use keymint for encrypting data. Requires the `fuchsia::keymint` board capability.
    #[serde(default)]
    pub keymint_enabled: bool,
}

/// How GPT-formatted block devices ought to be handled.
#[derive(Serialize, Deserialize, Debug, Default, Clone, Copy, PartialEq)]
#[serde(try_from = "String", into = "String")]
pub enum GptMode {
    /// Don't permit binding to GPT-formatted block devices.
    Disabled,
    /// Only permit binding to a single GPT-formatted block device.  This is the most common
    /// configuration, as typically there would be a single storage device which is GPT-formatted.
    #[default]
    Enabled,
    /// Permit binding to multiple GPT-formatted block devices.  This exists to support boards with
    /// multiple storage devices, or for removable media support.
    AllowMultiple,
}

impl GptMode {
    /// Whether GPT-formatted block devices are supported.
    pub fn enabled(&self) -> bool {
        if let Self::Disabled = self { false } else { true }
    }
}

impl TryFrom<String> for GptMode {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl TryFrom<&str> for GptMode {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let mode = match value {
            "disabled" => Some(Self::Disabled),
            "enabled" => Some(Self::Enabled),
            "allow_multiple" => Some(Self::AllowMultiple),
            _ => None,
        };
        mode.ok_or_else(|| {
            anyhow!(
                "Not a valid gpt mode, must be 'disabled', 'enabled', or 'allow_multiple': {}",
                value
            )
        })
    }
}

impl std::fmt::Display for GptMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GptMode::Disabled => f.write_str("disabled"),
            GptMode::Enabled => f.write_str("enabled"),
            GptMode::AllowMultiple => f.write_str("allow_multiple"),
        }
    }
}

impl From<GptMode> for String {
    fn from(value: GptMode) -> Self {
        value.to_string()
    }
}

/// Parameters describing how to generate the ZBI.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, WalkPaths)]
#[serde(deny_unknown_fields)]
pub struct Zbi {
    /// The compression format for the ZBI.
    #[serde(default)]
    pub compression: ZbiCompression,

    /// An optional script to post-process the ZBI.
    /// This is often used to prepare the ZBI for flashing/updating.
    #[serde(default)]
    #[walk_paths]
    pub postprocessing_script: Option<PostProcessingScript>,
}

/// The compression format for the ZBI.
#[derive(Serialize, Deserialize, Debug, Default, Clone, Copy, PartialEq)]
#[serde(try_from = "String", into = "String")]
pub enum ZbiCompression {
    /// zstd default compression.
    #[default]
    ZStd,

    /// zstd compression at a specific level (4 <= level <= 21)
    ZStdLevel(u8),

    /// zstd.max compression.
    ZStdMax,

    /// no compression.
    None,
}

impl TryFrom<String> for ZbiCompression {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl TryFrom<&str> for ZbiCompression {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let level = match value {
            "none" => Some(Self::None),
            "zstd" => Some(Self::ZStd),
            "zstd.max" => Some(Self::ZStdMax),
            _ => value.strip_prefix("zstd.").and_then(|v| v.parse::<u8>().ok()).and_then(|level| {
                if level >= 4 && level <= 21 { Some(Self::ZStdLevel(level)) } else { None }
            }),
        };
        level.ok_or_else(||  anyhow!("Not a valid zstd compression level, must be 'none', 'zstd', 'zstd.max', or 'zstd.<N> where 4 <= N <= 21: {}", value))
    }
}

impl std::fmt::Display for ZbiCompression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZbiCompression::ZStd => f.write_str("zstd"),
            ZbiCompression::ZStdLevel(level) => f.write_fmt(format_args!("zstd.{}", level)),
            ZbiCompression::ZStdMax => f.write_str("zstd.max"),
            ZbiCompression::None => f.write_str("none"),
        }
    }
}

impl From<ZbiCompression> for String {
    fn from(value: ZbiCompression) -> Self {
        value.to_string()
    }
}

/// A script to process the ZBI after it is constructed.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PostProcessingScript {
    /// TODO(lijiaming) This is going to be deprecated once we move all the users to use the board_script_path.
    /// The path to the script on host.
    /// This script _must_ take the following arguments:
    ///   -z <path to ZBI>
    ///   -o <output path>
    ///   -B <build directory, relative to script's source directory>
    #[serde(default)]
    pub path: Option<Utf8PathBuf>,

    /// The path to the script, relative to board configuration.
    /// This script _must_ take the following arguments:
    ///   -z <path to ZBI>
    ///   -o <output path>
    #[serde(default)]
    pub board_script_path: Option<Utf8PathBuf>,

    /// Additional arguments to pass to the script after the above arguments.
    #[serde(default)]
    pub args: Vec<String>,

    /// The input files needed by `board_script_path` at runtime.
    /// These are expected to be a sibling to `board_script_path`.
    ///
    /// This is a map of the path that is expected by `board_script_path` to
    /// the path in the container, and must include the `board_script_path`.
    #[serde(default)]
    pub inputs: BTreeMap<Utf8PathBuf, Utf8PathBuf>,
}

// We need to implement a custom WalkPaths to ensure `inputs` sits in the same
// directory as `board_script_path`.
impl WalkPaths for PostProcessingScript {
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> Result<()> {
        // Walk all the `inputs` and ensure `board_script_path` is NOT inside the list.
        // If it's inside the `inputs`, then we'll accidentally copy the file twice,
        // causing issues.
        for (relative_path, path) in &mut self.inputs {
            let parent = relative_path
                .parent()
                .ok_or_else(|| anyhow!("Failed to get parent directory for: {}", relative_path))?;
            let destination = dest.join(parent);

            // If we find a source that matches the `board_script_path`,
            // remember the destination for later.
            let path_owned = path.clone();
            if Some(path_owned) == self.board_script_path {
                bail!("The zbi signing script should not be inside the signing inputs");
            }

            path.walk_paths_with_dest(found, destination)?;
        }

        // Walk board_script_path.
        self.board_script_path.walk_paths_with_dest(found, dest)?;
        Ok(())
    }
}

/// The parameters describing how to create a VBMeta image.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, WalkPaths)]
#[serde(deny_unknown_fields)]
pub struct VBMeta {
    /// The style of VBMeta image to assemble.
    #[serde(default)]
    pub style: VBMetaStyle,

    /// Path on host to the key for signing VBMeta.

    #[walk_paths]
    pub key: Utf8PathBuf,

    /// Optional path on host to the key metadata to add to the VBMeta, when
    /// applicable.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[walk_paths]
    pub key_metadata: Option<Utf8PathBuf>,

    /// Optional descriptors to add to the VBMeta image.
    #[serde(default)]
    pub additional_descriptors: Vec<VBMetaDescriptor>,

    /// Optional name to override the default name of the vbmeta image.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Whether to include the base merkle root command line descriptor.
    #[serde(default)]
    pub include_base_merkle: bool,
}

/// The parameters of a VBMeta descriptor to add to a VBMeta image.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VBMetaDescriptor {
    /// Name of the partition.
    pub name: String,

    /// Size of the partition in bytes.
    pub size: u64,

    /// Custom VBMeta flags to add.
    pub flags: u32,

    /// Minimum AVB version to add.
    pub min_avb_version: String,
}

/// The style of VBMeta image to assemble.
#[derive(Serialize, Deserialize, Debug, Default, Clone, Copy, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum VBMetaStyle {
    /// Generates a standalone, Fuchsia-style VBMeta image.
    #[default]
    Fuchsia,

    /// Generates a standalone VBMeta image for the system partition.
    #[serde(rename = "vbmeta-system")]
    VBMetaSystem,

    /// Generates the VBMeta expected by Android protected VMs as a footer to
    /// the product bundle's QEMU kernel (i.e., boot shim), where the choice of
    /// ramdisk (i.e., ZBI) hash descriptor 'partition name' - which controls a
    /// UART-related debug policy - is "initrd_normal" in user builds and
    /// "initrd_debug" in non-user builds.
    ///
    /// See https://android.googlesource.com/platform/packages/modules/Virtualization/+/refs/heads/android16-release/guest/pvmfw/README.md#guest-image-signing
    /// for more details.
    AndroidPvmAuto,

    /// Similar to `AndroidPvmAuto`, except that the ramdisk hash descriptor partition name
    /// is fixed as "initrd_normal".
    AndroidPvmNormal,

    /// Similar to `AndroidPvmAuto`, except that the ramdisk hash descriptor partition name
    /// is fixed as "initrd_debug".
    AndroidPvmDebug,
}

/// How blobs should be formatted in the base fxfs image during assembly.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum FxfsBlobFormat {
    /// Blobs will not be compressed.
    Uncompressed,

    /// Blobs will be compressed with Zstd.
    #[default]
    Zstd,

    /// Blobs will be compressed with LZ4.
    Lz4,
}

/// Build-type specific size configurations.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct BuildSpecificSize {
    /// Limit applied to 'eng' builds.
    pub eng: Option<u64>,
    /// Limit applied to 'userdebug' builds.
    pub userdebug: Option<u64>,
    /// Limit applied to 'user' builds.
    pub user: Option<u64>,
}

/// A flexible size definition that can be either uniform or build-specific.
///
/// Example:
///
/// ```json
/// "size_uniform": 1048576
///
/// "size_build_specific": {
///   "eng": 1048576,
///   "userdebug": 2097152,
///   "user": 4194304
/// }
/// ```
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(untagged)]
pub enum FlexibleSize {
    /// A single size applied universally.
    Uniform(u64),
    /// Granular overrides based on the build type.
    BuildSpecific(BuildSpecificSize),
}

/// The parameters describing how to create an Fxfs image.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Fxfs {
    /// If `target_size` bytes is set, the raw image will be set to exactly this
    /// size (and an error is returned if the contents exceed that size).  If
    /// unset (or 0), the image will be truncated to twice the size of its
    /// contents, which is a heuristic that gives us roughly enough space for
    /// normal usage of the image.
    pub size_bytes: Option<u64>,

    /// The maximum number of bytes we can place in Fxfs during assembly.
    /// This value must be smaller than `size_bytes` which is the absolute maximum amount of space
    /// we can use in Fxfs.
    pub size_checker_maximum_bytes: Option<u64>,

    /// Maximum number of bytes that the blob volume can consume at runtime.
    pub blob_maximum_bytes: Option<FlexibleSize>,

    /// Maximum number of bytes that the data volume can consume at runtime.
    pub data_maximum_bytes: Option<FlexibleSize>,

    /// How blobs should be formatted in the base fxfs image during assembly.
    #[serde(skip_serializing_if = "crate::is_default")]
    pub blob_format: FxfsBlobFormat,
}

impl Default for Fxfs {
    fn default() -> Self {
        Self {
            size_bytes: None,
            size_checker_maximum_bytes: None,
            blob_maximum_bytes: None,
            data_maximum_bytes: None,
            blob_format: FxfsBlobFormat::default(),
        }
    }
}

/// The parameters describing how to create a FVM image.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Fvm {
    /// Slice size (in bytes) for an FVM if requested by a product. If the product does not
    /// request a minfs, then these values are ignored.
    #[serde(default)]
    pub slice_size: FvmSliceSize,

    /// If provided, the standard fvm will be truncated to the specified length.
    #[serde(default)]
    pub truncate_to_length: Option<u64>,

    /// Board configuration for a blobfs if requested by a product. If the product does not
    /// request a blobfs, then these values are ignored.
    #[serde(default)]
    pub blobfs: Blobfs,

    /// Board configuration for a minfs if requested by the product. If the product does not
    /// request a minfs, then these values are ignored.
    #[serde(default)]
    pub minfs: Minfs,

    /// If specified, a sparse fvm will be built with the supplied configuration.
    #[serde(default)]
    pub sparse_output: Option<SparseFvmConfig>,

    /// If specified, a nand fvm will be built with the supplied configuration.
    #[serde(default)]
    pub nand_output: Option<NandFvmConfig>,

    /// If specified, a fvm will be built with the supplied configuration that is optimized for
    /// fastboot flashing.
    #[serde(default)]
    pub fastboot_output: Option<FastbootFvmConfig>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FvmSliceSize(pub u64);
impl Default for FvmSliceSize {
    fn default() -> Self {
        Self(8388608)
    }
}

/// Configuration for building a Blobfs volume.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Blobfs {
    /// The maximum number of bytes we can place in blobfs during assembly.
    /// This value must be smaller than `maximum_bytes` which is the absolute maximum amount
    /// of space we can use in blobfs at runtime.
    #[serde(default)]
    pub size_checker_maximum_bytes: Option<u64>,

    /// Maximum number of bytes blobfs can consume at build time.
    /// This value is used by the fvm tool to preallocate space.
    /// Most boards should avoid setting this value, and set maximum_bytes instead.
    #[serde(default)]
    pub build_time_maximum_bytes: Option<u64>,

    /// Maximum number of bytes blobfs can consume at runtime.
    /// This value is placed in fshost config to enforce size budgets at runtime.
    #[serde(default)]
    pub maximum_bytes: Option<u64>,

    /// Minimum number of inodes to reserve in blobfs.
    #[serde(default)]
    pub minimum_inodes: Option<u64>,

    /// Minimum number of bytes to reserve for blob data.
    #[serde(default)]
    pub minimum_data_bytes: Option<u64>,
}

/// Configuration for building an Minfs volume.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Minfs {
    /// Maximum number of bytes minfs can consume at runtime.
    #[serde(default)]
    pub maximum_bytes: Option<u64>,
}

/// A FVM that is compressed sparse.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SparseFvmConfig {
    /// The maximum size the FVM can expand to at runtime.
    /// This sets the amount of slice metadata to allocate during construction,
    /// which cannot be modified at runtime.
    #[serde(default)]
    pub max_disk_size: Option<u64>,
}

/// A FVM prepared for a Nand partition.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NandFvmConfig {
    /// The maximum size the FVM can expand to at runtime.
    /// This sets the amount of slice metadata to allocate during construction,
    /// which cannot be modified at runtime.
    #[serde(default)]
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

/// A FVM prepared for fastboot flashing.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FastbootFvmConfig {
    /// Whether to compress the FVM.
    #[serde(default)]
    pub compress: bool,

    /// Truncate the file to this length.
    #[serde(default)]
    pub truncate_to_length: Option<u64>,
}

#[cfg(test)]
mod tests {

    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_zbi_compression_to_string() {
        assert_eq!(ZbiCompression::None.to_string(), "none");
        assert_eq!(&ZbiCompression::ZStd.to_string(), "zstd");
        assert_eq!(&ZbiCompression::ZStdMax.to_string(), "zstd.max");
        assert_eq!(&ZbiCompression::ZStdLevel(12).to_string(), "zstd.12");
    }

    #[test]
    fn test_zbi_compression_from_string() {
        assert_eq!(ZbiCompression::try_from("none").unwrap(), ZbiCompression::None);
        assert_eq!(ZbiCompression::try_from("zstd").unwrap(), ZbiCompression::ZStd);
        assert_eq!(ZbiCompression::try_from("zstd.max").unwrap(), ZbiCompression::ZStdMax);
        assert_eq!(ZbiCompression::try_from("zstd.12").unwrap(), ZbiCompression::ZStdLevel(12));
    }

    #[test]
    fn test_zbi_compression_from_string_out_of_range() {
        // Invalid levels (too low)
        assert!(ZbiCompression::try_from("zstd.0").is_err());
        assert!(ZbiCompression::try_from("zstd.1").is_err());
        assert!(ZbiCompression::try_from("zstd.3").is_err());

        // Invalid levels (too high)
        assert!(ZbiCompression::try_from("zstd.22").is_err());
        assert!(ZbiCompression::try_from("zstd.23").is_err());
        assert!(ZbiCompression::try_from("zstd.346204345").is_err());

        // Invalid level (negagive)
        assert!(ZbiCompression::try_from("zstd.-1").is_err());

        // Invalid level (junk)
        assert!(ZbiCompression::try_from("zstd.sdflkajsdfasl;dfj").is_err());
        assert!(ZbiCompression::try_from("sdafasdflkajsdfasl;dfj").is_err());
    }

    // This checks that the deserialization for serde is wired up correctly, as the TryFrom<String>
    // implementation is necessary for this to work.  Using '&str' and telling serde to deserialize
    // using TryFrom<&str> compiles, but fails this test.  If this test is written using the
    // serde_json::from_str() function, it would (falsely) pass the test, but uses in-tree of
    // serd_json::from_value() would fail.
    #[test]
    fn test_zbi_compression_deserialization_from_value() {
        assert_eq!(
            serde_json::from_value::<ZbiCompression>(serde_json::json!("none")).unwrap(),
            ZbiCompression::None
        );
    }

    #[test]
    fn test_postprocessing_script_walk() {
        let mut script = PostProcessingScript {
            path: None,
            board_script_path: Some(Utf8PathBuf::from(
                Utf8PathBuf::from_str("input/script.sh").unwrap(),
            )),
            args: vec![],
            inputs: [(
                Utf8PathBuf::from_str("nested/resource").unwrap(),
                Utf8PathBuf::from(Utf8PathBuf::from_str("input/resource.txt").unwrap()),
            )]
            .into(),
        };
        let mut found_dest = vec![];
        script
            .walk_paths_with_dest(
                &mut |path: &mut Utf8PathBuf,
                      dest: Utf8PathBuf,
                      filetype: assembly_container::FileType| {
                    found_dest.push(dest);
                    *path = Utf8PathBuf::from_str("bogus").unwrap();
                    assert_eq!(assembly_container::FileType::Unknown, filetype);
                    Ok(())
                },
                Utf8PathBuf::from_str("inner/path").unwrap(),
            )
            .unwrap();
        found_dest.sort();
        assert_eq!(
            vec![
                Utf8PathBuf::from_str("inner/path").unwrap(),
                Utf8PathBuf::from_str("inner/path/nested").unwrap(),
            ],
            found_dest
        );
    }

    #[test]
    fn test_fxfs_blob_format_deserialization() {
        assert_eq!(
            serde_json::from_value::<FxfsBlobFormat>(serde_json::json!("zstd")).unwrap(),
            FxfsBlobFormat::Zstd
        );
        assert_eq!(
            serde_json::from_value::<FxfsBlobFormat>(serde_json::json!("lz4")).unwrap(),
            FxfsBlobFormat::Lz4
        );
        assert_eq!(
            serde_json::from_value::<FxfsBlobFormat>(serde_json::json!("uncompressed")).unwrap(),
            FxfsBlobFormat::Uncompressed
        );
    }

    #[test]
    fn test_fxfs_default_serialization() {
        assert_eq!(
            serde_json::to_value(Fxfs::default()).unwrap(),
            serde_json::json!({
                "blob_maximum_bytes": null,
                "data_maximum_bytes": null,
                "size_bytes": null,
                "size_checker_maximum_bytes": null,
            })
        );
    }

    #[test]
    fn test_fxfs_build_specific_size_serialization() {
        let fxfs = Fxfs {
            blob_maximum_bytes: Some(FlexibleSize::BuildSpecific(BuildSpecificSize {
                eng: Some(1048576),
                userdebug: Some(2097152),
                user: Some(4194304),
            })),
            data_maximum_bytes: Some(FlexibleSize::Uniform(1048576)),
            size_bytes: Some(1048576),
            size_checker_maximum_bytes: Some(1048576),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(fxfs).unwrap(),
            serde_json::json!({
                "blob_maximum_bytes": {
                    "eng": 1048576,
                    "userdebug": 2097152,
                    "user": 4194304,
                },
                "data_maximum_bytes": 1048576,
                "size_bytes": 1048576,
                "size_checker_maximum_bytes": 1048576,
            })
        );
    }
}
