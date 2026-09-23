// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

use std::collections::BTreeMap;

use crate::BuildType;
use crate::platform_settings::sysmem_config::BoardSysmemConfig;
use anyhow::{Result, anyhow};
use assembly_constants::Arm64DebugDapSoc;
use assembly_container::{AssemblyContainer, DirectoryPathBuf, WalkPaths, assembly_container};
use assembly_images_config::BoardFilesystemConfig;
use assembly_release_info::BoardReleaseInfo;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::board_input_bundle::{BoardInputBundle, BoardProvidedConfig, IncludeInBuildType};

/// The architecture of the hardware.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Architecture {
    /// x64.
    #[default]
    X64,

    /// arm64.
    ARM64,

    /// riscv64.
    RISCV64,
}

impl FromStr for Architecture {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "x64" => Ok(Self::X64),
            "arm64" => Ok(Self::ARM64),
            "riscv64" => Ok(Self::RISCV64),
            _ => Err(anyhow!("Unknown architecture: {}", s)),
        }
    }
}

impl std::fmt::Display for Architecture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::X64 => write!(f, "x64"),
            Self::ARM64 => write!(f, "arm64"),
            Self::RISCV64 => write!(f, "riscv64"),
        }
    }
}

/// This struct provides information about the "board" that a product is being
/// assembled to run on.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, WalkPaths)]
#[serde(deny_unknown_fields)]
#[assembly_container(board_configuration.json)]
pub struct BoardConfig {
    /// The name of the board.
    pub name: String,

    /// The architecture of the hardware.
    pub arch: Architecture,

    /// Metadata about the board that's provided to the 'fuchsia.hwinfo.Board'
    /// protocol and to the Board Driver via the PlatformID and BoardInfo ZBI
    /// items.
    #[serde(default)]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub hardware_info: HardwareInfo,

    /// The "features" that this board provides to the product.
    ///
    /// NOTE: This is a still-evolving, loosely-coupled, set of identifiers.
    /// It's an unstable interface between the boards and the platform.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub provided_features: Vec<String>,

    /// Path to a non-bootable ZBI containing extra items to be included in the generated
    /// ZBI for the board.
    #[serde(default)]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zbi_extra_items: Option<Utf8PathBuf>,

    /// Path to the devicetree binary (.dtb) this provided by this board.
    #[serde(default)]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub devicetree: Option<Utf8PathBuf>,

    /// Path to the devicetree binary overlay (.dtbo) this provided by this board.
    #[serde(default)]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub devicetree_overlay: Option<Utf8PathBuf>,

    /// The partitions config that details what partitions are available for
    /// this board.
    #[serde(default)]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partitions_config: Option<DirectoryPathBuf>,

    /// Configuration for the various filesystems that the product can choose to
    /// include.
    #[serde(default)]
    #[walk_paths]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub filesystems: BoardFilesystemConfig,

    /// These are paths to the directories that are board input bundles that
    /// this board configuration includes.  Product assembly will always include
    /// these into the images that it creates.
    ///
    /// These are the board-specific artifacts that the Fuchsia platform needs
    /// added to the assembled system in order to be able to boot Fuchsia on
    /// this board.
    ///
    /// Examples:
    ///  - the "board driver"
    ///  - storage drivers
    ///
    /// If any of these artifacts are removed, even the 'bootstrap' feature set
    /// may be unable to boot.
    #[serde(default)]
    #[walk_paths]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub input_bundles: BTreeMap<String, DirectoryPathBuf>,

    /// Consolidated configuration from all of the BoardInputBundles.  This is
    /// not deserialized from the BoardConfiguration, but is instead created by
    /// parsing each of the input_bundles and merging their configuration fields.
    #[serde(skip)]
    #[walk_paths]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub configuration: BoardProvidedConfig,

    /// Configure kernel cmdline args
    /// TODO: Move this into platform section below
    #[serde(default)]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub kernel: BoardKernelConfig,

    /// Configure platform related feature
    #[serde(default)]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub platform: PlatformSettings,

    /// GUIDs for the TAs provided by this board's TEE driver.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub global_platform_tee_trusted_app_guids: Vec<uuid::Uuid>,

