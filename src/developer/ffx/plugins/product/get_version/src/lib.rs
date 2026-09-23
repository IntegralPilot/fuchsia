// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! FFX plugin for the product version of a Product Bundle.

mod load_config;
mod unique_release_info;

use crate::load_config::{
    VersionInfoWithDependencies, load_bib_set, load_board, load_pibs, load_platform, load_product,
    load_product_bundle_v2,
};
use crate::unique_release_info::UniqueReleaseInfoVector;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{FfxContext, FfxMain, FfxTool};
use product_bundle::ProductBundle;

mod args;
pub use args::GetVersionCommand;

/// This plugin will get the the product version of a Product Bundle.
#[derive(FfxTool)]
pub struct PbGetVersionTool {
    #[command]
    cmd: GetVersionCommand,
}

#[async_trait(?Send)]
impl FfxMain for PbGetVersionTool {
    type Writer = MachineWriter<UniqueReleaseInfoVector>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let artifact_path = &self.cmd.artifact;
        let info: VersionInfoWithDependencies =
            if let Ok(product_bundle) = ProductBundle::try_load_from(artifact_path) {
                match product_bundle {
                    ProductBundle::V2(pb) => load_product_bundle_v2(&pb),
                }
            } else if artifact_path.join("platform_artifacts.json").exists() {
                load_platform(artifact_path)?.into_version_with_deps()
            } else if artifact_path.join("product_configuration.json").exists() {
                load_product(artifact_path)?.into_version_with_deps()
            } else if artifact_path.join("product_input_bundle.json").exists() {
                load_pibs(artifact_path)?.into_version_with_deps()
            } else if artifact_path.join("board_configuration.json").exists() {
                load_board(artifact_path)?
            } else if artifact_path.join("board_input_bundle_set.json").exists() {
                load_bib_set(artifact_path)?.into_version_with_deps()
            } else {
                return Err(fho::Error::User(anyhow!("error parsing the artifact type")));
            };

