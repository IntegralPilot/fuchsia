// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::TestEnv;
use fidl_fuchsia_pkg::PackageIndexIteratorMarker;
use fidl_fuchsia_pkg_ext::BlobId;
use fuchsia_hash::Hash;
use fuchsia_pkg_testing::{PackageBuilder, SystemImageBuilder};

async fn verify_cache_packages(
    env: &TestEnv,
    mut expected_entries: impl Iterator<Item = (String, Hash)>,
) {
    let (pkg_iterator, server_end) = fidl::endpoints::create_proxy::<PackageIndexIteratorMarker>();
    env.proxies.package_cache.cache_package_index(server_end).unwrap();

    loop {
        let chunk = pkg_iterator.next().await.unwrap();
        if chunk.is_empty() {
            break;
        }
        for entry in chunk {
            let (expected_url, expected_hash) = expected_entries.next().unwrap();

            assert_eq!(entry.package_url.url, expected_url);
            assert_eq!(BlobId::from(entry.meta_far_blob_id), BlobId::from(expected_hash));
        }
    }

    assert_eq!(expected_entries.next(), None);
}

#[fuchsia::test]
async fn missing_cache_package_manifest_empty_iterator() {
    let system_image_package = SystemImageBuilder::new().build().await;
    let env = TestEnv::builder()
        .blobfs_from_system_image_and_extra_packages(&system_image_package, &[])
        .await
        .build()
        .await;
    env.block_until_started().await;

    verify_cache_packages(&env, vec![].into_iter()).await;
}

#[fuchsia::test]
async fn present_cache_package_manifest() {
    let pkg = PackageBuilder::new("some-cache-package").build().await.unwrap();
    let system_image_package = SystemImageBuilder::new().cache_packages(&[&pkg]).build().await;
    let env = TestEnv::builder()
        .blobfs_from_system_image_and_extra_packages(&system_image_package, &[&pkg])
        .await
        .build()
        .await;
    env.block_until_started().await;

    verify_cache_packages(
        &env,
        vec![("fuchsia-pkg://fuchsia.com/some-cache-package".to_string(), *pkg.hash())].into_iter(),
    )
    .await;
}
