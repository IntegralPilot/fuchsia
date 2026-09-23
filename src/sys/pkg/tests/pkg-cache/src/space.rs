// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{
    TestEnv, WriteErrorExt as _, blob_written, compress_and_write_blob, get_missing_blobs,
};
use assert_matches::assert_matches;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_paver as fpaver;
use fidl_fuchsia_pkg::{self as fpkg, NeededBlobsMarker};
use fidl_fuchsia_pkg_ext::BlobId;
use fidl_fuchsia_pkg_garbagecollector::GcError;
use fuchsia_async as fasync;
use fuchsia_pkg_testing::{PackageBuilder, SystemImageBuilder};
use futures::TryFutureExt as _;
use mock_paver::{MockPaverServiceBuilder, PaverEvent, hooks as mphooks};
use rand::prelude::*;
use std::collections::BTreeSet;
use zx::{self as zx, Status};

#[fuchsia::test]
async fn gc_error_pending_commit() {
    let (throttle_hook, throttler) = mphooks::throttle();

    let system_image_package = SystemImageBuilder::new().build().await;
    let env = TestEnv::builder()
        .blobfs_from_system_image(&system_image_package)
        .await
        .paver_service_builder(
            MockPaverServiceBuilder::new().insert_hook(throttle_hook).insert_hook(
                mphooks::config_status_and_boot_attempts(|_| {
                    Ok((fpaver::ConfigurationStatus::Pending, Some(1)))
                }),
            ),
        )
        .build()
        .await;

    // Allow the paver to emit enough events to unblock the CommitStatusProvider FIDL server, but
    // few enough to guarantee the commit is still pending.
    let () = throttler.emit_next_paver_events(&[
        PaverEvent::QueryCurrentConfiguration,
        PaverEvent::QueryConfigurationStatusAndBootAttempts {
            configuration: fpaver::Configuration::A,
        },
    ]);
    assert_matches!(env.proxies.space_manager.gc().await, Ok(Err(GcError::PendingCommit)));

    // When the commit completes, GC should unblock as well.
    let () = throttler.emit_next_paver_events(&[
        PaverEvent::SetConfigurationHealthy { configuration: fpaver::Configuration::A },
        PaverEvent::SetConfigurationUnbootable { configuration: fpaver::Configuration::B },
        PaverEvent::BootManagerFlush,
    ]);
    let event_pair =
        env.proxies.commit_status_provider.is_current_system_committed().await.unwrap();
    assert_eq!(
        fasync::OnSignals::new(&event_pair, zx::Signals::USER_0).await,
        Ok(zx::Signals::USER_0)
    );
    assert_matches!(env.proxies.space_manager.gc().await, Ok(Ok(())));
}

/// Assert that performing a GC does nothing on a blobfs that only includes the system image and
/// static packages.
#[fuchsia::test]
async fn gc_noop_system_image() {
    let static_package = PackageBuilder::new("static-package")
        .add_resource_at("resource", &[][..])
        .build()
        .await
        .unwrap();
    let system_image = SystemImageBuilder::new().static_packages(&[&static_package]).build().await;
    let env = TestEnv::builder()
        .blobfs_from_system_image_and_extra_packages(&system_image, &[&static_package])
        .await
        .build()
        .await;
    let () = env.block_until_started().await;

    let mut expected_base_blobs = static_package.list_blobs();
    expected_base_blobs.extend(system_image.list_blobs());
    // static-package meta.far and content blob, system_image meta.far and static packages manifest
    assert_eq!(expected_base_blobs.len(), 4);
    assert_eq!(env.blobfs.list_blobs().unwrap(), expected_base_blobs);

    assert_matches!(env.proxies.space_manager.gc().await, Ok(Ok(())));

    assert_eq!(env.blobfs.list_blobs().unwrap(), expected_base_blobs);
}

