// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module tests the specific properties of the -full package resolver. The remote resolver
//! modules test properties that the -full resolver shares with other capabilities.

use diagnostics_assertions::{AnyProperty, assert_data_tree};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_pkg as fpkg;
use fuchsia_async as fasync;
use futures::stream::StreamExt as _;
use std::collections::HashMap;
use std::sync::Arc;

// Verifies that all fetched blobs are protected from GC by the writing index and open package
// tracking during and after the resolve. Procedure:
//   1. Set the blob fetch concurrency limit to 1.
//   2. After each blob is requested from the blob server (which occurs after a
//      fuchsia.fxfs/BlobWriter connection has been created for the blob), delay sending the
//      response Body (the Header is still sent, which contains the content length, which allows
//      pkg-cache to call BlobWriter.GetVmo).
//   3. While the Body download is paused, trigger a GC and verify that no blobs are deleted.
//   4. After the resolve is completed, trigger a final GC (now that blobs should be protected by
//      open package tracking instead of the writing index) that should still not delete any blobs.
//   5. Drop the handles to the package directories, which should eventually (once the VFS code in
//      pkg-cache that is serving the package directories notices the clients have gone and shuts
//      down) remove open package tracking protection, and verify that the blobs can be deleted (to
//      make sure they weren't being protected by something else).
#[fuchsia::test]
async fn gc_protection() {
    let subsubpackage = fuchsia_pkg_testing::PackageBuilder::new("subsubpackage")
        .add_resource_at("subsubpackage-blob", "subsubpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let subpackage = fuchsia_pkg_testing::PackageBuilder::new("subpackage")
        .add_subpackage("my-subsubpackage", &subsubpackage)
        .add_resource_at("subpackage-blob", "subpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let superpackage = fuchsia_pkg_testing::PackageBuilder::new("superpackage")
        .add_subpackage("my-subpackage", &subpackage)
        .add_resource_at("superpackage-blob", "superpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();

    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&superpackage)
            .build()
            .await
            .unwrap(),
    );
    let (blocker, mut blocked_fetches) =
        fuchsia_pkg_testing::serve::responder::BlockResponseBodies::new();
    let served_repository = Arc::clone(&repo).server().response_overrider(blocker).start().unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&superpackage],
        ))
        .blob_fetch_concurrency_limit(1)
        .build()
        .await;

    let (super_dir, server_end) = fidl::endpoints::create_proxy();
    let super_fut = env
        .proxies
        .full_package_resolver
        .resolve("fuchsia-pkg://example.org/superpackage", server_end);

    let assert_gc_deletes_nothing = || async {
        let before_gc = env.blobfs.list_blobs().unwrap();
        let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
        assert_eq!(before_gc, env.blobfs.list_blobs().unwrap());
    };

    // Three meta.fars, three content blobs.
    for _ in 0..6 {
        let unblocker = blocked_fetches.next().await.unwrap();
        let () = assert_gc_deletes_nothing().await;
        let () = unblocker.unblock();
    }

    // Verify that open package tracking of the superpackage protects all transitive subpackages.
    let super_context = super_fut.await.unwrap().unwrap();
    let () = assert_gc_deletes_nothing().await;
    let () = superpackage.verify_contents(&super_dir).await.unwrap();

    let (sub_dir, sub_context) =
        env.resolve_with_context_full("my-subpackage", &super_context).await.unwrap();
    let () = subpackage.verify_contents(&sub_dir).await.unwrap();

    let (subsub_dir, _subsub_context) =
        env.resolve_with_context_full("my-subsubpackage", &sub_context).await.unwrap();
    let () = subsubpackage.verify_contents(&subsub_dir).await.unwrap();

    // Verify that open package tracking of the subpackages does not protect the supers.
    drop(super_dir);
    loop {
        let () = fasync::Timer::new(std::time::Duration::from_millis(10)).await;
        let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
        if env.blobfs.list_blobs().unwrap().is_disjoint(
            &superpackage.list_blobs().difference(&subpackage.list_blobs()).copied().collect(),
        ) {
            break;
        }
    }
    let () = subpackage.verify_contents(&sub_dir).await.unwrap();
    let () = subsubpackage.verify_contents(&subsub_dir).await.unwrap();

    drop(sub_dir);
    loop {
        let () = fasync::Timer::new(std::time::Duration::from_millis(10)).await;
        let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
        if env.blobfs.list_blobs().unwrap().is_disjoint(
            &subpackage.list_blobs().difference(&subsubpackage.list_blobs()).copied().collect(),
        ) {
            break;
        }
    }
    let () = subsubpackage.verify_contents(&subsub_dir).await.unwrap();

    // Dropping the final subpackage should enable deletion of all resolved blobs.
    drop(subsub_dir);
    loop {
        let () = fasync::Timer::new(std::time::Duration::from_millis(10)).await;
        let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
        if env.blobfs.list_blobs().unwrap().is_disjoint(&superpackage.list_blobs()) {
            break;
        }
    }
}