    /// GUIDs for the TAs provided by this board's TEE driver.
    ///
    /// NOTE: This is the deprecated name for
    /// `BoardConfig::global_platform_tee_trusted_app_guids`. At most one of the two fields
    /// may be non-empty.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tee_trusted_app_guids: Vec<uuid::Uuid>,

    /// Release information about this assembly container artifact.
    pub release_info: BoardReleaseInfo,
}

/// This struct defines board-provided data for the 'fuchsia.hwinfo.Board' fidl
/// protocol and for the Platform_ID and Board_Info ZBI items.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HardwareInfo {
    /// This is the value returned in the 'BoardInfo.name' field, if different
    /// from the name provided for the board itself.  It's also the name that's
    /// set in the PLATFORM_ID ZBI Item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// The vendor id to add to a PLATFORM_ID ZBI Item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor_id: Option<u32>,

    /// The product id to add to a PLATFORM_ID ZBI Item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_id: Option<u32>,

    /// The board revision to add to a BOARD_INFO ZBI Item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u32>,
}

impl BoardConfig {
    /// Add the names of the BIBs to the map.
    pub fn add_bib_names(mut self) -> Result<Self> {
        self.input_bundles = self
            .input_bundles
            .into_values()
            .map(|dir| {
                let bib = BoardInputBundle::from_dir(&dir)?;
                Ok::<(String, DirectoryPathBuf), anyhow::Error>((bib.name, dir))
            })
            .collect::<Result<BTreeMap<String, DirectoryPathBuf>>>()?;
        Ok(self)
    }

    /// Merge the board input bundle sets.
    pub fn merge_board_input_bundle_sets(&mut self, bib_sets: Vec<crate::BoardInputBundleSet>) {
        let replace_bib_sets: BTreeMap<String, crate::BoardInputBundleSet> =
            bib_sets.into_iter().map(|set| (set.name.clone(), set)).collect();

        for (full_bib_name, bib_path) in &mut self.input_bundles {
            let bib_ref = BibReference::from(full_bib_name);

            // Replace BIBs that are part of a BIB set.
            if let BibReference::FromBibSet { set, name } = bib_ref
                && let Some(replace_bib_set) = replace_bib_sets.get(&set)
                && let Some(replace_bib_entry) = replace_bib_set.board_input_bundles.get(&name)
            {
                *bib_path = replace_bib_entry.path.clone();
            }
        }
    }
}

/// A reference of a BIB found in a board, which can either have been from a
/// BIB set or added independently not through a set.
pub enum BibReference {
    /// A BIB that was added via a BIB set.
    /// We keep track of the set name, so that we can easily replace the entire
    /// set of BIBs wholesale.
    FromBibSet {
        /// The name of the BIB set.
        set: String,
        /// The name of the BIB.
        name: String,
    },

    /// A BIB that was added independent of a BIB set.
    Independent {
        /// The name of the BIB.
        name: String,
    },
}

impl From<&String> for BibReference {
    fn from(s: &String) -> Self {
        let mut parts: Vec<&str> = s.split("::").collect();
        let bib_name = parts.pop();
        let set_name = parts.pop();
        match (set_name, bib_name) {
            (Some(set), Some(name)) => {
                Self::FromBibSet { set: set.to_string(), name: name.to_string() }
            }
            _ => Self::Independent { name: s.to_string() },
        }
    }
}

impl std::fmt::Display for BibReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FromBibSet { set, name } => write!(f, "{}::{}", set, name),
            Self::Independent { name } => write!(f, "{}", name),
        }
    }
}

impl BoardInputBundle {
    /// Return whether this BIB should be included in a product with the given
    /// build type.
    pub fn should_be_included(&self, build_type: BuildType) -> bool {
        match (&self.include_in, build_type) {
            (&IncludeInBuildType::All, _) => true,
            (&IncludeInBuildType::Eng, BuildType::Eng) => true,
            (&IncludeInBuildType::Eng, BuildType::User | BuildType::UserDebug) => false,
            (&IncludeInBuildType::UserAndUserdebug, BuildType::User | BuildType::UserDebug) => true,
            (&IncludeInBuildType::UserAndUserdebug, BuildType::Eng) => false,
        }
    }
}

