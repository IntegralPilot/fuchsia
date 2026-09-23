// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::frozen_index::{BaseIndex, CacheIndex};
use fidl_fuchsia_pkg as fpkg;
use fuchsia_sync::Mutex;
use fuchsia_url::fuchsia_pkg::{PinnedAbsolutePackageUrl, UnpinnedAbsolutePackageUrl};
use log::error;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub struct UpgradablePackages {
    packages: Mutex<HashMap<UnpinnedAbsolutePackageUrl, fuchsia_url::Hash>>,
    cache_packages: Arc<CacheIndex>,
    init_event: async_utils::event::Event,
}

impl UpgradablePackages {
    pub fn new(cache_packages: Arc<CacheIndex>) -> Self {
        Self {
            packages: Mutex::new(HashMap::new()),
            cache_packages,
            init_event: async_utils::event::Event::new(),
        }
    }

    pub async fn get_hash(&self, url: &UnpinnedAbsolutePackageUrl) -> Option<fuchsia_url::Hash> {
        let () = self.init_event.wait().await;
        self.packages
            .lock()
            .get(url)
            .or_else(|| self.cache_packages.root_package_urls_and_hashes().get(url))
            .copied()
    }

    pub fn set_upgradable_urls(
        &self,
        pinned_urls: Vec<fpkg::PackageUrl>,
        base_packages: &BaseIndex,
    ) -> Result<(), fpkg::SetUpgradableUrlsError> {
        let mut partial_set = false;
        {
            let mut packages = self.packages.lock();
            for fpkg::PackageUrl { url } in pinned_urls {
                let url = match url.parse::<PinnedAbsolutePackageUrl>() {
                    Ok(url) => url,
                    Err(e) => {
                        error!("failed to parse pinned url {url:?}: {e:?}");
                        partial_set = true;
                        continue;
                    }
                };
                if base_packages.root_package_urls_and_hashes().contains_key(url.as_unpinned()) {
                    error!("upgrade base package {} is not allowed", url.as_unpinned());
                    partial_set = true;
                    continue;
                }
                let (unpinned, hash) = url.into_unpinned_and_hash();
                packages.insert(unpinned, hash);
            }
        }
        self.init_event.signal();

        if partial_set {
            return Err(fpkg::SetUpgradableUrlsError::PartialSet);
        }
        Ok(())
    }

    pub async fn list_blobs(&self, blobfs: &blobfs::Client) -> HashSet<fuchsia_hash::Hash> {
        let () = self.init_event.wait().await;

        let mut all_blobs = HashSet::new();
        let package_hashes: Vec<_> = self.packages.lock().values().copied().collect();
        let memoized_packages = async_lock::RwLock::new(HashMap::new());
        for package_hash in package_hashes {
            match crate::required_blobs::find_required_blobs_recursive(
                blobfs,
                &package_hash,
                &memoized_packages,
                crate::required_blobs::ErrorStrategy::PropagateFailure,
            )
            .await
            {
                Ok(blobs) => {
                    all_blobs.insert(package_hash);
                    all_blobs.extend(blobs);
                }
                Err(e) => {
                    error!(
                        "find required blobs for package {package_hash}: {:#}",
                        anyhow::anyhow!(e)
                    );
                }
            }
        }
        all_blobs
    }
}