        if self.cmd.include_dependencies {
            writer.machine(&UniqueReleaseInfoVector(info.version_with_deps.machine)).bug()?;
            writer.line(&info.version_with_deps.human).bug()?;
        } else {
            writer.machine(&UniqueReleaseInfoVector(info.version.machine)).bug()?;
            writer.line(&info.version.human).bug()?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::load_config::VersionInfo;

    use assembly_partitions_config::{PartitionsConfig, Slot};
    use assembly_release_info::{
        BoardReleaseInfo, ProductBundleReleaseInfo, ProductReleaseInfo, ReleaseInfo,
        SystemReleaseInfo,
    };
    use camino::Utf8Path;
    use ffx_writer::{Format, MachineWriter, TestBuffers};
    use product_bundle::ProductBundleV2;
    use unique_release_info::UniqueReleaseInfo;

    fn generate_test_product_bundle() -> ProductBundleV2 {
        ProductBundleV2 {
            product_name: "fake_name".to_string(),
            product_version: "fake_version".to_string(),
            sdk_version: "fake_sdk_version".to_string(),
            partitions: PartitionsConfig::default(),
            system_a: None,
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: Some(ProductBundleReleaseInfo {
                name: "fake_name".to_string(),
                version: "fake_version".to_string(),
                sdk_version: "fake_version".to_string(),
                system_a: Some(SystemReleaseInfo {
                    platform: ReleaseInfo {
                        name: "fake_platform".to_string(),
                        repository: "fake_repository_for_platform".to_string(),
                        version: "fake_version_for_platform".to_string(),
                    },
                    product: ProductReleaseInfo {
                        info: ReleaseInfo {
                            name: "fake_product".to_string(),
                            repository: "fake_repository_for_product".to_string(),
                            version: "fake_version_for_product".to_string(),
                        },
                        pibs: vec![
                            ReleaseInfo {
                                name: "fake_example_pib_1".to_string(),
                                repository: "fake_repository_for_product".to_string(),
                                version: "fake_version_for_product".to_string(),
                            },
                            ReleaseInfo {
                                name: "fake_example_pib_2".to_string(),
                                repository: "fake_repository_for_product".to_string(),
                                version: "fake_version_for_product".to_string(),
                            },
                        ],
                    },
                    board: BoardReleaseInfo {
                        info: ReleaseInfo {
                            name: "fake_board".to_string(),
                            repository: "fake_repository_for_board".to_string(),
                            version: "fake_version_for_board".to_string(),
                        },
                        bib_sets: vec![
                            ReleaseInfo {
                                name: "fake_example_bib_set_1".to_string(),
                                repository: "fake_repository_for_board".to_string(),
                                version: "fake_version_for_board".to_string(),
                            },
                            ReleaseInfo {
                                name: "fake_example_bib_set_2".to_string(),
                                repository: "fake_repository_for_board".to_string(),
                                version: "fake_version_for_board".to_string(),
                            },
                            ReleaseInfo {
                                name: "fake_example_bib_set_3".to_string(),
                                repository: "fake_repository_for_board".to_string(),
                                version: "fake_version_for_board".to_string(),
                            },
                        ],
                    },
                }),
                system_b: None,
                system_r: Some(SystemReleaseInfo {
                    platform: ReleaseInfo {
                        name: "fake_platform".to_string(),
                        repository: "fake_repository_for_platform".to_string(),
                        version: "fake_version_for_platform".to_string(),
                    },
                    // This is the same product as "fake_product" in Slot A (system_a) defined
                    // above. We expect both product entries to be combined into a single
                    // UniqueReleaseInfo entry in the result.
                    product: ProductReleaseInfo {
                        info: ReleaseInfo {
                            name: "fake_product".to_string(),
                            repository: "fake_repository_for_product".to_string(),
                            version: "fake_version_for_product".to_string(),
                        },
                        pibs: vec![],
                    },
                    // This is the same board as "fake_board" in Slot A (system_a) defined
                    // above. We expect both board entries to be combined into a single
                    // UniqueReleaseInfo entry in the result.
                    board: BoardReleaseInfo {
                        info: ReleaseInfo {
                            name: "fake_board".to_string(),
                            repository: "fake_repository_for_board".to_string(),
                            version: "fake_version_for_board".to_string(),
                        },
                        bib_sets: vec![],
                    },
                }),
            }),
        }
    }

    fn generate_test_pb_flat_unique_release_info_vector() -> Vec<UniqueReleaseInfo> {
        vec![
            UniqueReleaseInfo::new(
                "fake_platform".to_string(),
                "fake_version_for_platform".to_string(),
                "fake_repository_for_platform".to_string(),
                vec![Slot::A, Slot::R],
                "platform".to_string(),
            ),
            UniqueReleaseInfo::new(
                "fake_product".to_string(),
                "fake_version_for_product".to_string(),
                "fake_repository_for_product".to_string(),
                vec![Slot::A, Slot::R],
                "product".to_string(),
            ),
            UniqueReleaseInfo::new(
                "fake_example_pib_1".to_string(),
                "fake_version_for_product".to_string(),
                "fake_repository_for_product".to_string(),
                vec![Slot::A],
                "pib".to_string(),
            ),
            UniqueReleaseInfo::new(
                "fake_example_pib_2".to_string(),
                "fake_version_for_product".to_string(),
                "fake_repository_for_product".to_string(),
                vec![Slot::A],
                "pib".to_string(),
            ),
            UniqueReleaseInfo::new(
                "fake_board".to_string(),
                "fake_version_for_board".to_string(),
                "fake_repository_for_board".to_string(),
                vec![Slot::A, Slot::R],
                "board".to_string(),
            ),
            UniqueReleaseInfo::new(
                "fake_example_bib_set_1".to_string(),
                "fake_version_for_board".to_string(),
                "fake_repository_for_board".to_string(),
                vec![Slot::A],
                "bib".to_string(),
            ),
            UniqueReleaseInfo::new(
                "fake_example_bib_set_2".to_string(),
                "fake_version_for_board".to_string(),
                "fake_repository_for_board".to_string(),
                vec![Slot::A],
                "bib".to_string(),
            ),
            UniqueReleaseInfo::new(
                "fake_example_bib_set_3".to_string(),
                "fake_version_for_board".to_string(),
                "fake_repository_for_board".to_string(),
                vec![Slot::A],
                "bib".to_string(),
            ),
        ]
    }

    #[test]
    fn test_version_string_writer() {
        let pb = generate_test_product_bundle();
        let version_info = load_product_bundle_v2(&pb);
        assert_eq!("fake_version", version_info.version.human);

        let test_buffers = TestBuffers::default();
        let mut writer: MachineWriter<VersionInfo> = MachineWriter::new_test(None, &test_buffers);

        // Write the version string.
        let res = writer.line(version_info.version);
        assert!(res.is_ok());

        // Write the json structure. This should be suppressed.
        let res = writer.machine(&version_info.version_with_deps);
        assert!(res.is_ok());

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!("fake_version\n", stdout);
        assert_eq!("", stderr);
    }

    #[test]
    fn test_machine_json_writer() {
        let mut expected = generate_test_pb_flat_unique_release_info_vector();
        let pb = generate_test_product_bundle();
        let version_info = load_product_bundle_v2(&pb);

        expected.sort();
        let mut release_info = version_info.version_with_deps;
        release_info.machine.sort();
        assert_eq!(expected, release_info.machine);

        let test_buffers = TestBuffers::default();
        let mut writer: MachineWriter<VersionInfo> =
            MachineWriter::new_test(Some(Format::Json), &test_buffers);

        // Write the version string. This should be suppressed.
        let res = writer.line(version_info.version);
        assert!(res.is_ok());

        // Write the json structure.
        let res = writer.machine(&release_info);
        assert!(res.is_ok());

        let (stdout, stderr) = test_buffers.into_strings();

        // The stdout string needs to be un-escaped, which can be done using serde.
        let mut actual: VersionInfo = serde_json::from_str(&stdout).unwrap();

        actual.machine.sort();
        assert_eq!(expected, actual.machine);
        assert_eq!(stderr, "");
    }

    #[test]
    fn test_load_platform() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(tmp.path()).unwrap();
        let platform_artifacts_path = path.join("platform_artifacts.json");
        let mut file = std::fs::File::create(platform_artifacts_path).unwrap();
        serde_json::to_writer(
            &mut file,
            &serde_json::json!(
                {
                    // This verifies it can handle config changes.
                    "new_field": "some_value",
                    "release_info": {
                        "name": "fake_platform",
                        "repository": "fake_repository_for_platform",
                        "version": "fake_version_for_platform"
                    }
                }
            ),
        )
        .unwrap();

        let info = load_platform(&path).unwrap();
        assert_eq!(info.human, "fake_version_for_platform");
        assert_eq!(
            info.machine[0],
            UniqueReleaseInfo::new(
                "fake_platform".to_string(),
                "fake_version_for_platform".to_string(),
                "fake_repository_for_platform".to_string(),
                vec![],
                "platform".to_string(),
            )
        );
    }