/// Where to print the serial logs.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SerialMode {
    /// Do not output any serial logs.
    #[default]
    NoOutput,
    /// Output the serial logs to the legacy console.
    /// This is only valid on 'eng' builds.
    Legacy,
}

/// This struct defines supported kernel features.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BoardKernelConfig {
    /// Enable the use of 'contiguous physical pages'. This should be enabled
    /// when a significant contiguous memory size is required.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub contiguous_physical_pages: bool,

    /// Where to print serial logs.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub serial_mode: SerialMode,

    /// Disable printing to the console during early boot (ie, make it quiet)
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub quiet_early_boot: bool,

    /// When enabled, each ARM cpu will enable an event stream generator, which
    /// per-cpu sets the hidden event flag at a particular rate. This has the
    /// effect of kicking cpus out of any WFE states they may be sitting in.
    pub arm64_event_stream_enable: bool,

    /// This controls what serial port is used.  If provided, it overrides the
    /// serial port described by the system's bootdata.  The kernel debug serial
    /// port is a reserved resource and may not be used outside of the kernel.
    ///
    /// If set to "none", the kernel debug serial port will be disabled and will
    /// not be reserved, allowing the default serial port to be used outside the
    /// kernel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,

    /// When searching for a CPU on which to place a task, prefer little cores
    /// over big cores. Enabling this option trades off improved performance in
    /// favor of reduced power consumption.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub scheduler_prefer_little_cpus: bool,

    /// The system will halt on a kernel panic instead of rebooting.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub halt_on_panic: bool,

    /// Allow debug UART to be suspended.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub allow_debug_uart_suspend: bool,
}

impl Default for BoardKernelConfig {
    fn default() -> Self {
        Self {
            contiguous_physical_pages: false,
            serial_mode: SerialMode::default(),
            quiet_early_boot: false,
            arm64_event_stream_enable: true,
            serial: None,
            scheduler_prefer_little_cpus: false,
            halt_on_panic: false,
            allow_debug_uart_suspend: false,
        }
    }
}

/// This struct defines platform configurations specified by board.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformSettings {
    /// Configure connectivity related features
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub connectivity: ConnectivityConfig,

    /// Configure development support related features
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub development_support: DevelopmentSupportConfig,

    /// Configure development support related features
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub graphics: GraphicsConfig,

    /// Configure security related features
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub security: SecurityConfig,

    /// Sysmem board defaults. This can be overridden field-by-field by the same
    /// struct in platform config.
    ///
    /// We don't provide format_costs_persistent_fidl files via this struct, as
    /// a BoardInputBundle provides the files via the BoardProvidedConfig
    /// struct.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub sysmem_defaults: BoardSysmemConfig,
}

/// This struct defines security configurations specified by board.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SecurityConfig {
    /// Absolute URL of the board-provided realm of components for proprietary TEE.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tee_realm_url: Option<String>,
}

/// This struct defines connectivity configurations.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ConnectivityConfig {
    /// Configure network related features
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub network: NetworkConfig,
}

/// This struct defines development support configurations.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DevelopmentSupportConfig {
    /// Configure debug access port for specific SoC
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_debug_access_port_for_soc: Option<Arm64DebugDapSoc>,
    /// Specify which network interface in Boot Items we should use to generate the
    /// device node name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_item_interface_for_node_name: Option<u8>,
    /// Flip the low bit of the MAC address we retrieve from boot items. This
    /// mimics a behavior of the CDC ethernet driver and is necessary for
    /// compatibility with it.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub boot_item_interface_flip_low_mac_bit: bool,
}

/// This struct defines network configurations.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    /// This option instructs netsvc to use only the device whose topological
    /// path ends with the option's value, with any wildcard `*` characters
    /// matching any zero or more characters of the topological path. All other
    /// devices are ignored by netsvc. The topological path for a device can be
    /// determined from the shell by running the `lsdev` command on the device
    /// (e.g. `/dev/class/network/000` or `/dev/class/ethernet/000`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub netsvc_interface: Option<String>,
}

