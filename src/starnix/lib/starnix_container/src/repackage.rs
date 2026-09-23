// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use assembly_config_schema::product_config::ProductConfig;
use camino::Utf8Path;
use depfile::Depfile;
use std::collections::HashMap;

pub fn repackage_starnix_containers(config: &mut ProductConfig, outdir: &Utf8Path) -> Result<()> {
    if config.product.starnix_containers.is_empty() {
        return Ok(());
    }

    let repackaged_containers_dir = outdir.join("repackaged_containers");
    std::fs::create_dir_all(&repackaged_containers_dir)
        .map_err(|e| anyhow::anyhow!("creating repackaged container dir: {}", e))?;

    let mut depfile = Depfile::new();
    let mut packages_to_update = HashMap::new();

    for container in &mut config.product.starnix_containers {
        let container_manifest_path = match &container.images_or_package {
            assembly_config_schema::product_settings::StarnixImagesOrPackage::Package(p) => {
                Ok(p.clone())
            }
            _ => Err(anyhow::anyhow!(
                "The product command does not support building starnix containers from images.",
            )),
        }?;

        let container_outdir = repackaged_containers_dir.join(&container.name);
        std::fs::create_dir_all(&container_outdir)
            .map_err(|e| anyhow::anyhow!("creating repackaged container outdir: {}", e))?;

        let container_base_manifest_path =
            ProductConfig::find_package_in_pibs(&config.product_input_bundles, &container.base)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "finding starnix base package '{}' in product input bundles",
                        container.base
                    )
                })?
                .clone();

        let hals =
            ProductConfig::find_hals_in_pibs(&config.product_input_bundles, &container.hals)?;

        for (name, manifest) in container.hals.iter().zip(hals.iter()) {
            packages_to_update.insert(name.clone(), manifest.clone());
        }

        let output_manifest_path = crate::StarnixContainerRepackager {
            name: container.name.clone(),
            outdir: container_outdir,
            container_manifest_path: container_manifest_path,
            base: container_base_manifest_path,
            hals: hals,
            skip_subpackages: container.skip_subpackages,
        }
        .build(&mut depfile)?;

        container.images_or_package =
            assembly_config_schema::product_settings::StarnixImagesOrPackage::Package(
                output_manifest_path.clone(),
            );
        packages_to_update.insert(container.name.clone(), output_manifest_path);
    }

    // Replace the HALs in the product config package sets.
    for (name, path) in &packages_to_update {
        if let Some(target_path) = config.find_package_in_product(name) {
            *target_path = path.clone();
        }
    }

    Ok(())
}
