// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::package_reader::PackageReader;
use crate::package_types::{ComponentManifest, PackageDefinition, PartialPackageDefinition};
use anyhow::{Result, anyhow};
use fuchsia_merkle::{HASH_SIZE, Hash};
use fuchsia_url::fuchsia_pkg::{AbsolutePackageUrl, PinnedAbsolutePackageUrl};
use fuchsia_url::{PackageName, PackageVariant};
use scrutiny_collection::model::DataModel;
use scrutiny_testing::TEST_REPO_URL;
use scrutiny_testing::artifact::AppendResult;
use scrutiny_testing::fake::fake_model_config;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

pub struct MockPackageReader {
    pkg_urls: Option<Vec<PinnedAbsolutePackageUrl>>,
    update_pkg_def: Option<PartialPackageDefinition>,
    pkg_defs: HashMap<AbsolutePackageUrl, PackageDefinition>,
    deps: HashSet<PathBuf>,
}

impl MockPackageReader {
    pub fn new() -> Self {
        Self {
            pkg_urls: None,
            update_pkg_def: None,
            pkg_defs: HashMap::new(),
            deps: HashSet::new(),
        }
    }

    pub fn append_update_package(
        &mut self,
        pkg_urls: Vec<PinnedAbsolutePackageUrl>,
        update_pkg_def: PartialPackageDefinition,
    ) -> AppendResult {
        let result = if self.pkg_urls.is_some() || self.update_pkg_def.is_some() {
            AppendResult::Merged
        } else {
            AppendResult::Appended
        };
        self.pkg_urls = Some(pkg_urls);
        self.update_pkg_def = Some(update_pkg_def);
        result
    }

    pub fn append_pkg_def(&mut self, pkg_def: PackageDefinition) -> AppendResult {
        if let Some(existing_pkg_def) = self.pkg_defs.get_mut(&pkg_def.url) {
            *existing_pkg_def = pkg_def;
            AppendResult::Merged
        } else {
            self.pkg_defs.insert(pkg_def.url.clone(), pkg_def);
            AppendResult::Appended
        }
    }

    pub fn append_dep(&mut self, path_buf: PathBuf) -> AppendResult {
        if self.deps.insert(path_buf) { AppendResult::Appended } else { AppendResult::Merged }
    }
}

impl PackageReader for MockPackageReader {
    fn read_package_urls(&mut self) -> Result<Vec<PinnedAbsolutePackageUrl>> {
        self.pkg_urls.clone().ok_or_else(|| {
            anyhow!(
                "Attempt to read package URLs from mock package reader with no package URLs set"
            )
        })
    }

    fn read_package_definition(
        &mut self,
        pkg_url: &PinnedAbsolutePackageUrl,
    ) -> Result<PackageDefinition> {
        self.pkg_defs.get(&pkg_url.clone().into()).map(|pkg_def| pkg_def.clone()).ok_or_else(|| {
            anyhow!("Mock package reader contains no package definition for {:?}", pkg_url)
        })
    }

    fn read_update_package_definition(&mut self) -> Result<PartialPackageDefinition> {
        self.update_pkg_def.as_ref().map(|update_pkg_def| update_pkg_def.clone()).ok_or_else(|| {
            anyhow!(
                "Attempt to read update package from mock package reader with no update package set"
            )
        })
    }

    fn get_deps(&self) -> HashSet<PathBuf> {
        self.deps.clone()
    }
}

/// Create component manifest v2 (cm) entries.
pub fn create_test_cm_map(entries: Vec<(PathBuf, Vec<u8>)>) -> HashMap<PathBuf, ComponentManifest> {
    entries.into_iter().map(|entry| (entry.0, ComponentManifest::Version2(entry.1))).collect()
}

pub fn create_test_package_with_cms(
    name: PackageName,
    variant: Option<PackageVariant>,
    cms: HashMap<PathBuf, ComponentManifest>,
) -> PackageDefinition {
    PackageDefinition {
        url: AbsolutePackageUrl::new(
            TEST_REPO_URL.clone(),
            name,
            variant,
            Some([0; HASH_SIZE].into()),
        ),
        meta: HashMap::new(),
        contents: HashMap::new(),
        cms,
        cvfs: Default::default(),
    }
}

pub fn create_test_package_with_contents(
    name: PackageName,
    variant: Option<PackageVariant>,
    contents: HashMap<PathBuf, Hash>,
) -> PackageDefinition {
    PackageDefinition {
        url: AbsolutePackageUrl::new(
            TEST_REPO_URL.clone(),
            name,
            variant,
            Some([0; HASH_SIZE].into()),
        ),
        meta: HashMap::new(),
        contents: contents,
        cms: HashMap::new(),
        cvfs: HashMap::new(),
    }
}

pub fn create_model() -> Arc<DataModel> {
    let config = fake_model_config();
    Arc::new(DataModel::new(config).unwrap())
}