    #[test]
    fn test_load_product() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(tmp.path()).unwrap();
        let product_configuration_path = path.join("product_configuration.json");
        let mut file = std::fs::File::create(product_configuration_path).unwrap();
        serde_json::to_writer(
            &mut file,
            &serde_json::json!(
                {
                    // This verifies it can handle config changes.
                    "new_field": "some_value",
                    "product": {
                        "release_info": {
                            "info": {
                                "name": "fake_product",
                                "repository": "fake_repository_for_product",
                                "version": "fake_version_for_product"
                            },
                            "pibs": [
                                {
                                    "name": "fake_example_pib_1",
                                    "repository": "fake_repository_for_product",
                                    "version": "fake_version_for_product"
                                },
                                {
                                    "name": "fake_example_pib_2",
                                    "repository": "fake_repository_for_product",
                                    "version": "fake_version_for_product"
                                }
                            ]
                        }
                    }
                }
            ),
        )
        .unwrap();

        let info = load_product(&path).unwrap();
        assert_eq!(info.human, "fake_version_for_product");
        assert_eq!(
            info.machine[0],
            UniqueReleaseInfo::new(
                "fake_product".to_string(),
                "fake_version_for_product".to_string(),
                "fake_repository_for_product".to_string(),
                vec![],
                "product".to_string(),
            )
        );
    }

    #[test]
    fn test_load_pibs() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(tmp.path()).unwrap();
        let product_input_bundle_path = path.join("product_input_bundle.json");
        let mut file = std::fs::File::create(product_input_bundle_path).unwrap();
        serde_json::to_writer(
            &mut file,
            &serde_json::json!(
                {
                    // This verifies it can handle config changes.
                    "new_field": "some_value",
                    "release_info": {
                        "name": "fake_pib",
                        "repository": "fake_repository_for_pib",
                        "version": "fake_version_for_pib"
                    }
                }
            ),
        )
        .unwrap();

        let info = load_pibs(&path).unwrap();
        assert_eq!(info.human, "fake_version_for_pib");
        assert_eq!(
            info.machine[0],
            UniqueReleaseInfo::new(
                "fake_pib".to_string(),
                "fake_version_for_pib".to_string(),
                "fake_repository_for_pib".to_string(),
                vec![],
                "pib".to_string(),
            )
        );
    }

    #[test]
    fn test_load_board() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(tmp.path()).unwrap();
        let board_configuration_path = path.join("board_configuration.json");
        let mut file = std::fs::File::create(board_configuration_path).unwrap();
        serde_json::to_writer(
            &mut file,
            &serde_json::json!(
                {
                    // This verifies it can handle config changes.
                    "new_field": "some_value",
                    "release_info": {
                        "info": {
                            "name": "fake_board",
                            "repository": "fake_repository_for_board",
                            "version": "fake_version_for_board"
                        },
                        "bib_sets": [
                            {
                                "name": "fake_example_bib_set_1",
                                "repository": "fake_repository_for_board",
                                "version": "fake_version_for_board"
                            },
                            {
                                "name": "fake_example_bib_set_2",
                                "repository": "fake_repository_for_board",
                                "version": "fake_version_for_board"
                            }
                        ]
                    }
                }
            ),
        )
        .unwrap();

        let info = load_board(&path).unwrap();
        assert_eq!(info.version.human, "fake_version_for_board");
        assert_eq!(
            info.version.machine[0],
            UniqueReleaseInfo::new(
                "fake_board".to_string(),
                "fake_version_for_board".to_string(),
                "fake_repository_for_board".to_string(),
                vec![],
                "board".to_string(),
            )
        );
    }

    #[test]
    fn test_load_bib_set() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(tmp.path()).unwrap();
        let board_input_bundle_path = path.join("board_input_bundle_set.json");
        let mut file = std::fs::File::create(board_input_bundle_path).unwrap();
        serde_json::to_writer(
            &mut file,
            &serde_json::json!(
                {
                    // This verifies it can handle config changes.
                    "new_field": "some_value",
                    "release_info": {
                        "name": "fake_bib_set",
                        "repository": "fake_repository_for_bib_set",
                        "version": "fake_version_for_bib_set"
                    }
                }
            ),
        )
        .unwrap();

        let info = load_bib_set(&path).unwrap();
        assert_eq!(info.human, "fake_version_for_bib_set");
        assert_eq!(
            info.machine[0],
            UniqueReleaseInfo::new(
                "fake_bib_set".to_string(),
                "fake_version_for_bib_set".to_string(),
                "fake_repository_for_bib_set".to_string(),
                vec![],
                "bib_set".to_string(),
            )
        );
    }
}