/// This struct defines graphics configurations.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GraphicsConfig {
    /// Configure display related features.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub display: DisplayConfig,
}

/// This struct defines display configurations.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DisplayConfig {
    /// The number of degrees to the rotate the screen display by.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation: Option<u32>,

    /// Whether the display has rounded corners.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub rounded_corners: bool,

    /// Fallback horizontal display size in millimeters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_horizontal_size_mm: Option<u32>,

    /// Fallback vertical display size in millimeters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_vertical_size_mm: Option<u32>,
}

#[cfg(test)]
mod test {
    use super::*;
    use camino::Utf8PathBuf;
    use tempfile::tempdir;

    #[test]
    fn test_basic_board_deserialize() {
        let json = serde_json::json!({
            "name": "sample board",
            "arch": "x64",
            "release_info": {
                "info": {
                    "name": "",
                    "repository": "",
                    "version": "",
                },
                "bib_sets": [],
            }
        });

        let parsed: BoardConfig = serde_json::from_value(json).unwrap();
        let expected = BoardConfig { name: "sample board".to_owned(), ..Default::default() };

        assert_eq!(parsed, expected);
    }

    #[test]
    fn test_board_default_serialization() {
        let value: BoardConfig = serde_json::from_str("{\"name\": \"foo\", \"arch\": \"x64\", \"release_info\": {\"info\": { \"name\": \"\", \"repository\": \"\", \"version\": \"\" }, \"bib_sets\": [] }}").unwrap();
        crate::common::tests::value_serialization_helper(value);
    }

    #[test]
    fn test_bib_default_serialization() {
        crate::common::tests::default_serialization_helper::<BoardInputBundle>();
    }

    #[test]
    fn test_board_provided_config_default_serialization() {
        crate::common::tests::default_serialization_helper::<BoardProvidedConfig>();
    }

    #[test]
    fn test_board_kernel_config_default_serialization() {
        crate::common::tests::default_serialization_helper::<BoardKernelConfig>();
    }

    #[test]
    fn test_complete_board_deserialize_with_relative_paths() {
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let config_path = dir_path.join("board_configuration.json");
        let config_file = std::fs::File::create(&config_path).unwrap();

        let devicetree_path = dir_path.join("test.dtb");
        std::fs::write(&devicetree_path, "").unwrap();

        let json = serde_json::json!({
            "name": "sample board",
            "arch": "x64",
            "hardware_info": {
                "name": "hwinfo_name",
                "vendor_id": 1,
                "product_id": 2,
                "revision": 3,
            },
            "provided_features": [
                "feature_a",
                "feature_b"
            ],
            "input_bundles": {},
            "devicetree": "test.dtb",
            "kernel": {
                "contiguous_physical_pages": true,
                "scheduler_prefer_little_cpus": true,
                "arm64_event_stream_enable": false,
            },
            "platform": {
                "development_support": {
                    "enable_debug_access_port_for_soc": "amlogic-t931g",
                }
            },
            "release_info": {
                "info": {
                    "name": "",
                    "repository": "",
                    "version": "",
                },
                "bib_sets": [],
            }
        });
        serde_json::to_writer(config_file, &json).unwrap();
        let resolved = BoardConfig::from_dir(&dir_path).unwrap();

        let expected = BoardConfig {
            name: "sample board".to_owned(),
            hardware_info: HardwareInfo {
                name: Some("hwinfo_name".into()),
                vendor_id: Some(0x01),
                product_id: Some(0x02),
                revision: Some(0x03),
            },
            provided_features: vec!["feature_a".into(), "feature_b".into()],
            input_bundles: [].into(),
            devicetree: Some(devicetree_path),
            devicetree_overlay: None,
            kernel: BoardKernelConfig {
                contiguous_physical_pages: true,
                serial_mode: SerialMode::NoOutput,
                quiet_early_boot: false,
                serial: None,
                scheduler_prefer_little_cpus: true,
                halt_on_panic: false,
                arm64_event_stream_enable: false,
                allow_debug_uart_suspend: false,
            },
            platform: PlatformSettings {
                connectivity: ConnectivityConfig::default(),
                development_support: DevelopmentSupportConfig {
                    enable_debug_access_port_for_soc: Some(Arm64DebugDapSoc::AmlogicT931g),
                    boot_item_interface_for_node_name: None,
                    boot_item_interface_flip_low_mac_bit: false,
                },
                graphics: GraphicsConfig::default(),
                security: SecurityConfig::default(),
                sysmem_defaults: BoardSysmemConfig::default(),
            },
            ..Default::default()
        };

        assert_eq!(resolved, expected);
    }

