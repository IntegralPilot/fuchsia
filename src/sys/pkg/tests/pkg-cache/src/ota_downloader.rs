// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module tests the specific properties of fuchsia.pkg.internal/OtaDownloader.

use fidl_fuchsia_pkg as fpkg;
use std::sync::Arc;
use test_case::{test_case, test_matrix};

#[test_matrix(
    [blobfs_ramdisk::Implementation::CppBlobfs, blobfs_ramdisk::Implementation::Fxblob],
    [false, true]
)]
#[fuchsia::test]
async fn fetch_blob_writes_blobs_if_missing(
    blob_impl: blobfs_ramdisk::Implementation,
    overwrite_existing: bool,
) {
    // TODO(https://fxbug.dev/540501441): Serve the blobs without the package repository.
    let package = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("blob-0", "blob-0-contents".as_bytes())
        .add_resource_at("blob-1", "blob-1-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&package)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let env = crate::TestEnv::builder().blobfs_impl(blob_impl).build().await;
    assert_eq!(env.blobfs.list_blobs().unwrap().intersection(&package.list_blobs()).count(), 0);

    // None of the blobs are present, so all three should be fetched.
    let base_url = served_repository.local_url() + "/blobs/1";
    for blob in package.list_blobs() {
        let size = env.fetch_blob(blob, &base_url, overwrite_existing).await.unwrap();
        assert!(size > 0);
    }
    assert!(env.blobfs.list_blobs().unwrap().is_superset(&package.list_blobs()));
    assert_eq!(served_repository.history().lock().iter().count(), 3);
}

#[test_case(false; "no_overwrite")]
#[test_case(true; "yes_overwrite")]
#[fuchsia::test]
async fn fetch_blob_overwrites_if_blobfs_requests(overwrite_existing: bool) {
    // TODO(https://fxbug.dev/540501441): Serve the blobs without the package repository.
    let package = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("blob-0", "blob-0-contents".as_bytes())
        .add_resource_at("blob-1", "blob-1-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&package)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let env = crate::TestEnv::builder()
        // fuchsia.storage.blobfs/OverwriteConfiguration is only implemented by c++blobfs.
        .cpp_blobfs()
        .build()
        .await;

    let startup_blobs = env.blobfs.list_blobs().unwrap();
    assert_eq!(startup_blobs.intersection(&package.list_blobs()).count(), 0);

    // Write the blobs in padded format, they should not need overwrite.
    let overwrite_configuration = env.blobfs.overwrite_configuration_proxy().unwrap();
    let () = overwrite_configuration
        .set(fidl_fuchsia_storage_blobfs::OverwriteFormat::OverwriteToPadded)
        .await
        .unwrap()
        .unwrap();
    let () = package.write_to_blobfs(&env.blobfs).await;
    let creator = env.blobfs.blob_creator_proxy().unwrap();
    for blob in package.list_blobs() {
        assert!(!creator.needs_overwrite(&blob).await.unwrap().unwrap());
    }

    // Change the desired format to compact, all the blobs should now need overwrite.
    let () = overwrite_configuration
        .set(fidl_fuchsia_storage_blobfs::OverwriteFormat::OverwriteToCompact)
        .await
        .unwrap()
        .unwrap();
    for blob in package.list_blobs() {
        assert!(creator.needs_overwrite(&blob).await.unwrap().unwrap());
    }

    // Fetch the blobs, each fetch should trigger a download (even if overwrite_existing is false),
    // so the returned size should be non-zero.
    let base_url = served_repository.local_url() + "/blobs/1";
    for blob in package.list_blobs() {
        let size = env.fetch_blob(blob, &base_url, overwrite_existing).await.unwrap();
        assert!(size > 0);
    }
    assert_eq!(served_repository.history().lock().iter().count(), 3);

    // Resolution should have overwritten all the blobs so they should no longer need overwrite.
    for blob in package.list_blobs() {
        assert!(!creator.needs_overwrite(&blob).await.unwrap().unwrap());
    }
}

#[test_case(blobfs_ramdisk::Implementation::CppBlobfs; "cpp_blobfs")]
#[test_case(blobfs_ramdisk::Implementation::Fxblob; "fx_blob")]
#[fuchsia::test]
async fn fetch_blob_no_overwrite_does_not_overwrite_if_blobfs_says_up_to_date(
    blob_impl: blobfs_ramdisk::Implementation,
) {
    // TODO(https://fxbug.dev/540501441): Serve the blobs without the package repository.
    let package = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("blob-0", "blob-0-contents".as_bytes())
        .add_resource_at("blob-1", "blob-1-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&package)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let env = crate::TestEnv::builder().blobfs_impl(blob_impl).build().await;
    let () = package.write_to_blobfs(&env.blobfs).await;

    // Without force overwrite, none of the fetches should download anything.
    let base_url = served_repository.local_url() + "/blobs/1";
    for blob in package.list_blobs() {
        let size = env.fetch_blob(blob, &base_url, false).await.unwrap();
        assert_eq!(size, 0);
    }
    assert_eq!(served_repository.history().lock().iter().count(), 0);
}

#[fuchsia::test]
async fn fetch_blob_404() {
    // TODO(https://fxbug.dev/540501441): Serve the blobs without the package repository.
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let env = crate::TestEnv::builder().build().await;

    let base_url = served_repository.local_url() + "/blobs/1";
    std::assert_matches!(
        env.fetch_blob([0; 32].into(), &base_url, false).await,
        Err(fpkg::ResolveError::UnavailableBlob)
    );
}
