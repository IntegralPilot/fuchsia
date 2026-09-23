// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Retrives and serves the base package index.

use crate::BlobId;
use anyhow::Error;
use fidl_fuchsia_pkg::{PackageCacheProxy, PackageIndexIteratorMarker};
use fuchsia_url::fuchsia_pkg::{AbsolutePackageUrl, UnpinnedAbsolutePackageUrl};
use std::collections::HashMap;

/// Represents the set of base packages.
#[derive(Debug)]
pub struct BasePackageIndex {
    index: HashMap<UnpinnedAbsolutePackageUrl, BlobId>,
}

impl BasePackageIndex {
    /// Creates a `BasePackageIndex` from a PackageCache proxy.
    pub async fn from_proxy(cache: &PackageCacheProxy) -> Result<Self, Error> {
        let (pkg_iterator, server_end) =
            fidl::endpoints::create_proxy::<PackageIndexIteratorMarker>();
        cache.base_package_index(server_end)?;

        let mut index = HashMap::with_capacity(256);
        let mut chunk = pkg_iterator.next().await?;
        while !chunk.is_empty() {
            for entry in chunk {
                let pkg_url = UnpinnedAbsolutePackageUrl::parse(&entry.package_url.url)?;
                let blob_id = BlobId::from(entry.meta_far_blob_id);
                index.insert(pkg_url, blob_id);
            }
            chunk = pkg_iterator.next().await?;
        }
        index.shrink_to_fit();

        Ok(Self { index })
    }

    /// Returns the package's hash if the url is unpinned and refers to a base package, otherwise
    /// returns None.
    pub fn is_unpinned_base_package(&self, pkg_url: &AbsolutePackageUrl) -> Option<BlobId> {
        // Always send Merkle-pinned requests through the resolver.
        // TODO(https://fxbug.dev/42140778) consider returning the pinned hash if it matches the base hash.
        let pkg_url = match pkg_url {
            AbsolutePackageUrl::Unpinned(unpinned) => unpinned,
            AbsolutePackageUrl::Pinned(_) => return None,
        };

        // Make sure to strip off a "/0" variant before checking the base index.
        let stripped_url;
        let url_without_zero_variant = match pkg_url.variant() {
            Some(variant) if variant.is_zero() => {
                let mut url = pkg_url.clone();
                url.clear_variant();
                stripped_url = url;
                &stripped_url
            }
            _ => pkg_url,
        };
        match self.index.get(&url_without_zero_variant) {
            Some(base_merkle) => Some(base_merkle.clone()),
            None => None,
        }
    }