    #[test]
    fn test_merge_board_input_bundle_sets() {
        let mut board = BoardConfig {
            input_bundles: [
                ("myset::mybib".to_string(), DirectoryPathBuf::new("path/to/old/bib".into())),
                (
                    "otherset::otherbib".to_string(),
                    DirectoryPathBuf::new("path/to/other/bib".into()),
                ),
                (
                    "independent_bib".to_string(),
                    DirectoryPathBuf::new("path/to/independent/bib".into()),
                ),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };

        let bib_set = crate::BoardInputBundleSet {
            name: "myset".to_string(),
            board_input_bundles: [(
                "mybib".to_string(),
                crate::BoardInputBundleEntry {
                    path: DirectoryPathBuf::new("path/to/new/bib".into()),
                },
            )]
            .into_iter()
            .collect(),
            release_info: Default::default(),
        };

        board.merge_board_input_bundle_sets(vec![bib_set]);

        assert_eq!(
            board.input_bundles.get("myset::mybib").unwrap(),
            &DirectoryPathBuf::new("path/to/new/bib".into())
        );
        assert_eq!(
            board.input_bundles.get("otherset::otherbib").unwrap(),
            &DirectoryPathBuf::new("path/to/other/bib".into())
        );
        assert_eq!(
            board.input_bundles.get("independent_bib").unwrap(),
            &DirectoryPathBuf::new("path/to/independent/bib".into())
        );
    }

    #[test]
    fn test_deserialize_historical_board() {
        use assembly_partitions_config::PartitionsConfig;

        let test_data_dir =
            camino::Utf8PathBuf::from(env!("TEST_DATA_DIR")).join("test_data/32.99991231.0.1");

        // Load BoardConfig
        let board_config = BoardConfig::from_dir(&test_data_dir).unwrap();
        assert_eq!(board_config.name, "x64");

        // Load PartitionsConfig from the resolved path
        let resolved_partitions_path =
            board_config.partitions_config.as_ref().unwrap().as_utf8_path_buf();
        let partitions_config = PartitionsConfig::from_dir(resolved_partitions_path).unwrap();
        assert_eq!(partitions_config.hardware_revision, "x64");
    }

    #[test]
    fn test_deserialize_maximal_board() {
        use assembly_partitions_config::PartitionsConfig;

        let test_data_dir =
            camino::Utf8PathBuf::from(env!("TEST_DATA_DIR")).join("test_data/maximal_board");

        // Load BoardConfig
        let board_config = BoardConfig::from_dir(&test_data_dir).unwrap();
        assert_eq!(board_config.name, "maximal_board");
        assert_eq!(board_config.arch, Architecture::ARM64);

        // Verify some fields to ensure they parsed correctly
        assert_eq!(board_config.hardware_info.name, Some("maximal_hw".into()));
        assert_eq!(board_config.hardware_info.vendor_id, Some(42));
        assert_eq!(board_config.hardware_info.product_id, Some(43));
        assert_eq!(board_config.hardware_info.revision, Some(44));

        assert_eq!(
            board_config.provided_features,
            vec!["feature1".to_string(), "feature2".to_string()]
        );

        // Load PartitionsConfig from the resolved path
        let resolved_partitions_path =
            board_config.partitions_config.as_ref().unwrap().as_utf8_path_buf();
        let partitions_config = PartitionsConfig::from_dir(resolved_partitions_path).unwrap();
        assert_eq!(partitions_config.hardware_revision, "x64"); // copied from x64
    }
}