#[fuchsia::test]
async fn base_pinning() {
    let base_pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("blob", "blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let remote_pkg_same_name = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("different-blob", "different-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let system_image_package =
        fuchsia_pkg_testing::SystemImageBuilder::new().static_packages(&[&base_pkg]).build().await;
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&remote_pkg_same_name)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://fuchsia.com".parse().unwrap());
    let env = crate::TestEnv::builder()
        .blobfs_from_system_image_and_extra_packages(&system_image_package, &[&base_pkg])
        .await
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&remote_pkg_same_name],
        ))
        .build()
        .await;

    // Full resolver should resolve the pkg in base, not the one in the remote repo.
    let (pkg_dir, _context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/test-package").await.unwrap();
    let () = base_pkg.verify_contents(&pkg_dir).await.unwrap();
    std::assert_matches!(remote_pkg_same_name.verify_contents(&pkg_dir).await, Err(_));
    assert_eq!(
        env.get_hash_full("fuchsia-pkg://fuchsia.com/test-package").await.unwrap(),
        *base_pkg.hash()
    );

    // Remote resolver should not have been used in any way.
    assert!(env.blobfs.list_blobs().unwrap().is_disjoint(&remote_pkg_same_name.list_blobs()));
    assert_eq!(*served_repository.history().lock(), vec![]);
    assert_eq!(env.mocks.pkg_authority.get_history_clone(), Vec::<String>::new());

    // Resource fragment rejected
    std::assert_matches!(
        env.resolve_full("fuchsia-pkg://fuchsia.com/test-package#wrong").await,
        Err(fpkg::ResolveError::InvalidUrl)
    );

    // Handles variant
    let (pkg_dir, _context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/test-package/0").await.unwrap();
    let () = base_pkg.verify_contents(&pkg_dir).await.unwrap();

    // Fails mismatched pin.
    std::assert_matches!(
        env.resolve_full(&format!(
            "fuchsia-pkg://fuchsia.com/test-package?hash={}",
            remote_pkg_same_name.hash()
        ))
        .await,
        Err(fpkg::ResolveError::InvalidUrl)
    );
}

#[fuchsia::test]
async fn cache_fallback() {
    let cache_pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("blob", "blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let cache_pkg_repo_not_found = fuchsia_pkg_testing::PackageBuilder::new("repo-not-found")
        .add_resource_at("repo-not-found-blob", "repo-not-found-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let cache_pkg_deprecated_fallback =
        fuchsia_pkg_testing::PackageBuilder::new("deprecated-fallback")
            .add_resource_at(
                "deprecated-fallback-blob",
                "deprecated-fallback-blob-contents".as_bytes(),
            )
            .build()
            .await
            .unwrap();
    let system_image_package = fuchsia_pkg_testing::SystemImageBuilder::new()
        .cache_packages(&[&cache_pkg, &cache_pkg_repo_not_found, &cache_pkg_deprecated_fallback])
        .build()
        .await;
    let env = crate::TestEnv::builder()
        .blobfs_from_system_image_and_extra_packages(
            &system_image_package,
            &[&cache_pkg, &cache_pkg_repo_not_found, &cache_pkg_deprecated_fallback],
        )
        .await
        .pkg_authority(crate::MockPkgAuthority::new(HashMap::from([
            (
                "fuchsia-pkg://fuchsia.com/test-package".to_owned(),
                Err(fpkg::AuthorityLookupError::UpstreamConnection),
            ),
            (
                "fuchsia-pkg://fuchsia.com/test-package/0".to_owned(),
                Err(fpkg::AuthorityLookupError::UpstreamConnection),
            ),
            (
                "fuchsia-pkg://fuchsia.com/repo-not-found".to_owned(),
                Err(fpkg::AuthorityLookupError::RepositoryNotFound),
            ),
            (
                "fuchsia-pkg://fuchsia.com/deprecated-fallback".to_owned(),
                Err(fpkg::AuthorityLookupError::PackageNotFound),
            ),
        ])))
        .build()
        .await;

    // Cache fallback works.
    let (pkg_dir, _context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/test-package").await.unwrap();
    let () = cache_pkg.verify_contents(&pkg_dir).await.unwrap();
    assert_eq!(
        env.get_hash_full("fuchsia-pkg://fuchsia.com/test-package").await.unwrap(),
        *cache_pkg.hash()
    );
    let (pkg_dir, _context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/repo-not-found").await.unwrap();
    let () = cache_pkg_repo_not_found.verify_contents(&pkg_dir).await.unwrap();
    assert_eq!(
        env.get_hash_full("fuchsia-pkg://fuchsia.com/repo-not-found").await.unwrap(),
        *cache_pkg_repo_not_found.hash()
    );

    // Deprecated cache fallback works.
    let (pkg_dir, _context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/deprecated-fallback").await.unwrap();
    let () = cache_pkg_deprecated_fallback.verify_contents(&pkg_dir).await.unwrap();
    assert_eq!(
        env.get_hash_full("fuchsia-pkg://fuchsia.com/deprecated-fallback").await.unwrap(),
        *cache_pkg_deprecated_fallback.hash()
    );

    // Cache fallback works if pin matches.
    let (pkg_dir, _context) = env
        .resolve_full(&format!("fuchsia-pkg://fuchsia.com/test-package?hash={}", cache_pkg.hash()))
        .await
        .unwrap();
    let () = cache_pkg.verify_contents(&pkg_dir).await.unwrap();
    assert_eq!(
        env.get_hash_full(&format!(
            "fuchsia-pkg://fuchsia.com/test-package?hash={}",
            cache_pkg.hash()
        ))
        .await
        .unwrap(),
        *cache_pkg.hash()
    );

    // Cache fallback fails on pin mismatch.
    std::assert_matches!(
        env.resolve_full(&format!(
            "fuchsia-pkg://fuchsia.com/test-package?hash={}",
            cache_pkg_deprecated_fallback.hash()
        ))
        .await,
        Err(fpkg::ResolveError::Io)
    );

    // Cache fallback handles variant.
    let (pkg_dir, _context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/test-package/0").await.unwrap();
    let () = cache_pkg.verify_contents(&pkg_dir).await.unwrap();
    assert_eq!(
        env.get_hash_full("fuchsia-pkg://fuchsia.com/test-package/0").await.unwrap(),
        *cache_pkg.hash()
    );
}

#[fuchsia::test]
async fn cache_fallback_prefers_remote_repo() {
    let cache_pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("cache-blob", "cache-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let remote_pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("remote-blob", "remote-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let system_image_package =
        fuchsia_pkg_testing::SystemImageBuilder::new().cache_packages(&[&cache_pkg]).build().await;
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&remote_pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://fuchsia.com".parse().unwrap());
    let env = crate::TestEnv::builder()
        .blobfs_from_system_image_and_extra_packages(&system_image_package, &[&cache_pkg])
        .await
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&remote_pkg],
        ))
        .build()
        .await;

    // Remote package takes precedence over cache version.
    let (pkg_dir, _context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/test-package").await.unwrap();
    let () = remote_pkg.verify_contents(&pkg_dir).await.unwrap();
    std::assert_matches!(cache_pkg.verify_contents(&pkg_dir).await, Err(_));
    assert_eq!(
        env.get_hash_full("fuchsia-pkg://fuchsia.com/test-package").await.unwrap(),
        *remote_pkg.hash()
    );
}

#[test_case::test_case(true; "enabled")]
#[test_case::test_case(false; "disabled")]
#[fuchsia::test]
async fn executability_enforcement(enforcement_enabled: bool) {
    let base_sub_pkg = fuchsia_pkg_testing::PackageBuilder::new("base-sub-package")
        .add_resource_at("base-sub-blob", "base-sub-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let base_pkg = fuchsia_pkg_testing::PackageBuilder::new("base-package")
        .add_subpackage("my-base-subpackage", &base_sub_pkg)
        .add_resource_at("base-blob", "base-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let cache_sub_pkg = fuchsia_pkg_testing::PackageBuilder::new("cache-sub-package")
        .add_resource_at("cache-sub-blob", "cache-sub-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let cache_pkg = fuchsia_pkg_testing::PackageBuilder::new("cache-package")
        .add_subpackage("my-cache-subpackage", &cache_sub_pkg)
        .add_resource_at("cache-blob", "cache-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let remote_sub_pkg = fuchsia_pkg_testing::PackageBuilder::new("remote-sub-package")
        .add_resource_at("remote-sub-blob", "remote-sub-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let remote_pkg = fuchsia_pkg_testing::PackageBuilder::new("remote-package")
        .add_subpackage("my-remote-subpackage", &remote_sub_pkg)
        .add_resource_at("remote-blob", "remote-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let mut system_image_package = fuchsia_pkg_testing::SystemImageBuilder::new()
        .static_packages(&[&base_pkg])
        .cache_packages(&[&cache_pkg]);
    if !enforcement_enabled {
        system_image_package = system_image_package.pkgfs_disable_executability_restrictions()
    }
    let system_image_package = system_image_package.build().await;
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&remote_pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let env = crate::TestEnv::builder()
        .blobfs_from_system_image_and_extra_packages(
            &system_image_package,
            &[&base_pkg, &cache_pkg],
        )
        .await
        .pkg_authority(
            crate::MockPkgAuthority::from_repo_config_and_packages(&repo_config, &[&remote_pkg])
                .add_lookup_response(
                    "fuchsia-pkg://fuchsia.com/cache-package",
                    Err(fpkg::AuthorityLookupError::UpstreamConnection),
                ),
        )
        .build()
        .await;

    async fn get_flags(dir: &fio::DirectoryProxy) -> fio::Flags {
        dir.get_flags().await.unwrap().unwrap() & fio::MASK_KNOWN_PERMISSIONS
    }

    let (base_dir, base_context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/base-package").await.unwrap();
    assert_eq!(get_flags(&base_dir).await, fio::PERM_READABLE | fio::PERM_EXECUTABLE);
    let (base_sub_dir, _) =
        env.resolve_with_context_full("my-base-subpackage", &base_context).await.unwrap();
    assert_eq!(get_flags(&base_sub_dir).await, fio::PERM_READABLE | fio::PERM_EXECUTABLE);

    let non_base_expected_flags = if enforcement_enabled {
        fio::PERM_READABLE
    } else {
        fio::PERM_READABLE | fio::PERM_EXECUTABLE
    };

    let (cache_dir, cache_context) =
        env.resolve_full("fuchsia-pkg://fuchsia.com/cache-package").await.unwrap();
    assert_eq!(get_flags(&cache_dir).await, non_base_expected_flags);
    let (cache_sub_dir, _) =
        env.resolve_with_context_full("my-cache-subpackage", &cache_context).await.unwrap();
    assert_eq!(get_flags(&cache_sub_dir).await, non_base_expected_flags);

    let (remote_dir, remote_context) =
        env.resolve_full("fuchsia-pkg://example.org/remote-package").await.unwrap();
    assert_eq!(get_flags(&remote_dir).await, non_base_expected_flags);
    let (remote_sub_dir, _) =
        env.resolve_with_context_full("my-remote-subpackage", &remote_context).await.unwrap();
    assert_eq!(get_flags(&remote_sub_dir).await, non_base_expected_flags);
}

#[fuchsia::test]
async fn inspect_success() {
    let package = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("blob", "blob-contents".as_bytes())
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
    let (blocker, mut blocked_fetches) =
        fuchsia_pkg_testing::serve::responder::BlockResponseBodies::new();
    let served_repository = Arc::clone(&repo).server().response_overrider(blocker).start().unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&package],
        ))
        .build()
        .await;

    let (_package_dir, server_end) = fidl::endpoints::create_proxy();
    let package_fut = env
        .proxies
        .full_package_resolver
        .resolve("fuchsia-pkg://example.org/test-package", server_end);

    // Let the meta.far fetch complete and wait for the content blob to be requested.
    let () = blocked_fetches.next().await.unwrap().unblock();
    let unblocker = blocked_fetches.next().await.unwrap();

    assert_data_tree!(
        env.inspect_hierarchy().await,
        "root": contains {
            "package_fetcher": {
                "0": {
                    "start_boot_ns": AnyProperty,
                    "package_hash": package.hash().to_string(),
                    "gc_protection": "OpenPackageTracking",
                    "blob_source": format!("{}/1", repo_config.mirrors()[0].blob_mirror_url()),
                    "known_outstanding_blobs": 1u64,
                }
            },
            "fuchsia.pkg.PackageResolver-full": {
                "active": {
                    "0": {
                        "start_boot_ns": AnyProperty,
                        "url": "fuchsia-pkg://example.org/test-package",
                        "authority": "tuf",
                        "blob_source":
                            format!("Some({}/1)", repo_config.mirrors()[0].blob_mirror_url()),
                        "hash": package.hash().to_string(),
                    },
                },
                "recent": {},
            }
        }
    );

    // Finish the resolve.
    let () = unblocker.unblock();
    let _context = package_fut.await.unwrap().unwrap();

    assert_data_tree!(
        env.inspect_hierarchy().await,
        "root": contains {
            "package_fetcher": {},
            "fuchsia.pkg.PackageResolver-full": {
                "active": {},
                "recent": {
                    "0": {
                        "0": {
                            "start_boot_ns": AnyProperty,
                            "url": "fuchsia-pkg://example.org/test-package",
                            "authority": "tuf",
                            "blob_source":
                                format!("Some({}/1)", repo_config.mirrors()[0].blob_mirror_url()),
                            "hash": package.hash().to_string(),
                            "result": "success",
                            "end_boot_ns": AnyProperty,
                        }
                    },
                },
            }
        }
    );
}

#[fuchsia::test]
async fn inspect_failure() {
    let package = fuchsia_pkg_testing::PackageBuilder::new("test-package").build().await.unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&package)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo)
        .server()
        .response_overrider(
            fuchsia_pkg_testing::serve::responder::StaticResponseCode::server_error(),
        )
        .start()
        .unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&package],
        ))
        .build()
        .await;

    std::assert_matches!(env.resolve_full("fuchsia-pkg://example.org/test-package").await, Err(_));

    assert_data_tree!(
        env.inspect_hierarchy().await,
        "root": contains {
            "package_fetcher": {},
            "fuchsia.pkg.PackageResolver-full": {
                "active": {},
                "recent": {
                    "0": {
                        "0": {
                            "start_boot_ns": AnyProperty,
                            "url": "fuchsia-pkg://example.org/test-package",
                            "authority": "tuf",
                            "blob_source":
                                format!("Some({}/1)", repo_config.mirrors()[0].blob_mirror_url()),
                            "hash": package.hash().to_string(),
                            "result":
                                "error: forwarding to package fetcher: fetching blob: error while \
                                 calling fuchsia.pkg.http.Client.DownloadBlob Network",
                            "end_boot_ns": AnyProperty,
                        }
                    },
                },
            }
        }
    );
}
