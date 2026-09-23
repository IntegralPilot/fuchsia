// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::extract::package::PackageExtractController;
use crate::verify::capability_routing::{CapabilityRouteController, ResponseLevel};
use crate::verify::component_resolvers::{
    ComponentResolverRequest, ComponentResolverResponse, ComponentResolversController,
};
use crate::verify::pre_signing::{PreSigningController, PreSigningResponse};
use crate::verify::route_sources::RouteSourcesController;
use crate::verify::structured_config::{
    ExtractStructuredConfigController, ExtractStructuredConfigResponse,
    VerifyStructuredConfigController, VerifyStructuredConfigResponse,
};
use crate::verify::{CapabilityRouteResults, VerifyRouteSourcesResults};
use anyhow::Result;
use cm_rust::CapabilityTypeName;
use fuchsia_url::fuchsia_pkg::AbsolutePackageUrl;
use scrutiny_collection::core::{Component, Components, Package, Packages};
use scrutiny_collection::model::DataModel;
use scrutiny_collection::static_packages::StaticPkgsCollection;
use scrutiny_collection::zbi::Zbi;
use scrutiny_utils::package::PackageIndexContents;
use scrutiny_utils::url::from_pkg_url_parts;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

pub struct ScrutinyArtifacts {
    pub model: Arc<DataModel>,
}

impl ScrutinyArtifacts {
    pub fn get_components(&self) -> Result<Vec<Component>> {
        let mut components = self.model.get::<Components>()?.entries.clone();
        components.sort_by(|a, b| a.url.partial_cmp(&b.url).unwrap());
        Ok(components)
    }

    pub fn get_package(&self, url: AbsolutePackageUrl) -> Result<Option<Package>> {
        let packages = &self.model.get::<Packages>()?.entries;
        for package in packages.iter() {
            if package.matches_url(&url) {
                return Ok(Some(package.clone()));
            }
        }
        return Ok(None);
    }

    pub fn get_package_urls(&self) -> Result<Vec<AbsolutePackageUrl>> {
        let mut packages = self.model.get::<Packages>().unwrap().entries.clone();
        packages.sort();
        Ok(packages
            .into_iter()
            .map(|package| from_pkg_url_parts(package.name, package.variant, Some(package.merkle)))
            .collect::<Result<Vec<AbsolutePackageUrl>>>()?)
    }

    pub fn get_bootfs_files(&self) -> Result<Vec<String>> {
        let mut files = self
            .model
            .get::<Zbi>()
            .unwrap()
            .bootfs_files
            .bootfs_files
            .keys()
            .cloned()
            .collect::<Vec<String>>();
        files.sort();
        Ok(files)
    }

    // TODO: Why is this optional?
    pub fn get_bootfs_packages(&self) -> Result<Option<PackageIndexContents>> {
        Ok(self.model.get::<Zbi>().unwrap().bootfs_packages.bootfs_pkgs.clone())
    }

    pub fn get_static_packages(&self) -> Result<StaticPkgsCollection> {
        Ok((*self.model.get::<StaticPkgsCollection>().unwrap()).clone())
    }

    pub fn get_monikers_for_resolver(
        &self,
        scheme: String,
        moniker: String,
        protocol: String,
    ) -> Result<ComponentResolverResponse> {
        let request = ComponentResolverRequest { scheme, moniker, protocol };
        ComponentResolversController::get_monikers(self.model.clone(), request)
    }

    pub fn get_cmdline(&self) -> Result<Vec<String>> {
        Ok(self.model.get::<Zbi>().unwrap().cmdline.clone())
    }

    pub fn extract_package(
        &self,
        url: AbsolutePackageUrl,
        output: impl AsRef<Path>,
    ) -> Result<Value> {
        PackageExtractController::extract(self.model.clone(), url, output)
    }

    pub fn extract_structured_config(&self) -> Result<ExtractStructuredConfigResponse> {
        ExtractStructuredConfigController::extract(self.model.clone())
    }

    pub fn get_capability_route_results(
        &self,
        capability_types: HashSet<CapabilityTypeName>,
        response_level: &ResponseLevel,
    ) -> Result<CapabilityRouteResults> {
        CapabilityRouteController::get_results(self.model.clone(), capability_types, response_level)
    }

    pub fn get_route_sources(&self, input: String) -> Result<VerifyRouteSourcesResults> {
        RouteSourcesController::get_results(self.model.clone(), input)
    }

    pub fn collect_presigning_errors(
        &self,
        policy_path: String,
        golden_files_dir: String,
    ) -> Result<PreSigningResponse> {
        PreSigningController::collect_errors(self.model.clone(), policy_path, golden_files_dir)
    }

    pub fn verify_structured_config(
        &self,
        policy_path: impl AsRef<Path>,
    ) -> Result<VerifyStructuredConfigResponse> {
        VerifyStructuredConfigController::verify(self.model.clone(), policy_path)
    }
}
