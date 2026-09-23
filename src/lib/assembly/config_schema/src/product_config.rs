// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::platform_settings::PlatformSettings;
use anyhow::Result;
use assembly_container::{AssemblyContainer, WalkPaths, assembly_container};
use camino::Utf8PathBuf;
use fuchsia_pkg::PackageManifest;
use product_input_bundle::ProductInputBundle;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::product_settings::{ProductPackageDetails, ProductSettings};

/// Configuration for a Product Assembly operation.
///
/// This is a high-level operation that takes a more abstract description of
/// what is desired in the assembled product images, and then generates the
/// complete Image Assembly configuration (`ImageProductConfig`) from that.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
#[assembly_container(product_configuration.json)]
pub struct ProductConfig {
    #[walk_paths]
    pub platform: PlatformSettings,
    #[walk_paths]
    pub product: ProductSettings,
    #[walk_paths]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(skip)]
    pub product_input_bundles: BTreeMap<String, ProductInputBundle>,
}

impl ProductConfig {
    pub fn add_package_names(mut self) -> Result<Self> {
        self.product.packages.base = Self::add_package_names_to_set(self.product.packages.base)?;
        self.product.packages.cache = Self::add_package_names_to_set(self.product.packages.cache)?;
        Ok(self)
    }

    fn add_package_names_to_set(
        set: BTreeMap<String, ProductPackageDetails>,
    ) -> Result<BTreeMap<String, ProductPackageDetails>> {
        set.into_values()
            .map(|pkg| {
                let manifest = PackageManifest::try_load_from(&pkg.manifest)?;
                let name = manifest.name().to_string();
                Ok((name, pkg))
            })
            .collect()
    }

    pub fn find_package_in_product(
        &mut self,
        package_name: impl AsRef<str>,
    ) -> Option<&mut Utf8PathBuf> {
        self.product.packages.base.iter_mut().chain(&mut self.product.packages.cache).find_map(
            |(name, pkg)| {
                if name == package_name.as_ref() {
                    return Some(&mut pkg.manifest);
                }
                None
            },
        )
    }

    pub fn find_package_in_pibs(
        product_input_bundles: &BTreeMap<String, ProductInputBundle>,
        package_name: impl AsRef<str>,
    ) -> Option<&Utf8PathBuf> {
        product_input_bundles.values().find_map(|pib| {
            pib.packages.for_product_config.get(package_name.as_ref()).map(|pkg| &pkg.manifest)
        })
    }

    pub fn find_hals_in_pibs(
        product_input_bundles: &BTreeMap<String, ProductInputBundle>,
        hal_names: &[String],
    ) -> Result<Vec<Utf8PathBuf>> {
        hal_names
            .iter()
            .map(|name| {
                let hal_package = Self::find_package_in_pibs(product_input_bundles, name)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "finding starnix hal package '{}' in product input bundles",
                            name
                        )
                    })?;
                Ok(hal_package.clone())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly_input_bundle::{AssemblyInputBundle, PartialKernelConfig};
    use crate::common::{DriverDetails, PackageDetails, PackagedDriverDetails};
    use crate::platform_settings::media_config::{
        AudioConfig, AudioDeviceRegistryConfig, PlatformMediaConfig,
    };
    use crate::platform_settings::{BuildType, FeatureSetLevel};
    use crate::product_settings::{
        ProductPackageDetails, StarnixContainerConfig, StarnixImages, StarnixImagesOrPackage,
    };
    use assembly_constants::FileEntry;
    use assembly_package_utils::PackageInternalPathBuf;
    use assembly_util as util;
    use camino::Utf8PathBuf;
    use fuchsia_pkg::{MetaPackage, PackageManifestBuilder, PackageName};
    use std::collections::BTreeSet;
    use std::str::FromStr;
    use tempfile::tempdir;

