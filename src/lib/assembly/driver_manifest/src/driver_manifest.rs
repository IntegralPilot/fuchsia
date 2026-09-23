// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use assembly_config_capabilities::{Config, ConfigNestedValueType, ConfigValueType};
use assembly_config_schema::DriverDetails;
use camino::Utf8Path;
use fuchsia_pkg::PackageManifest;

/// Possible driver package.
#[derive(Clone, Copy)]
pub enum DriverPackageType {
    /// A base-driver package
    Base,
    /// A boot-driver package
    Boot,
}

/// A builder for the driver manifest package.
#[derive(Default)]
pub struct DriverManifestBuilder {
    drivers: Vec<String>,
}

impl DriverManifestBuilder {
    /// Add a driver manifest fragment to the driver manifest.
    pub fn add_driver(&mut self, driver_details: DriverDetails, package_url: &str) -> Result<()> {
        let driver_manifests = driver_details
            .components
            .iter()
            .map(|component_path| format!("{}#{}", package_url, component_path))
            .collect::<Vec<String>>();

        self.drivers.extend(driver_manifests);
        Ok(())
    }

    /// Encode driver manifest as a config capability.
    pub fn create_config(&self) -> Config {
        Config::new(
            ConfigValueType::Vector {
                nested_type: ConfigNestedValueType::String { max_size: 150 },
                max_count: 200,
            },
            self.drivers.clone().into(),
        )
    }

    /// Helper function to determine a driver's package url
    pub fn get_package_url(
        package_type: DriverPackageType,
        path: impl AsRef<Utf8Path>,
    ) -> Result<String> {
        // Load the PackageManifest from the given path
        let manifest = PackageManifest::try_load_from(&path).with_context(|| {
            format!("parsing driver package {} as a package manifest", path.as_ref())
        })?;
        match package_type {
            DriverPackageType::Base => {
                let repository = manifest.repository().unwrap_or("fuchsia.com");
                Ok(format!("fuchsia-pkg://{}/{}", repository, manifest.name()))
            }
            DriverPackageType::Boot => Ok(format!("fuchsia-boot:///{}", manifest.name())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_test_util::generate_test_manifest;
    use camino::Utf8PathBuf;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn create_config() -> Result<()> {
        let tmp = TempDir::new()?;
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();

        std::fs::create_dir(outdir.join("driver"))?;
        let driver_package_manifest_file_path = outdir.join("driver/package_manifest.json");
        let mut driver_package_manifest_file = File::create(&driver_package_manifest_file_path)?;
        let package_manifest = generate_test_manifest("base_driver", None);
        serde_json::to_writer(&driver_package_manifest_file, &package_manifest)?;
        driver_package_manifest_file.flush()?;

        let driver_details = DriverDetails {
            package: driver_package_manifest_file_path.to_owned(),
            components: vec![Utf8PathBuf::from("meta/foobar.cm")],
        };
        let mut driver_manifest_builder = DriverManifestBuilder::default();
        driver_manifest_builder.add_driver(
            driver_details,
            &DriverManifestBuilder::get_package_url(
                DriverPackageType::Base,
                driver_package_manifest_file_path,
            )?,
        )?;

        let config = driver_manifest_builder.create_config();

        assert_eq!(
            Config::new(
                ConfigValueType::Vector {
                    nested_type: ConfigNestedValueType::String { max_size: 150 },
                    max_count: 200,
                },
                vec!["fuchsia-pkg://testrepository.com/base_driver#meta/foobar.cm"].into(),
            ),
            config
        );

        Ok(())
    }
}
