// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module tests the -full package resolver's ability to resolve upgradable packages.

use fidl_fuchsia_pkg as fpkg;

#[fuchsia::test]
async fn set_then_resolve() {
    let env = crate::TestEnv::builder().enable_upgradable_packages().build().await;
    let upgradable_pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("upgradable-blob", "upgradable-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let () = upgradable_pkg.write_to_blobfs(&env.blobfs).await;

    let () = env.set_upgradable_urls([upgradable_pkg.pinned_fuchsia_url()]).await.unwrap();
    let (upgradable_dir, _context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/test-package").await.unwrap();

    let () = upgradable_pkg.verify_contents(&upgradable_dir).await.unwrap();
}

#[fuchsia::test]
async fn cache_fallback_then_upgrade() {
    let cache_fallback_pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("fallback-blob", "fallback-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let system_image_pkg = fuchsia_pkg_testing::SystemImageBuilder::new()
        .cache_packages(&[&cache_fallback_pkg])
        .build()
        .await;
    let env = crate::TestEnv::builder()
        .enable_upgradable_packages()
        .blobfs_from_system_image_and_extra_packages(&system_image_pkg, &[&cache_fallback_pkg])
        .await
        .build()
        .await;

    // Full resolver should resolve the fallback (this is not using the TUF-resolver cache fallback
    // because the mock tuf authority has not been configured).
    let () = env.set_upgradable_urls([] as [&str; 0]).await.unwrap();
    let (fallback_dir, _context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/test-package").await.unwrap();
    let () = cache_fallback_pkg.verify_contents(&fallback_dir).await.unwrap();
    assert_eq!(env.mocks.pkg_authority.get_history_clone(), Vec::<String>::new());

    // Full resolver should resolve the new version after it is registered.
    let upgraded_pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("upgraded-blob", "upgraded-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let () = upgraded_pkg.write_to_blobfs(&env.blobfs).await;
    let () = env.set_upgradable_urls([upgraded_pkg.pinned_fuchsia_url()]).await.unwrap();
    let (upgraded_dir, _context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/test-package").await.unwrap();
    let () = upgraded_pkg.verify_contents(&upgraded_dir).await.unwrap();
    std::assert_matches!(cache_fallback_pkg.verify_contents(&upgraded_dir).await, Err(_));
}

#[fuchsia::test]
async fn set_upgradable_urls_does_not_block_base_packages() {
    let base_pkg =
        fuchsia_pkg_testing::PackageBuilder::new("a-base-package").build().await.unwrap();
    let system_image_pkg =
        fuchsia_pkg_testing::SystemImageBuilder::new().static_packages(&[&base_pkg]).build().await;
    let env = crate::TestEnv::builder()
        .enable_upgradable_packages()
        .blobfs_from_system_image_and_extra_packages(&system_image_pkg, &[&base_pkg])
        .await
        .build()
        .await;

    // Base packages can be resolved before `set_upgradable_urls` is called.
    let (base_dir, _context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/a-base-package").await.unwrap();
    let () = base_pkg.verify_contents(&base_dir).await.unwrap();
}

#[fuchsia::test]
async fn set_upgradable_urls_ignore_base_packages_and_invalid_urls() {
    let base_pkg =
        fuchsia_pkg_testing::PackageBuilder::new("a-base-package").build().await.unwrap();
    let upgradable_pkg =
        fuchsia_pkg_testing::PackageBuilder::new("upgradable-package").build().await.unwrap();
    let system_image_pkg =
        fuchsia_pkg_testing::SystemImageBuilder::new().static_packages(&[&base_pkg]).build().await;
    let env = crate::TestEnv::builder()
        .enable_upgradable_packages()
        .blobfs_from_system_image_and_extra_packages(
            &system_image_pkg,
            &[&base_pkg, &upgradable_pkg],
        )
        .await
        .build()
        .await;

    std::assert_matches!(
        env.set_upgradable_urls([
            base_pkg.pinned_fuchsia_url().to_string(),
            upgradable_pkg.pinned_fuchsia_url().to_string(),
            "".into(),
            "http://fuchsia.com/wrong-scheme".into(),
            "fuchsia-pkg://fuchsia.com/unpinned".into()
        ])
        .await,
        Err(fpkg::SetUpgradableUrlsError::PartialSet)
    );

    // Upgradable package resolution is unblocked even though set returned Err(PartialSet).
    let (dir, _context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/upgradable-package").await.unwrap();
    let () = upgradable_pkg.verify_contents(&dir).await.unwrap();
}
