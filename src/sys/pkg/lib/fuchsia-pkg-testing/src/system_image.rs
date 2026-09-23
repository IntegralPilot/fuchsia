// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Package, PackageBuilder};
use fuchsia_merkle::Hash;
use fuchsia_pkg::PackagePath;
use fuchsia_pkg::package_sets::AnchoredPackageSetType;
use fuchsia_url::fuchsia_pkg::PinnedAbsolutePackageUrl;
use std::future::Future;
use system_image::{AnchoredPackages, CachePackages, StaticPackages};

const DEFAULT_PACKAGE_REPO_URL: &str = "fuchsia-pkg://fuchsia.com";

/// Builds a system_image package.
#[derive(Default)]
pub struct SystemImageBuilder {
    static_packages: Option<Vec<(PackagePath, Hash)>>,
    cache_packages: Option<Vec<PinnedAbsolutePackageUrl>>,
    anchored_packages: Option<AnchoredPackages>,
    pkgfs_disable_executability_restrictions: bool,
}

impl SystemImageBuilder {
    /// Returns an empty `SystemImageBuilder` configured with no static or cache package.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends the given path and hash to the static packages manifest.
    pub fn static_package(mut self, path: PackagePath, hash: Hash) -> Self {
        self.static_packages.get_or_insert_with(Vec::new).push((path, hash));
        self
    }

    /// Use the supplied static_packages with the system image. Call at most once and not after
    /// calling [`Self::static_package`].
    pub fn static_packages(mut self, static_packages: &[&Package]) -> Self {
        assert_eq!(self.static_packages, None);
        self.static_packages = Some(Self::packages_to_entries(static_packages));
        self
    }

    /// Appends the given path and hash to the cache packages manifest, creating the manifest if it
    /// was not already staged to be added to the package.
    pub fn cache_package(mut self, path: PackagePath, hash: Hash) -> Self {
        let (name, variant) = path.into_name_and_variant();
        let pinned_url = PinnedAbsolutePackageUrl::new(
            DEFAULT_PACKAGE_REPO_URL.parse().unwrap(),
            name,
            Some(variant),
            hash,
        );
        self.cache_packages.get_or_insert_with(Vec::new).push(pinned_url);
        self
    }

    /// Use the supplied cache_packages with the system image. Call at most once and not after
    /// calling [`Self::cache_package`].
    pub fn cache_packages(mut self, cache_packages: &[&Package]) -> Self {
        assert_eq!(self.cache_packages, None);
        self.cache_packages =
            Some(cache_packages.iter().map(|pkg| pkg.pinned_fuchsia_url()).collect());
        self
    }

    /// Appends the given package set type, path and hash to the anchored packages manifest,
    /// creating the manifest if it was not already staged to be added to the package.
    pub fn anchored_package(
        mut self,
        package_set_type: AnchoredPackageSetType,
        path: PackagePath,
        hash: Hash,
    ) -> Self {
        let (name, variant) = path.into_name_and_variant();
        let pinned_url = PinnedAbsolutePackageUrl::new(
            DEFAULT_PACKAGE_REPO_URL.parse().unwrap(),
            name,
            Some(variant),
            hash,
        );
        self.anchored_packages
            .get_or_insert_default()
            .insert(package_set_type, pinned_url)
            .unwrap();
        self
    }

    /// Disable enforcement of executability restrictions for packages that are not base or
    /// allowlisted.
    pub fn pkgfs_disable_executability_restrictions(mut self) -> Self {
        self.pkgfs_disable_executability_restrictions = true;
        self
    }

    fn packages_to_entries(pkgs: &[&Package]) -> Vec<(PackagePath, Hash)> {
        pkgs.iter()
            .map(|pkg| {
                (
                    PackagePath::from_name_and_variant(pkg.name().to_owned(), "0".parse().unwrap()),
                    *pkg.hash(),
                )
            })
            .collect()
    }

    /// Build the system_image package.
    pub fn build(&self) -> impl Future<Output = Package> {
        let mut builder = PackageBuilder::new("system_image");
        let mut bytes = vec![];

        StaticPackages::from_iter(self.static_packages.clone().unwrap_or_default())
            .serialize(&mut bytes)
            .unwrap();
        builder = builder.add_resource_at("data/static_packages", bytes.as_slice());

        if let Some(cache_packages) = &self.cache_packages {
            bytes.clear();
            let cache_packages = CachePackages::from_entries(cache_packages.clone());
            cache_packages.serialize(&mut bytes).unwrap();
            builder = builder.add_resource_at("data/cache_packages.json", bytes.as_slice());
        }

        if let Some(anchored_packages) = &self.anchored_packages {
            bytes.clear();
            anchored_packages.serialize(&mut bytes).unwrap();
            builder = builder.add_resource_at("data/anchored_packages.json", bytes.as_slice());
        }

        if self.pkgfs_disable_executability_restrictions {
            builder = builder
                .add_resource_at("data/pkgfs_disable_executability_restrictions", &[] as &[u8]);
        }

        async move { builder.build().await.unwrap() }
    }
}
