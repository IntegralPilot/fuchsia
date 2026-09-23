// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// This module tests the property that pkg_resolver overwrites blobs that Storage says needs
/// overwriting during package resolution.
use {
    fuchsia_pkg_testing::{PackageBuilder, RepositoryBuilder},
    std::sync::Arc,
};

// TODO(https://fxbug.dev/542690944): Already migrated.
#[fuchsia::test]
async fn resolve_overwrites_all_blobs() {
    // fuchsia.storage.blobfs/OverwriteConfiguration is only implemented by c++blobfs.
    let env = lib::TestEnvBuilder::new().cpp_blobfs().build().await;
    let startup_blobs = env.blobfs.list_blobs().unwrap();

    // Create the superpackage that initially has no blobs in blobfs.
    let subpackage = PackageBuilder::new("subpackage")
        .add_resource_at("subpackage-blob", "subpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let superpackage = PackageBuilder::new("superpackage")
        .add_subpackage("my-subpackage", &subpackage)
        .add_resource_at("superpackage-blob", "superpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    assert_eq!(startup_blobs.intersection(&superpackage.list_blobs()).count(), 0);

    // Write the blobs in padded format, they should not need overwrite.
    let overwrite_configuration = env.blobfs.overwrite_configuration_proxy().unwrap();
    let () = overwrite_configuration
        .set(fidl_fuchsia_storage_blobfs::OverwriteFormat::OverwriteToPadded)
        .await
        .unwrap()
        .unwrap();
    let () = superpackage.write_to_blobfs(&env.blobfs).await;
    let creator = env.blobfs.blob_creator_proxy().unwrap();
    for blob in superpackage.list_blobs() {
        assert!(!creator.needs_overwrite(&blob).await.unwrap().unwrap());
    }

    // Change the desired format to compact, all the blobs should now need overwrite.
    let () = overwrite_configuration
        .set(fidl_fuchsia_storage_blobfs::OverwriteFormat::OverwriteToCompact)
        .await
        .unwrap()
        .unwrap();
    for blob in superpackage.list_blobs() {
        assert!(creator.needs_overwrite(&blob).await.unwrap().unwrap());
    }

    // Resolve the package.
    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(lib::EMPTY_REPO_PATH)
            .add_package(&superpackage)
            .add_package(&subpackage)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let repo_config = served_repository.make_repo_config("fuchsia-pkg://test".parse().unwrap());
    let () = env.proxies.repo_manager.add(&repo_config.into()).await.unwrap().unwrap();

    let (_package, _resolved_context) = env
        .resolve_package("fuchsia-pkg://test/superpackage")
        .await
        .expect("package to resolve without error");

    // Resolution should have overwritten all the blobs so they should no longer need overwrite.
    for blob in superpackage.list_blobs() {
        assert!(!creator.needs_overwrite(&blob).await.unwrap().unwrap());
    }

    env.stop().await;
}
