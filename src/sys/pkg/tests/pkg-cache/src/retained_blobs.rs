// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{TestEnv, replace_retained_blobs};
use assert_matches::assert_matches;
use fuchsia_pkg_testing::{PackageBuilder, SystemImageBuilder};

#[fuchsia::test]
async fn retained_blobs_are_not_garbage_collected() {
    let retained_package = PackageBuilder::new("pkg-a")
        .add_resource_at("bin/foo", "a-bin-foo".as_bytes())
        .add_resource_at("data/content", "a-data-content".as_bytes())
        .build()
        .await
        .unwrap();
    let garbage_package = PackageBuilder::new("pkg-b")
        .add_resource_at("bin/bar", "b-bin-bar".as_bytes())
        .add_resource_at("data/asset", "b-data-asset".as_bytes())
        .build()
        .await
        .unwrap();

    let system_image_package = SystemImageBuilder::new().build().await;

    let env = TestEnv::builder()
        .blobfs_from_system_image_and_extra_packages(
            &system_image_package,
            &[&retained_package, &garbage_package],
        )
        .await
        .build()
        .await;

    let retained_blobs = retained_package.list_blobs();

    replace_retained_blobs(&env.proxies.retained_blobs, retained_blobs.clone()).await;

    assert_matches!(env.proxies.space_manager.gc().await, Ok(Ok(())));

    assert!(env.blobfs.list_blobs().unwrap().is_superset(&retained_blobs));
    assert!(env.blobfs.list_blobs().unwrap().is_disjoint(&garbage_package.list_blobs()));
}