    /// Creates a BasePackageIndex backed only by the supplied `HashMap`. This
    /// is useful for testing.
    pub fn create_mock(index: HashMap<UnpinnedAbsolutePackageUrl, BlobId>) -> Self {
        Self { index }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_pkg::{
        self as fpkg, PackageCacheMarker, PackageCacheRequest, PackageCacheRequestStream,
        PackageIndexEntry, PackageIndexIteratorRequest, PackageIndexIteratorRequestStream,
    };
    use fuchsia_async as fasync;
    use futures::prelude::*;
    use log::error;
    use std::sync::Arc;

    // The actual pkg-cache will fit as many items per chunk as possible.  Intentionally choose a
    // small, fixed value here to verify the BasePackageIndex behavior with multiple chunks without
    // having to actually send hundreds of entries in these tests.
    const PACKAGE_INDEX_CHUNK_SIZE: u32 = 30;

    struct MockPackageCacheService {
        base_packages: Arc<HashMap<UnpinnedAbsolutePackageUrl, BlobId>>,
    }

    impl MockPackageCacheService {
        fn new_with_base_packages(
            base_packages: Arc<HashMap<UnpinnedAbsolutePackageUrl, BlobId>>,
        ) -> Self {
            Self { base_packages: base_packages }
        }

        async fn run_service(self, mut stream: PackageCacheRequestStream) {
            while let Some(req) = stream.try_next().await.unwrap() {
                match req {
                    PackageCacheRequest::BasePackageIndex { iterator, control_handle: _ } => {
                        let iterator = iterator.into_stream();
                        self.serve_package_iterator(iterator);
                    }
                    _ => panic!("unexpected PackageCache request: {:?}", req),
                }
            }
        }

        fn serve_package_iterator(&self, mut stream: PackageIndexIteratorRequestStream) {
            let packages = self
                .base_packages
                .iter()
                .map(|(path, hash)| PackageIndexEntry {
                    package_url: fpkg::PackageUrl {
                        url: format!("fuchsia-pkg://fuchsia.com/{}", path.name()),
                    },
                    meta_far_blob_id: BlobId::from(hash.clone()).into(),
                })
                .collect::<Vec<PackageIndexEntry>>();

            fasync::Task::spawn(
                async move {
                    let mut iter = packages.chunks(PACKAGE_INDEX_CHUNK_SIZE as usize);
                    while let Some(request) = stream.try_next().await? {
                        let PackageIndexIteratorRequest::Next { responder } = request;
                        responder.send(iter.next().unwrap_or(&[]))?;
                    }
                    Ok(())
                }
                .unwrap_or_else(|e: fidl::Error| {
                    error!("while serving package index iterator: {:?}", e)
                }),
            )
            .detach();
        }
    }

    async fn spawn_pkg_cache(
        base_package_index: HashMap<UnpinnedAbsolutePackageUrl, BlobId>,
    ) -> PackageCacheProxy {
        let (client, request_stream) = create_proxy_and_stream::<PackageCacheMarker>();
        let cache = MockPackageCacheService::new_with_base_packages(Arc::new(base_package_index));
        fasync::Task::spawn(cache.run_service(request_stream)).detach();
        client
    }

    #[fuchsia::test]
    async fn empty_base_packages() {
        let expected_packages = HashMap::new();
        let client = spawn_pkg_cache(expected_packages.clone()).await;
        let base = BasePackageIndex::from_proxy(&client).await.unwrap();
        assert_eq!(base.index, expected_packages);
    }

    // Generate an index with n unique entries.
    fn index_with_n_entries(n: u32) -> HashMap<UnpinnedAbsolutePackageUrl, BlobId> {
        let mut base = HashMap::new();
        for i in 0..n {
            let pkg_url = format!("fuchsia-pkg://fuchsia.com/{}", i)
                .parse::<UnpinnedAbsolutePackageUrl>()
                .unwrap();
            let blob_id = format!("{:064}", i).parse::<BlobId>().unwrap();
            base.insert(pkg_url, blob_id);
        }
        base
    }

    #[fuchsia::test]
    async fn chunk_size_boundary() {
        let package_counts = [
            PACKAGE_INDEX_CHUNK_SIZE - 1,
            PACKAGE_INDEX_CHUNK_SIZE,
            PACKAGE_INDEX_CHUNK_SIZE + 1,
            2 * PACKAGE_INDEX_CHUNK_SIZE + 1,
        ];
        for count in package_counts.iter() {
            let expected_packages = index_with_n_entries(*count);
            let client = spawn_pkg_cache(expected_packages.clone()).await;
            let base = BasePackageIndex::from_proxy(&client).await.unwrap();
            assert_eq!(base.index, expected_packages);
        }
    }

    fn zeroes_hash() -> BlobId {
        "0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap()
    }

    #[test]
    fn reject_pinned_urls() {
        let url: AbsolutePackageUrl = "fuchsia-pkg://fuchsia.com/package-name?\
                   hash=0000000000000000000000000000000000000000000000000000000000000000"
            .parse()
            .unwrap();
        let index = BasePackageIndex { index: [(url.as_unpinned().clone(), zeroes_hash())].into() };

        assert_eq!(index.is_unpinned_base_package(&url), None);
    }

    #[test]
    fn strip_0_variant() {
        let url_no_variant: UnpinnedAbsolutePackageUrl =
            "fuchsia-pkg://fuchsia.com/package-name".parse().unwrap();
        let index = BasePackageIndex { index: [(url_no_variant.clone(), zeroes_hash())].into() };

        let url_with_variant: AbsolutePackageUrl =
            "fuchsia-pkg://fuchsia.com/package-name/0".parse().unwrap();
        assert_eq!(index.is_unpinned_base_package(&url_with_variant), Some(zeroes_hash()));
    }

    #[test]
    fn leave_1_variant() {
        let url_no_variant: UnpinnedAbsolutePackageUrl =
            "fuchsia-pkg://fuchsia.com/package-name".parse().unwrap();
        let index = BasePackageIndex { index: [(url_no_variant.clone(), zeroes_hash())].into() };

        let url_with_variant: AbsolutePackageUrl =
            "fuchsia-pkg://fuchsia.com/package-name/1".parse().unwrap();
        assert_eq!(index.is_unpinned_base_package(&url_with_variant), None);
    }
}
