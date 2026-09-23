// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::TestEnv;
use fidl_fuchsia_io as fio;
use fuchsia_pkg_testing::{PackageBuilder, SystemImageBuilder};

#[fuchsia::test]
async fn expose_pkgfs_ctl_validation_missing_file() {
    let blobfs = blobfs_ramdisk::BlobfsRamdisk::builder().impl_from_env().start().await.unwrap();
    let base_package_with_missing_blob = PackageBuilder::new("has-missing-blob")
        .add_resource_at("missing-blob", b"missing-blob-contents".as_slice())
        .build()
        .await
        .unwrap();
    let system_image_package =
        SystemImageBuilder::new().static_packages(&[&base_package_with_missing_blob]).build().await;
    base_package_with_missing_blob.write_to_blobfs(&blobfs).await;
    system_image_package.write_to_blobfs(&blobfs).await;
    let mut missing_blob = None;
    for blob_info in base_package_with_missing_blob.content_blob_files() {
        let hash = blob_info.merkle.to_string();
        let () = blobfs
            .root_dir_proxy()
            .unwrap()
            .unlink(&hash, &fio::UnlinkOptions::default())
            .await
            .unwrap()
            .unwrap();
        assert!(missing_blob.is_none());
        missing_blob = Some(hash);
    }
    let env = TestEnv::builder()
        .blobfs_and_system_image_hash(blobfs, Some(*system_image_package.hash()))
        .build()
        .await;

    let missing = fuchsia_fs::directory::open_file(
        &env.proxies.pkgfs,
        "ctl/validation/missing",
        fio::PERM_READABLE,
    )
    .await
    .unwrap();

    assert_eq!(
        fuchsia_fs::file::read(&missing).await.unwrap(),
        format!("{}\n", missing_blob.unwrap()).into_bytes()
    );

    let () = env.stop().await;
}

#[fuchsia::test]
async fn expose_system_image_package_as_system_directory() {
    let system_image_package = SystemImageBuilder::new().build().await;
    let env =
        TestEnv::builder().blobfs_from_system_image(&system_image_package).await.build().await;

    system_image_package.verify_contents(&env.system_dir().await).await.unwrap();

    let () = env.stop().await;
}
