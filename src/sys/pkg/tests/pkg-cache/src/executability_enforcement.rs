// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::TestEnv;
use fidl_fuchsia_io as fio;
use fuchsia_pkg_testing::{Package, PackageBuilder, SystemImageBuilder};

/// Test executability enforcement of fuchsia.pkg/PackageCache.{Get|Open}, i.e. whether the
/// handle to the package directory has [`fio::PERM_EXECUTABLE`]. Enforcement is enabled by default.
///
/// When enabled, the handle should have [`fio::PERM_EXECUTABLE`] if and only if the package is a
/// base package (e.g. being in the cache or retained indices should not affect executability).
///
/// If executability enforcement is disabled (by the presence of file
/// data/pkgfs_disable_executability_restrictions in the meta.far of the system_image package
/// (just the meta.far, the blob can be missing from blobfs)) then the handle should always have
/// [`fio::PERM_EXECUTABLE`].

#[derive(Debug, Clone, Copy)]
enum IsRetained {
    True,
    False,
}

// Creates a blobfs containing `pkg` and `system_image`.
// Optionally adds `pkg` to the retained index.
// Does a Get and Open of `pkg` and compares the handle's flags to `expected_flags`.
async fn verify_package_executability(
    pkg: Package,
    system_image: SystemImageBuilder,
    is_retained: IsRetained,
    expected_flags: fio::Flags,
    superpackage: Package,
    subpackage_url: String,
) {
    let system_image = system_image.build().await;
    let env = TestEnv::builder()
        .blobfs_from_system_image_and_extra_packages(&system_image, &[&pkg, &superpackage])
        .await
        .build()
        .await;

    if let IsRetained::True = is_retained {
        let () = crate::replace_retained_packages(
            &env.proxies.retained_packages,
            &[(*pkg.hash()).into()],
        )
        .await;
    }

    async fn verify_flags(dir: &fio::DirectoryProxy, expected_flags: fio::Flags) {
        let flags = dir.get_flags().await.unwrap().unwrap() & fio::MASK_KNOWN_PERMISSIONS;
        assert_eq!(flags, expected_flags);
    }

    // Verify Get flags
    let dir = crate::verify_package_cached(&env.proxies.package_cache, &pkg).await;
    let () = verify_flags(&dir, expected_flags).await;

    // Verify GetSubpackage flags
    let _super = crate::verify_package_cached(&env.proxies.package_cache, &superpackage).await;
    let dir = crate::verify_get_subpackage(
        &env.proxies.package_cache,
        *superpackage.hash(),
        subpackage_url,
        &pkg,
    )
    .await;
    let () = verify_flags(&dir, expected_flags).await;

    let () = env.stop().await;
}

#[fuchsia::test]
async fn base_package_executable() {
    let pkg = PackageBuilder::new("base-package").build().await.unwrap();
    let superpkg =
        PackageBuilder::new("super").add_subpackage("my-subpackage", &pkg).build().await.unwrap();
    let system_image = SystemImageBuilder::new().static_packages(&[&pkg]);

    let () = verify_package_executability(
        pkg,
        system_image,
        IsRetained::False,
        fio::PERM_READABLE | fio::PERM_EXECUTABLE,
        superpkg,
        "my-subpackage".into(),
    )
    .await;
}

#[fuchsia::test]
async fn cache_package_not_executable() {
    let pkg = PackageBuilder::new("cache-package").build().await.unwrap();
    let superpkg =
        PackageBuilder::new("super").add_subpackage("my-subpackage", &pkg).build().await.unwrap();
    let system_image = SystemImageBuilder::new().cache_packages(&[&pkg]);

    let () = verify_package_executability(
        pkg,
        system_image,
        IsRetained::False,
        fio::PERM_READABLE,
        superpkg,
        "my-subpackage".into(),
    )
    .await;
}

#[fuchsia::test]
async fn retained_index_package_not_executable() {
    let pkg = PackageBuilder::new("retained-package").build().await.unwrap();
    let superpkg =
        PackageBuilder::new("super").add_subpackage("my-subpackage", &pkg).build().await.unwrap();
    let system_image = SystemImageBuilder::new();

    let () = verify_package_executability(
        pkg,
        system_image,
        IsRetained::True,
        fio::PERM_READABLE,
        superpkg,
        "my-subpackage".into(),
    )
    .await;
}

#[fuchsia::test]
async fn enforcement_disabled_cache_package_executable() {
    let pkg = PackageBuilder::new("cache-package").build().await.unwrap();
    let superpkg =
        PackageBuilder::new("super").add_subpackage("my-subpackage", &pkg).build().await.unwrap();
    let system_image = SystemImageBuilder::new()
        .cache_packages(&[&pkg])
        .pkgfs_disable_executability_restrictions();

    let () = verify_package_executability(
        pkg,
        system_image,
        IsRetained::False,
        fio::PERM_READABLE | fio::PERM_EXECUTABLE,
        superpkg,
        "my-subpackage".into(),
    )
    .await;
}

#[fuchsia::test]
async fn enforcement_disabled_retained_index_package_executable() {
    let pkg = PackageBuilder::new("retained-package").build().await.unwrap();
    let superpkg =
        PackageBuilder::new("super").add_subpackage("my-subpackage", &pkg).build().await.unwrap();
    let system_image = SystemImageBuilder::new().pkgfs_disable_executability_restrictions();

    let () = verify_package_executability(
        pkg,
        system_image,
        IsRetained::True,
        fio::PERM_READABLE | fio::PERM_EXECUTABLE,
        superpkg,
        "my-subpackage".into(),
    )
    .await;
}