    #[test]
    fn test_product_assembly_config_from_json5() {
        let json5 = r#"
        {
          platform: {
            build_type: "eng",
          },
          product: {},
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: ProductConfig = util::from_reader(&mut cursor).unwrap();
        let platform = config.platform;
        assert_eq!(platform.build_type, BuildType::Eng);
        assert_eq!(platform.feature_set_level, FeatureSetLevel::Standard);
    }

    #[test]
    fn test_bringup_product_assembly_config_from_json5() {
        let json5 = r#"
        {
          platform: {
            feature_set_level: "bootstrap",
            build_type: "eng",
          },
          product: {},
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: ProductConfig = util::from_reader(&mut cursor).unwrap();
        let platform = config.platform;
        assert_eq!(platform.build_type, BuildType::Eng);
        assert_eq!(platform.feature_set_level, FeatureSetLevel::Bootstrap);
    }

    #[test]
    fn test_minimal_product_assembly_config_from_json5() {
        let json5 = r#"
        {
          platform: {
            build_type: "eng",
          },
          product: {},
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: ProductConfig = util::from_reader(&mut cursor).unwrap();
        let platform = config.platform;
        assert_eq!(platform.build_type, BuildType::Eng);
        assert_eq!(platform.feature_set_level, FeatureSetLevel::Standard);
    }

    #[test]
    fn test_buildtype_deserialization_userdebug() {
        let json5 = r#"
        {
          platform: {
            build_type: "userdebug",
          },
          product: {},
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: ProductConfig = util::from_reader(&mut cursor).unwrap();
        let platform = config.platform;
        assert_eq!(platform.build_type, BuildType::UserDebug);
    }

    #[test]
    fn test_buildtype_deserialization_user() {
        let json5 = r#"
        {
          platform: {
            build_type: "user",
          },
          product: {},
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: ProductConfig = util::from_reader(&mut cursor).unwrap();
        let platform = config.platform;
        assert_eq!(platform.build_type, BuildType::User);
    }

    #[test]
    fn test_product_assembly_config_with_product_provided_parts() {
        let json5 = r#"
        {
          platform: {
            build_type: "eng",
          },
          product: {
              packages: {
                  base: [
                      { manifest: "path/to/base/package_manifest.json" }
                  ],
                  cache: [
                      { manifest: "path/to/cache/package_manifest.json" }
                  ],
              },
              base_drivers: [
                {
                  package: "path/to/base/driver/package_manifest.json",
                  components: [ "meta/path/to/component.cml" ]
                }
              ],
              starnix_containers: [
                {
                  name: "wear_os",
                  name: "wear_os",
                  base: "container_base",
                  hals: [
                    "hal_a",
                    "hal_b",
                  ],
                  skip_subpackages: true,
                  images_or_package:
                    {
                      images: {
                        system: "path/to/system.img",
                        vendor: "path/to/vendor.img",
                        ramdisk: ["path/to/ramdisk.img"],
                      },
                    },
                  fstab: "",
                  init: [],
                }
              ],
          },
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: ProductConfig = util::from_reader(&mut cursor).unwrap();
        let platform = config.platform;
        assert_eq!(platform.build_type, BuildType::Eng);
        assert_eq!(
            config.product.packages.base,
            [(
                "0".to_string(),
                ProductPackageDetails {
                    manifest: "path/to/base/package_manifest.json".into(),
                    config_data: Vec::default()
                }
            )]
            .into()
        );
        assert_eq!(
            config.product.packages.cache,
            [(
                "0".to_string(),
                ProductPackageDetails {
                    manifest: "path/to/cache/package_manifest.json".into(),
                    config_data: Vec::default()
                }
            )]
            .into()
        );
        let expected_drivers: Vec<DriverDetails> = serde_json::from_value(serde_json::json!([
            {
                "package": "path/to/base/driver/package_manifest.json",
                "components": [ "meta/path/to/component.cml" ]
            }
        ]))
        .unwrap();
        assert_eq!(config.product.base_drivers, expected_drivers);
        assert_eq!(
            config.product.starnix_containers,
            vec![StarnixContainerConfig {
                name: "wear_os".to_string(),
                base: "container_base".to_string(),
                skip_subpackages: true,
                fstab: Some("".into()),
                init: vec![],
                hals: vec!["hal_a".to_string(), "hal_b".to_string()],
                images_or_package: StarnixImagesOrPackage::Images(StarnixImages {
                    system: "path/to/system.img".into(),
                    vendor: Some("path/to/vendor.img".into()),
                    ramdisk: vec![("path/to/ramdisk.img".into()),],
                }),
                file_overrides: vec![],
            }]
        )
    }

    #[test]
    fn test_product_assembly_config_with_relative_paths() {
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let config_path = dir_path.join("product_configuration.json");
        let config_file = std::fs::File::create(&config_path).unwrap();

        let index_path = dir_path.join("component_id_index.json");
        std::fs::write(&index_path, "").unwrap();

        let json = serde_json::json!({
          "platform": {
            "build_type": "eng",
            "storage": {
              "component_id_index": {
                "product_index": "component_id_index.json",
              },
            },
          },
        });
        serde_json::to_writer(config_file, &json).unwrap();
        let config = ProductConfig::from_dir(&dir_path).unwrap();

        assert_eq!(index_path, config.platform.storage.component_id_index.product_index.unwrap());
    }

    #[test]
    fn test_assembly_input_bundle_from_json5() {
        let json5 = r#"
            {
              // json5 files can have comments in them.
              packages: [
                {
                    package: "package5",
                    set: "base",
                },
                {
                    package: "package6",
                    set: "cache",
                },
              ],
              kernel: {
                path: "path/to/kernel",
                args: ["arg1", "arg2"],
              },
              // and lists can have trailing commas
              boot_args: ["arg1", "arg2", ],
              bootfs_files: [
                {
                  source: "path/to/source",
                  destination: "path/to/destination",
                }
              ],
              config_data: {
                "package1": [
                  {
                    source: "path/to/source.json",
                    destination: "config.json"
                  }
                ]
              },
              drivers: [
                {
                  package: "path/to/driver",
                  set: "base",
                  components: ["path/to/1234", "path/to/5678"]
                }
              ],
              shell_commands: {
                "package1": ["path/to/binary1", "path/to/binary2"]
              },
              packages_to_compile: [
                  {
                    name: "core",
                    components: [
                      {
                        component_name: "component1",
                        shards: ["path/to/component1.cml"],
                        cmc_features: [],
                      },
                      {
                        component_name: "component2",
                        shards: ["path/to/component2.cml"],
                        cmc_features: [],
                      },
                    ],
                    contents: [
                        {
                            source: "path/to/source",
                            destination: "path/to/destination",
                        }
                    ],
                    includes: [ "src/path/to/include.cml" ]
                },
              ],
              memory_buckets: [
                "path/to/buckets.json",
              ],
            }
        "#;
        let bundle =
            util::from_reader::<_, AssemblyInputBundle>(&mut std::io::Cursor::new(json5)).unwrap();
        let expected_packages: Vec<PackageDetails> = serde_json::from_value(serde_json::json!([
            {
                "package": "package5",
                "set": "base"
            },
            {
                "package": "package6",
                "set": "cache"
            }
        ]))
        .unwrap();
        assert_eq!(bundle.packages, expected_packages);
        let expected_kernel = PartialKernelConfig {
            path: Some(Utf8PathBuf::from("path/to/kernel")),
            args: vec!["arg1".to_string(), "arg2".to_string()],
        };
        assert_eq!(bundle.kernel, Some(expected_kernel));
        assert_eq!(bundle.boot_args, vec!("arg1".to_string(), "arg2".to_string()));
        assert_eq!(
            bundle.bootfs_files,
            vec!(FileEntry {
                source: Utf8PathBuf::from("path/to/source"),
                destination: "path/to/destination".to_string()
            })
        );
        assert_eq!(
            bundle.config_data.get("package1").unwrap(),
            &vec!(FileEntry {
                source: Utf8PathBuf::from("path/to/source.json"),
                destination: "config.json".to_string()
            })
        );
        let expected_driver: PackagedDriverDetails = serde_json::from_value(serde_json::json!(
            {
                "package": "path/to/driver",
                "set": "base",
                "components": ["path/to/1234", "path/to/5678"]
            }
        ))
        .unwrap();
        assert_eq!(bundle.drivers[0], expected_driver);
        assert_eq!(
            bundle.shell_commands.get("package1").unwrap(),
            &BTreeSet::from([
                PackageInternalPathBuf::from("path/to/binary1"),
                PackageInternalPathBuf::from("path/to/binary2"),
            ])
        );
        assert_eq!(bundle.memory_buckets, vec![Utf8PathBuf::from("path/to/buckets.json")]);
    }

    #[test]
    fn test_assembly_config_wrapper_for_overrides() {
        let config: ProductConfig = serde_json::from_value(serde_json::json!({
            "platform": {
                "build_type": "eng",
            },
            "product": {},
        }))
        .unwrap();

        let overrides = serde_json::json!({
            "platform": {
                "media": {
                    "audio": {
                      "device_registry": {},
                    },
                },
            },
        });

        let config = config.apply_overrides(overrides).unwrap();

        assert_eq!(
            config.platform.media,
            PlatformMediaConfig {
                audio: Some(AudioConfig::DeviceRegistry(AudioDeviceRegistryConfig {
                    eager_start: false,
                    ignore_devices: false,
                })),
                ..Default::default()
            },
        );
    }

    #[test]
    fn test_get_package_names() {
        // Prepare a directory for temporary files.
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        // Generate a fake package manifest.
        let package_name = PackageName::from_str("my_pkg").unwrap();
        let meta_package = MetaPackage::from_name_and_variant_zero(package_name);
        let package_manifest_builder = PackageManifestBuilder::new(meta_package);
        let package_manifest = package_manifest_builder.build();
        let package_manifest_path = dir_path.join("my_pkg_package_manifest.json");
        package_manifest.write_with_relative_paths(&package_manifest_path).unwrap();

        // Create an assembly config with the package manifest.
        let json5 = r#"
        {
          platform: {
            build_type: "eng",
          },
        }
        "#;
        let mut cursor = std::io::Cursor::new(json5);
        let mut config: ProductConfig = util::from_reader(&mut cursor).unwrap();
        config.product.packages.base.insert(
            "0".to_string(),
            ProductPackageDetails { manifest: package_manifest_path.clone(), config_data: vec![] },
        );

        // Test the logic to add proper package names.
        let config = config.add_package_names().unwrap();
        let details = config.product.packages.base.get("my_pkg").unwrap();
        assert_eq!(&details.manifest, &package_manifest_path);
    }
}
