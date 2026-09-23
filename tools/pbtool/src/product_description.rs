// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Generate a json description file for a product bundle.
//!
//! E.g.
//! ```
//! {
//!   "schema_version": "1",
//!   "abi": "0ABC1DEF",  # not yet implemented.
//!   "platform_version": "0.20221213.2.1",  # not yet implemented.
//!   "product_name": "workstation",
//!   "product_version": "2",
//!   "sdk_version": "10.23434342",
//!   "transfer_url": "gs://fuchsia-artifacts-release/builds/8794953117502721265/transfer.json"
//! }
//! ```

use anyhow::{Context, Result};
use argh::FromArgs;
use camino::Utf8PathBuf;
use product_bundle::ProductBundle;
use product_description::{ProductDescription, ProductDescriptionV1};
use std::fs::File;

/// Generate a description using the specified `args`.
#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "generate-description")]
pub struct GenerateProductDescription {
    /// path to a product bundle.
    #[argh(option)]
    product_bundle: Utf8PathBuf,

    /// file path to the write the product description.
    #[argh(option)]
    out_file: Utf8PathBuf,

    /// location the transfer manifest is uploaded to.
    #[argh(option)]
    transfer_url: String,
}

impl GenerateProductDescription {
    pub fn generate(self) -> Result<()> {
        let product_bundle =
            ProductBundle::try_load_from(&self.product_bundle).with_context(|| {
                format!("loading {:?} for GenerateProductDescription", self.product_bundle)
            })?;
        let product_bundle = match product_bundle {
            ProductBundle::V2(pb) => pb,
        };

        // Ensure the `out_file` parent exists.
        let out_dir =
            self.out_file.parent().expect("product description file must have a parent directory.");
        std::fs::create_dir_all(&out_dir)
            .with_context(|| format!("Creating the out_dir: {}", out_dir))?;

        let description = ProductDescription::V1(ProductDescriptionV1 {
            product_name: product_bundle.product_name.to_string(),
            product_version: product_bundle.product_version.to_string(),
            sdk_version: Some(product_bundle.sdk_version),
            transfer_url: self.transfer_url.to_string(),
            ..Default::default()
        });
        let description_file = File::create(self.out_file).context("Creating PB description")?;
        serde_json::to_writer(description_file, &description).context("Writing PB description")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assembly_partitions_config::PartitionsConfig;
    use camino::Utf8Path;
    use product_bundle::ProductBundleV2;
    use tempfile::tempdir;

    #[fuchsia::test]
    async fn test_generate() {
        let tmp = tempdir().unwrap();
        let tempdir = Utf8Path::from_path(tmp.path()).unwrap();

        let pb_path = tempdir.join("product_bundle");
        std::fs::create_dir_all(&pb_path).unwrap();

        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "fake.pb-name".to_string(),
            product_version: "fake.pb-version".to_string(),
            partitions: PartitionsConfig::default(),
            sdk_version: "fake.sdk-version".to_string(),
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
        pb.write(&pb_path).unwrap();

        let output = tempdir.join("product_description.json");
        let cmd = GenerateProductDescription {
            product_bundle: pb_path.clone(),
            out_file: output.to_path_buf(),
            transfer_url: "gs://fake/path/to/transfer.json".to_string(),
        };
        cmd.generate().unwrap();

        let product_description_file = File::open(&output).unwrap();
        let product_description: ProductDescription =
            serde_json::from_reader(product_description_file).unwrap();
        assert_eq!(
            product_description,
            ProductDescription::V1(ProductDescriptionV1 {
                abi: None,
                board_name: None,
                platform_version: None,
                product_name: "fake.pb-name".to_string(),
                transfer_url: "gs://fake/path/to/transfer.json".to_string(),
                product_version: "fake.pb-version".to_string(),
                sdk_version: Some("fake.sdk-version".to_string()),
            }),
        );
    }
}