/// Assert that any blobs referenced by cache packages are ineligible for garbage collection.
#[fuchsia::test]
async fn gc_cache_packages_protected() {
    let cache_subpkg = PackageBuilder::new("cache-subpkg")
        .add_resource_at("sub-blob", "sub-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let cache_pkg = PackageBuilder::new("cache-pkg")
        .add_subpackage("cache-subpkg", &cache_subpkg)
        .add_resource_at("cache-blob", "cache-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let system_image = SystemImageBuilder::new().cache_packages(&[&cache_pkg]).build().await;
    let env = TestEnv::builder()
        .blobfs_from_system_image_and_extra_packages(&system_image, &[&cache_pkg])
        .await
        .build()
        .await;

    // All blobs required for the cache package should be in blobfs.
    let protected_blobs = cache_pkg.list_blobs();
    assert!(env.blobfs.list_blobs().unwrap().is_superset(&protected_blobs));

    // All blobs are still present after gc.
    let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
    assert!(env.blobfs.list_blobs().unwrap().is_superset(&protected_blobs));
}

#[fuchsia::test]
async fn gc_unowned_blob() {
    let env = TestEnv::builder().build().await;
    let unowned_content = b"blob not referenced by any protected packages";
    let unowned_hash = fuchsia_merkle::root_from_slice(unowned_content);
    let () = env.write_to_blobfs(&unowned_hash, unowned_content).await;
    assert!(env.blobfs.list_blobs().unwrap().contains(&unowned_hash));

    let () = env.proxies.space_manager.gc().await.unwrap().unwrap();

    assert!(!env.blobfs.list_blobs().unwrap().contains(&unowned_hash));
}

async fn gc_frees_space_so_write_can_succeed(blob_implementation: blobfs_ramdisk::Implementation) {
    // Create a 7 MB blobfs (14,336 blocks * 512 bytes / block).
    let small_blobfs = blobfs_ramdisk::Ramdisk::builder()
        .block_count(14336)
        .into_blobfs_builder()
        .await
        .expect("made blobfs builder")
        .implementation(blob_implementation)
        .start()
        .await
        .expect("started blobfs");

    // Write an orphaned incompressible 4 MB blob.
    let mut orphan_data = vec![0; 4 * 1024 * 1024];
    StdRng::from_seed([0u8; 32]).fill(&mut orphan_data[..]);
    let orphan_hash = fuchsia_merkle::root_from_slice(&orphan_data);
    let () = small_blobfs.add_blob_from(orphan_hash, &orphan_data[..]).await.unwrap();
    assert!(small_blobfs.list_blobs().unwrap().contains(&orphan_hash));

    // Create a TestEnv using this blobfs.
    let system_image_package = SystemImageBuilder::new().build().await;
    system_image_package.write_to_blobfs(&small_blobfs).await;
    let env = TestEnv::builder()
        .blobfs_and_system_image_hash(small_blobfs, Some(*system_image_package.hash()))
        .blobfs_impl(blob_implementation)
        .build()
        .await;

    // Try to cache a package with an incompressible 4 MB meta.far.
    let pkg = PackageBuilder::new("pkg-a")
        .add_resource_at("meta/asset", &orphan_data[..])
        .build()
        .await
        .expect("build large package");
    assert_ne!(*pkg.hash(), orphan_hash);
    let meta_blob_info = fpkg::BlobInfo { blob_id: BlobId::from(*pkg.hash()).into(), length: 0 };
    let (needed_blobs, needed_blobs_server_end) =
        fidl::endpoints::create_proxy::<NeededBlobsMarker>();
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
        .map_ok(|res| res.map_err(Status::err_from_raw));

    // Writing the meta.far should fail with NO_SPACE.
    let (meta_far, _contents) = pkg.contents();
    let meta_blob = needed_blobs.open_meta_blob().await.unwrap().unwrap().unwrap().into_proxy();
    let () = compress_and_write_blob(&meta_far.contents, meta_blob)
        .await
        .unwrap_err()
        .assert_out_of_space();

    // GC should free space, allowing the meta.far write and therefore get to succeed.
    let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
    assert!(!env.blobfs.list_blobs().unwrap().contains(&orphan_hash));
    let meta_blob = needed_blobs.open_meta_blob().await.unwrap().unwrap().unwrap().into_proxy();
    let () = compress_and_write_blob(&meta_far.contents, meta_blob).await.unwrap();
    let () = blob_written(&needed_blobs, meta_far.merkle).await;
    let (_, blob_iterator_server_end) =
        fidl::endpoints::create_proxy::<fpkg::BlobInfoIteratorMarker>();
    let () = needed_blobs.get_missing_blobs(blob_iterator_server_end).unwrap();

    let () = get_fut.await.unwrap().unwrap();
    let () = pkg.verify_contents(&dir).await.unwrap();
}

#[fuchsia::test]
async fn gc_frees_space_so_write_can_succeed_cpp_blobfs() {
    let () = gc_frees_space_so_write_can_succeed(blobfs_ramdisk::Implementation::CppBlobfs).await;
}

#[fuchsia::test]
async fn gc_frees_space_so_write_can_succeed_fxblob() {
    let () = gc_frees_space_so_write_can_succeed(blobfs_ramdisk::Implementation::Fxblob).await;
}

async fn blobs_protected_from_gc_during_get(gc_protection: fpkg::GcProtection) {
    let env = TestEnv::builder().build().await;
    let initial_blobs = env.blobfs.list_blobs().unwrap();

    let subsubpackage = PackageBuilder::new("subsubpackage")
        .add_resource_at("subsubpackage-blob", "subsubpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let subpackage = PackageBuilder::new("subpackage")
        .add_subpackage("my-subsubpackage", &subsubpackage)
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

    // Verify that none of the to-be-fetched blobs are in blobfs.
    let to_be_fetched: Vec<(fuchsia_merkle::Hash, Vec<u8>)> = vec![
        (*superpackage.hash(), superpackage.contents().0.contents.clone()),
        superpackage.contents().1.into_iter().next().unwrap(),
        (*subpackage.hash(), subpackage.contents().0.contents.clone()),
        subpackage.contents().1.into_iter().next().unwrap(),
        (*subsubpackage.hash(), subsubpackage.contents().0.contents),
        subsubpackage.contents().1.into_iter().next().unwrap(),
    ];
    let to_be_fetched_hashes = BTreeSet::from_iter(to_be_fetched.iter().map(|(hash, _)| *hash));
    assert_eq!(to_be_fetched_hashes.len(), 6);
    assert!(initial_blobs.is_disjoint(&to_be_fetched_hashes));

    // Verify that none of the to-be-fetched blobs are protected yet.
    for (hash, bytes) in to_be_fetched.iter() {
        let () = env.blobfs.write_blob(*hash, bytes).await.unwrap();
    }
    assert!(env.blobfs.list_blobs().unwrap().is_superset(&to_be_fetched_hashes));
    let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
    assert!(env.blobfs.list_blobs().unwrap().is_disjoint(&to_be_fetched_hashes));

    // Start the Get.
    match gc_protection {
        fpkg::GcProtection::Retained => {
            crate::replace_retained_packages(
                &env.proxies.retained_packages,
                &[(*superpackage.hash()).into()],
            )
            .await
        }
        fpkg::GcProtection::OpenPackageTracking => (),
    }
    let meta_blob_info =
        fpkg::BlobInfo { blob_id: BlobId::from(*superpackage.hash()).into(), length: 0 };
    let (needed_blobs, needed_blobs_server) = fidl::endpoints::create_proxy::<NeededBlobsMarker>();
    let (dir, dir_server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    let get_fut = env
        .proxies
        .package_cache
        .get(&meta_blob_info, gc_protection, needed_blobs_server, dir_server)
        .map_ok(|res| res.map_err(Status::err_from_raw));

    let blob_is_present_and_protected = |i: usize| {
        let (i, env, to_be_fetched) = (i, &env, &to_be_fetched);
        async move {
            assert!(env.blobfs.list_blobs().unwrap().contains(&to_be_fetched[i].0));
            let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
            assert!(env.blobfs.list_blobs().unwrap().contains(&to_be_fetched[i].0));
        }
    };

    // Write the superpackage meta.far.
    let meta_blob = needed_blobs.open_meta_blob().await.unwrap().unwrap().unwrap().into_proxy();
    let () = compress_and_write_blob(&to_be_fetched[0].1, meta_blob).await.unwrap();
    let () = blob_written(&needed_blobs, to_be_fetched[0].0).await;
    let () = blob_is_present_and_protected(0).await;

    // Read the superpackage content blob and subpackage meta.far from the missing blobs iterator
    // to guarantee that pkg-cache is ready for them to be written.
    let (blob_iterator, blob_iterator_server_end) =
        fidl::endpoints::create_proxy::<fpkg::BlobInfoIteratorMarker>();
    let () = needed_blobs.get_missing_blobs(blob_iterator_server_end).unwrap();
    assert_eq!(
        blob_iterator.next().await.unwrap(),
        vec![
            fpkg::BlobInfo { blob_id: BlobId::from(to_be_fetched[2].0).into(), length: 0 },
            fpkg::BlobInfo { blob_id: BlobId::from(to_be_fetched[1].0).into(), length: 0 },
        ]
    );

    let write_blob = |i: usize| {
        let (i, needed_blobs, to_be_fetched) = (i, &needed_blobs, &to_be_fetched);
        async move {
            let blob = needed_blobs
                .open_blob(&BlobId::from(to_be_fetched[i].0).into(), false)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .into_proxy();
            let () = compress_and_write_blob(&to_be_fetched[i].1, blob).await.unwrap();
            let () = blob_written(needed_blobs, to_be_fetched[i].0).await;
        }
    };

    // Write the superpackage content blob.
    let () = write_blob(1).await;
    let () = blob_is_present_and_protected(1).await;

    // Write the subpackage meta.far.
    let () = write_blob(2).await;
    let () = blob_is_present_and_protected(2).await;

    // Prepare pkg-cache for the subpackage content blob and subsubpackage meta.far.
    assert_eq!(
        blob_iterator.next().await.unwrap(),
        vec![
            fpkg::BlobInfo { blob_id: BlobId::from(to_be_fetched[4].0).into(), length: 0 },
            fpkg::BlobInfo { blob_id: BlobId::from(to_be_fetched[3].0).into(), length: 0 },
        ]
    );

    // Write the subpackage content blob.
    let () = write_blob(3).await;
    let () = blob_is_present_and_protected(3).await;

    // Write the subsubpackage meta.far.
    let () = write_blob(4).await;
    let () = blob_is_present_and_protected(4).await;

    // Prepare pkg-cache for the subsubpackage content blob.
    assert_eq!(
        blob_iterator.next().await.unwrap(),
        vec![fpkg::BlobInfo { blob_id: BlobId::from(to_be_fetched[5].0).into(), length: 0 },]
    );

    // Write the subsubpackage content blob.
    let () = write_blob(5).await;
    let () = blob_is_present_and_protected(5).await;

    // Complete the Get.
    assert_eq!(blob_iterator.next().await.unwrap(), vec![]);
    let () = get_fut.await.unwrap().unwrap();
    let () = superpackage.verify_contents(&dir).await.unwrap();

    // All blobs should still be protected.
    let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
    assert!(env.blobfs.list_blobs().unwrap().is_superset(&to_be_fetched_hashes));

    // Without the protection gc should delete all the blobs.
    match gc_protection {
        fpkg::GcProtection::Retained => {
            crate::replace_retained_packages(&env.proxies.retained_packages, &[]).await
        }
        fpkg::GcProtection::OpenPackageTracking => {
            drop(dir);
            let () = env.wait_for_package_to_close(superpackage.hash()).await;
        }
    }
    let () = env.proxies.space_manager.gc().await.unwrap().unwrap();
    assert!(env.blobfs.list_blobs().unwrap().is_disjoint(&to_be_fetched_hashes));

    let () = env.stop().await;
}

#[fuchsia::test]
async fn blobs_protected_from_gc_during_get_by_retained_index() {
    let () = blobs_protected_from_gc_during_get(fpkg::GcProtection::Retained).await;
}

#[fuchsia::test]
async fn blobs_protected_from_gc_during_get_by_writing_index() {
    let () = blobs_protected_from_gc_during_get(fpkg::GcProtection::OpenPackageTracking).await;
}

#[fuchsia::test]
async fn blobs_protected_from_gc_by_open_package_tracking() {
    let env = TestEnv::builder().build().await;
    let () = env.block_until_started().await;

    let subpkg0 = PackageBuilder::new("open-subpackage0")
        .add_resource_at("content-blob", "open-subpackage0-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let pkg0 = PackageBuilder::new("open-package")
        .add_subpackage("subpackage", &subpkg0)
        .add_resource_at("content-blob", "v0-contents".as_bytes())
        .build()
        .await
        .unwrap();

    // The four blobs protected from GC by pkg0.
    let pkg0_protected = pkg0.list_blobs();
    assert_eq!(pkg0_protected.len(), 4);
    assert!(env.blobfs.list_blobs().unwrap().is_disjoint(&pkg0_protected));

    // While dir0 lives, pkg0 protects its blobs.
    let dir0 = crate::get_and_verify_package(
        &env.proxies.package_cache,
        fpkg::GcProtection::OpenPackageTracking,
        &pkg0,
    )
    .await;
    assert_matches!(env.proxies.space_manager.gc().await, Ok(Ok(())));
    assert!(env.blobfs.list_blobs().unwrap().is_superset(&pkg0_protected));

    // Sometime after the connection to dir0 closes, open package tracking stops protecting pkg0's
    // blobs. This occurs asynchronously, when pkg-cache's VFS task serving the package directory
    // notices that the client end of the channel was closed and then finishes, which drops the
    // Arc<RootDir>.
    drop(dir0);
    let () = env.wait_for_package_to_close(pkg0.hash()).await;
    assert_matches!(env.proxies.space_manager.gc().await, Ok(Ok(())));
    assert!(env.blobfs.list_blobs().unwrap().is_disjoint(&pkg0_protected));
}

#[fuchsia::test]
async fn writing_index_clears_on_get_error() {
    let env = TestEnv::builder().build().await;
    let pkg = PackageBuilder::new("ephemeral")
        .add_resource_at("content-blob", &b"some-content"[..])
        .build()
        .await
        .unwrap();
    let (needed_blobs, needed_blobs_server_end) =
        fidl::endpoints::create_proxy::<NeededBlobsMarker>();
    let get_fut = env
        .proxies
        .package_cache
        .get(
            &fpkg::BlobInfo { blob_id: BlobId::from(*pkg.hash()).into(), length: 0 },
            fpkg::GcProtection::OpenPackageTracking,
            needed_blobs_server_end,
            fidl::endpoints::create_endpoints().1,
        )
        .map_ok(|res| res.map_err(Status::err_from_raw));
    let (meta_far, content_blobs) = pkg.contents();
    let meta_blob = needed_blobs.open_meta_blob().await.unwrap().unwrap().unwrap().into_proxy();
    let () = compress_and_write_blob(&meta_far.contents, meta_blob).await.unwrap();
    let () = blob_written(&needed_blobs, meta_far.merkle).await;
    assert_matches!(env.proxies.space_manager.gc().await, Ok(Ok(())));
    // The meta.far is in blobfs after GC.
    assert!(env.blobfs.list_blobs().unwrap().contains(pkg.hash()));
    assert_eq!(
        get_missing_blobs(&needed_blobs).await,
        Vec::from_iter(
            content_blobs
                .into_keys()
                .map(|h| fpkg::BlobInfo { blob_id: BlobId::from(h).into(), length: 0 })
        )
    );

    // It is a protocol violation to call GetMissingBlobs a second time. Doing so will fail the Get
    // with an error, but the package should still be removed from the writing index.
    let (_, blob_iterator_server_end) =
        fidl::endpoints::create_proxy::<fpkg::BlobInfoIteratorMarker>();
    let () = needed_blobs.get_missing_blobs(blob_iterator_server_end).unwrap();
    assert_matches!(get_fut.await.unwrap(), Err(Status::UNAVAILABLE));

    // PackageCache.Get has responded, so the index should be empty and GC should now remove the
    // meta.far.
    assert_matches!(env.proxies.space_manager.gc().await, Ok(Ok(())));
    assert!(!env.blobfs.list_blobs().unwrap().contains(pkg.hash()));
}

#[fuchsia::test]
async fn upgradable_packages_protected_from_gc() {
    let env = TestEnv::builder().enable_upgradable_packages().build().await;
    let () = env.block_until_started().await;

    let subpkg0 = PackageBuilder::new("upgradable-subpackage0")
        .add_resource_at("content-blob", "upgradable-subpackage0-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let pkg0 = PackageBuilder::new("upgradable-package")
        .add_subpackage("subpackage", &subpkg0)
        .add_resource_at("content-blob", "v0-contents".as_bytes())
        .build()
        .await
        .unwrap();

    // The four blobs protected from GC by pkg0.
    let pkg0_protected = pkg0.list_blobs();
    assert_eq!(pkg0_protected.len(), 4);
    assert!(env.blobfs.list_blobs().unwrap().is_disjoint(&pkg0_protected));

    // While dir0 lives, pkg0 protects its blobs.
    let dir0 = crate::get_and_verify_package(
        &env.proxies.package_cache,
        fpkg::GcProtection::OpenPackageTracking,
        &pkg0,
    )
    .await;
    let () = env.set_upgradable_urls([pkg0.pinned_fuchsia_url()]).await.unwrap();
    // Sometime after the connection to dir0 closes, open package tracking stops protecting pkg0's
    // blobs. This occurs asynchronously, when pkg-cache's VFS task serving the package directory
    // notices that the client end of the channel was closed and then finishes, which drops the
    // Arc<RootDir>.
    drop(dir0);
    let () = env.wait_for_package_to_close(pkg0.hash()).await;
    assert!(env.blobfs.list_blobs().unwrap().is_superset(&pkg0_protected));
    assert_matches!(env.proxies.space_manager.gc().await, Ok(Ok(())));
    assert!(env.blobfs.list_blobs().unwrap().is_superset(&pkg0_protected));

    let subpkg1 = PackageBuilder::new("upgradable-subpackage1")
        .add_resource_at("content-blob", "upgradable-subpackage1-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let pkg1 = PackageBuilder::new("upgradable-package")
        .add_subpackage("subpackage", &subpkg1)
        .add_resource_at("content-blob", "v1-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let pkg1_protected = pkg1.list_blobs();

    // pkg0 should not be protected after upgrading to pkg1
    let dir1 = crate::get_and_verify_package(
        &env.proxies.package_cache,
        fpkg::GcProtection::OpenPackageTracking,
        &pkg1,
    )
    .await;
    let () = env.set_upgradable_urls([pkg1.pinned_fuchsia_url()]).await.unwrap();
    drop(dir1);
    let () = env.wait_for_package_to_close(pkg1.hash()).await;
    assert_matches!(env.proxies.space_manager.gc().await, Ok(Ok(())));
    assert!(env.blobfs.list_blobs().unwrap().is_superset(&pkg1_protected));
    assert!(env.blobfs.list_blobs().unwrap().is_disjoint(&pkg0_protected));
}
