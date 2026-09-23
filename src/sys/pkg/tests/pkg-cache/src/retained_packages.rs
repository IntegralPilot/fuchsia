// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{
    TestEnv, replace_retained_packages, verify_packages_cached, write_meta_far, write_needed_blobs,
};
use assert_matches::assert_matches;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_pkg::{self as fpkg};
use fidl_fuchsia_pkg_ext::BlobId;
use fuchsia_pkg_testing::{PackageBuilder, SystemImageBuilder};
use futures::TryFutureExt;

#[fuchsia::test]
async fn cached_packages_are_retained() {
    // Packages to be retained.
    let packages = vec![
        PackageBuilder::new("pkg-a").build().await.unwrap(),
        PackageBuilder::new("multi-pkg-a")
            .add_resource_at("bin/foo", "a-bin-foo".as_bytes())
            .add_resource_at("data/content", "a-data-content".as_bytes())
            .build()
            .await
            .unwrap(),
    ];

    // Packages to be written to BlobFS to emulate data available for GC.
    let garbage_packages = [
        PackageBuilder::new("pkg-b").build().await.unwrap(),
        PackageBuilder::new("multi-pkg-b")
            .add_resource_at("bin/bar", "b-bin-bar".as_bytes())
            .add_resource_at("data/asset", "b-data-asset".as_bytes())
            .build()
            .await
            .unwrap(),
    ];

    let system_image_package = SystemImageBuilder::new().build().await;

    let env = TestEnv::builder()
        .blobfs_from_system_image_and_extra_packages(
            &system_image_package,
            &packages.iter().chain(garbage_packages.iter()).collect::<Vec<_>>(),
        )
        .await
        .build()
        .await;

    let blob_ids = packages.iter().map(|pkg| BlobId::from(*pkg.hash())).collect::<Vec<_>>();

    // Mark packages as retained.
    replace_retained_packages(&env.proxies.retained_packages, blob_ids.as_slice()).await;

    assert_matches!(env.proxies.space_manager.gc().await, Ok(Ok(())));

    // Verify no retained package blobs are deleted, directly from blobfs.
    for pkg in packages.iter() {
        assert!(env.blobfs.client().blob_present_and_up_to_date(pkg.hash()).await);
    }

    for pkg in garbage_packages.iter() {
        assert_eq!(env.blobfs.client().blob_present_and_up_to_date(pkg.hash()).await, false);
    }

    // Verify no retained package blobs are deleted, using PackageCache API.
    verify_packages_cached(&env.proxies.package_cache, &packages).await;
}

#[fuchsia::test]
async fn packages_are_retained_gc_mid_process() {
    let env = TestEnv::builder().build().await;
    let package = PackageBuilder::new("multi-pkg-a")
        .add_resource_at("bin/foo", "a-bin-foo".as_bytes())
        .add_resource_at("data/content", "a-data-content".as_bytes())
        .build()
        .await
        .unwrap();

    let blob_id = BlobId::from(*package.hash());

    // Start installing a package (write the meta far).
    let meta_blob_info = fpkg::BlobInfo { blob_id: blob_id.into(), length: 0 };

    let (needed_blobs, needed_blobs_server_end) =
        fidl::endpoints::create_proxy::<fpkg::NeededBlobsMarker>();
    let (dir, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    let get_fut = env
        .proxies
        .package_cache
        .get(
            &meta_blob_info,
            fpkg::GcProtection::OpenPackageTracking,
            needed_blobs_server_end,
            dir_server_end,
        )
        .map_ok(|res| res.map_err(zx::Status::err_from_raw));

    let (meta_far, contents) = package.contents();
    write_meta_far(&needed_blobs, meta_far).await;

    // Add the packages as retained.
    replace_retained_packages(&env.proxies.retained_packages, &[blob_id]).await;

    // Clear the retained index and GC.
    assert_matches!(env.proxies.retained_packages.clear().await, Ok(()));
    assert_matches!(env.proxies.space_manager.gc().await, Ok(Ok(())));

    // Verify the package’s meta far is not deleted.
    assert!(env.blobfs.client().blob_present_and_up_to_date(package.hash()).await);

    write_needed_blobs(&needed_blobs, contents).await;

    assert_matches!(env.proxies.space_manager.gc().await, Ok(Ok(())));

    let () = get_fut.await.unwrap().unwrap();
    let () = package.verify_contents(&dir).await.unwrap();
}
