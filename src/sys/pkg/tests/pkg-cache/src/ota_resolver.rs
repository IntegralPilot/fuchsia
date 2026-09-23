// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module tests the specific properties of the -ota package resolver. The remote resolver
//! modules test properties that the -ota resolver shares with other capabilities.

use diagnostics_assertions::{AnyProperty, assert_data_tree};
use fuchsia_async as fasync;
use futures::stream::StreamExt as _;
use std::collections::BTreeSet;
use std::sync::Arc;

#[fuchsia::test]
async fn resolve_overwrites_all_blobs() {
    // Create the superpackage that initially has no blobs in blobfs.
    let subpackage = fuchsia_pkg_testing::PackageBuilder::new("subpackage")
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
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&superpackage],
        ))
        // fuchsia.storage.blobfs/OverwriteConfiguration is only implemented by c++blobfs.
        .cpp_blobfs()
        .build()
        .await;

    let startup_blobs = env.blobfs.list_blobs().unwrap();
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

    let (superpackage_dir, context) =
        env.resolve_ota("fuchsia-pkg://example.org/superpackage").await.unwrap();
    let () = superpackage.verify_contents(&superpackage_dir).await.unwrap();

    // Resolution should have overwritten all the blobs so they should no longer need overwrite.
    for blob in superpackage.list_blobs() {
        assert!(!creator.needs_overwrite(&blob).await.unwrap().unwrap());
    }

    let (subpackage_dir, _context) =
        env.resolve_with_context_ota("my-subpackage", &context).await.unwrap();
    let () = subpackage.verify_contents(&subpackage_dir).await.unwrap();
}

#[fuchsia::test]
async fn does_not_use_open_package_tracking() {
    let pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("blob", "blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&pkg],
        ))
        .build()
        .await;
    let (pkg_dir, _context) =
        env.resolve_ota("fuchsia-pkg://example.org/test-package").await.unwrap();
    let () = pkg.verify_contents(&pkg_dir).await.unwrap();

    // The package blobs are all in blobfs.
    assert!(env.blobfs.list_blobs().unwrap().is_superset(&pkg.list_blobs()));

    // After GC, the package blobs are all removed from blobfs, even though the handle to the
    // package directory, `pkg_dir`, is still alive.
    let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
    assert_eq!(env.blobfs.list_blobs().unwrap().intersection(&pkg.list_blobs()).count(), 0);
}

// Verifies that all fetched blobs are protected from GC by the retained index during and after the
// resolve. Procedure:
//   1. Set the blob fetch concurrency limit to 1.
//   2. After each blob is requested from the blob server (which occurs after a
//      fuchsia.fxfs/BlobWriter connection has been created for the blob), delay sending the
//      response Body (the Header is still sent, which contains the content length, which allows
//      pkg-cache to call BlobWriter.GetVmo).
//   3. While the Body download is paused, trigger a GC and verify that no blobs are deleted.
//   4. After the resolve is completed, trigger a final GC (now that blobs will not be protected by
//      blobfs' open blob protection) that should still not delete any blobs.
//   5. Clear the retained index and verify that the blobs can be deleted (to make sure they weren't
//      being protected by something else).
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

    crate::replace_retained_packages(
        &env.proxies.retained_packages,
        &[(*superpackage.hash()).into()],
    )
    .await;
    let (super_dir, server_end) = fidl::endpoints::create_proxy();
    let super_fut = env
        .proxies
        .ota_package_resolver
        .resolve("fuchsia-pkg://example.org/superpackage", server_end);

    // Three meta.fars, three content blobs.
    for _ in 0..6 {
        let unblocker = blocked_fetches.next().await.unwrap();
        let before_gc = env.blobfs.list_blobs().unwrap();
        let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
        assert_eq!(before_gc, env.blobfs.list_blobs().unwrap());
        let () = unblocker.unblock();
    }
    // Wait for the last blob to be fully written.
    while !env.blobfs.list_blobs().unwrap().is_superset(&superpackage.list_blobs()) {
        let () = fasync::Timer::new(std::time::Duration::from_millis(10)).await;
    }
    let before_gc = env.blobfs.list_blobs().unwrap();
    let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
    assert_eq!(before_gc, env.blobfs.list_blobs().unwrap());

    // The packages should still be readable.
    let context = super_fut.await.unwrap().unwrap();
    let () = superpackage.verify_contents(&super_dir).await.unwrap();

    let (sub_dir, context) = env.resolve_with_context_ota("my-subpackage", &context).await.unwrap();
    let () = subpackage.verify_contents(&sub_dir).await.unwrap();

    let (subsub_dir, _context) =
        env.resolve_with_context_ota("my-subsubpackage", &context).await.unwrap();
    let () = subsubpackage.verify_contents(&subsub_dir).await.unwrap();

    // After clearing the retained index, the blobs should be deletable.
    crate::replace_retained_packages(&env.proxies.retained_packages, &[]).await;
    let before_gc = env.blobfs.list_blobs().unwrap();
    let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
    let removed =
        before_gc.difference(&env.blobfs.list_blobs().unwrap()).copied().collect::<BTreeSet<_>>();
    assert_eq!(removed, superpackage.list_blobs());
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
        .ota_package_resolver
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
                    "gc_protection": "Retained",
                    "blob_source": format!("{}/1", repo_config.mirrors()[0].blob_mirror_url()),
                    "known_outstanding_blobs": 1u64,
                }
            },
            "fuchsia.pkg.PackageResolver-ota": {
                "active": {
                    "0": {
                        "start_boot_ns": AnyProperty,
                        "url": "fuchsia-pkg://example.org/test-package",
                        "blob_source": format!("{}/1", repo_config.mirrors()[0].blob_mirror_url()),
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
            "fuchsia.pkg.PackageResolver-ota": {
                "active": {},
                "recent": {
                    "0": {
                        "0": {
                            "start_boot_ns": AnyProperty,
                            "url": "fuchsia-pkg://example.org/test-package",
                            "blob_source":
                                format!("{}/1", repo_config.mirrors()[0].blob_mirror_url()),
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

    std::assert_matches!(env.resolve_ota("fuchsia-pkg://example.org/test-package").await, Err(_));

    assert_data_tree!(
        env.inspect_hierarchy().await,
        "root": contains {
            "package_fetcher": {},
            "fuchsia.pkg.PackageResolver-ota": {
                "active": {},
                "recent": {
                    "0": {
                        "0": {
                            "start_boot_ns": AnyProperty,
                            "url": "fuchsia-pkg://example.org/test-package",
                            "blob_source":
                                format!("{}/1", repo_config.mirrors()[0].blob_mirror_url()),
                            "hash": package.hash().to_string(),
                            "result":
                                "error: forwarding to the package fetcher: fetching blob: error \
                                 while calling fuchsia.pkg.http.Client.DownloadBlob Network",
                            "end_boot_ns": AnyProperty,
                        }
                    },
                },
            }
        }
    );
}
