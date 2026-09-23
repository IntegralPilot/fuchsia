// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use assembly_container::{AssemblyContainer, FileType, WalkPaths, WalkPathsFn, assembly_container};
use assembly_package_utils::PackageManifestPathBuf;
use assembly_release_info::ReleaseInfo;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A subset of product functionality that can be added to a product config.
#[derive(Debug, Default, Deserialize, Serialize, WalkPaths)]
#[serde(deny_unknown_fields)]
#[assembly_container(product_input_bundle.json)]
pub struct ProductInputBundle {
    /// Packages to add to the product.
    #[walk_paths]
    pub packages: ProductPackagesConfig,
    /// The release info for the PIB.
    pub release_info: ReleaseInfo,
}

/// Packages to add to the product.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ProductPackagesConfig {
    /// The base packages to add to the product.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub base: BTreeMap<String, ProductPackageDetails>,

    /// The cache packages to add to the product.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub cache: BTreeMap<String, ProductPackageDetails>,

    /// The flexible packages to add to the product.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub flexible: BTreeMap<String, ProductPackageDetails>,

    /// Packages that are only included in the final images if the product
    /// config references them.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub for_product_config: BTreeMap<String, ProductPackageDetails>,
}

/// Describes in more detail a package to add to the assembly.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProductPackageDetails {
    /// Path to the package manifest for this package.
    pub manifest: Utf8PathBuf,
}

fn walk_package_set<F>(
    set: &mut BTreeMap<String, ProductPackageDetails>,
    found: &mut F,
    dest: Utf8PathBuf,
) -> Result<()>
where
    F: WalkPathsFn,
{
    for (name, pkg) in set {
        let pkg_dest = dest.join(name);
        found(&mut pkg.manifest, pkg_dest, FileType::PackageManifest)?;
    }
    Ok(())
}

impl WalkPaths for ProductPackagesConfig {
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> anyhow::Result<()> {
        walk_package_set(&mut self.base, found, dest.join("base"))?;
        walk_package_set(&mut self.cache, found, dest.join("cache"))?;
        walk_package_set(&mut self.flexible, found, dest.join("flexible"))?;
        walk_package_set(&mut self.for_product_config, found, dest.join("for_product_config"))?;
        Ok(())
    }
}

impl From<PackageManifestPathBuf> for ProductPackageDetails {
    fn from(manifest: PackageManifestPathBuf) -> Self {
        let manifestpath: &Utf8Path = manifest.as_ref();
        let path: Utf8PathBuf = manifestpath.into();
        Self { manifest: path }
    }
}

impl From<&str> for ProductPackageDetails {
    fn from(s: &str) -> Self {
        ProductPackageDetails { manifest: s.into() }
    }
}
