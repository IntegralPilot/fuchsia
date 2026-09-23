// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use fidl_fuchsia_pkg::ResolveError;
use pretty_assertions::assert_eq;

#[fuchsia::test]
async fn succeeds_even_if_retained_packages_fails() {
    let env =
        TestEnv::builder().unregister_protocol(crate::Protocol::RetainedPackages).build().await;

    env.resolver.url(UPDATE_PKG_URL_PINNED).respond_serially(vec![
        // To trigger first RetainedPackages use, first two resolves of update package must fail.
        Err(ResolveError::NoSpace),
        Err(ResolveError::NoSpace),
        Ok(env
            .resolver
            .package("update", UPDATE_HASH)
            .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
            .add_file("epoch.json", make_current_epoch_json())
            .add_file("images.json", make_images_json_zbi())),
    ]);

    env.resolver
        .url(SYSTEM_IMAGE_URL)
        .resolve(&env.resolver.package("system_image/0", SYSTEM_IMAGE_HASH));

    let () = env
        .run_update_with_options(UPDATE_PKG_URL_PINNED, default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve(UPDATE_PKG_URL_PINNED.to_string()),
        Gc,
        PackageResolve(UPDATE_PKG_URL_PINNED.to_string()),
        // No RetainedPackages use here b/c protocol is blocked
        Gc,
        PackageResolve(UPDATE_PKG_URL_PINNED.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        // No RetainedPackages use here b/c protocol is blocked
        Gc,
        PackageResolve(SYSTEM_IMAGE_URL.to_string()),
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn succeeds_even_if_retained_blobs_fails_packageless() {
    let content_blob = vec![1; 200];
    let content_blob_hash = fuchsia_merkle::root_from_slice(&content_blob);

    let manifest = make_manifest([manifest::Blob {
        uncompressed_size: content_blob.len() as u64,
        fuchsia_merkle_root: content_blob_hash,
    }]);
    let env = TestEnv::builder()
        .unregister_protocol(crate::Protocol::RetainedBlobs)
        .ota_manifest(manifest)
        .blob(content_blob_hash, content_blob)
        .build()
        .await;

    let () = env.run_packageless_update().await.expect("run system updater");

    env.assert_interactions(initial_interactions().chain([
        // No RetainedBlobs use here b/c protocol is blocked
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        // No RetainedBlobs use here b/c protocol is blocked
        Gc,
        OtaDownloader(OtaDownloaderEvent::FetchBlob(content_blob_hash.into())),
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

// Verifies that:
//   1. pinned update pkg url causes both RetainedPackages uses to include the update package
//   2. second RetainedPackages use includes packages from packages.json
#[fuchsia::test]
async fn pinned_url_and_non_empty_packages_json() {
    let env = TestEnv::builder().build().await;

    env.resolver.url(UPDATE_PKG_URL_PINNED).respond_serially(vec![
        // To trigger first RetainedPackages use, first two resolves of update package must fail.
        Err(ResolveError::NoSpace),
        Err(ResolveError::NoSpace),
        Ok(env
            .resolver
            .package("update", UPDATE_HASH)
            .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
            .add_file("epoch.json", make_current_epoch_json())
            .add_file("images.json", make_images_json_zbi())),
    ]);

    env.resolver
        .url(SYSTEM_IMAGE_URL)
        .resolve(&env.resolver.package("system_image/0", SYSTEM_IMAGE_HASH));

    let () = env
        .run_update_with_options(UPDATE_PKG_URL_PINNED, default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve(UPDATE_PKG_URL_PINNED.to_string()),
        Gc,
        PackageResolve(UPDATE_PKG_URL_PINNED.to_string()),
        ReplaceRetainedPackages(vec![UPDATE_HASH.parse().unwrap()]),
        Gc,
        PackageResolve(UPDATE_PKG_URL_PINNED.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![
            SYSTEM_IMAGE_HASH.parse().unwrap(),
            UPDATE_HASH.parse().unwrap(),
        ]),
        Gc,
        PackageResolve(SYSTEM_IMAGE_URL.to_string()),
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

// Verifies that an unpinned update pkg URL causes:
//   1. The first RetainedPackages use to be Clear (instead of setting the index to the hash of the
//      update package, since the hash is missing from the URL and therefore unknown).
//   2. The second RetainedPackages use to be Replace with the system image hash (the only entry of
//      packages.json) and the update package hash. The update package hash was obtained from the
//      resolved update package (since it was missing from the URL).
//
// There are no images.json-related manipulations of the retained index because the only
// images.json entry is a ZBI with the empty hash, and the paver will report that the currently
// written image in the available slot has the empty hash, and system-updater does not resolve
// image packages if the desired image is already present.
#[fuchsia::test]
async fn unpinned_url_and_non_empty_packages_json() {
    let env = TestEnv::builder().build().await;

    env.resolver.url(UPDATE_PKG_URL).respond_serially(vec![
        // To trigger first RetainedPackages use, first two resolves of update package must fail.
        Err(ResolveError::NoSpace),
        Err(ResolveError::NoSpace),
        Ok(env
            .resolver
            .package("update", UPDATE_HASH)
            .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
            .add_file("epoch.json", make_current_epoch_json())
            .add_file("images.json", make_images_json_zbi())),
    ]);

    env.resolver
        .url(SYSTEM_IMAGE_URL)
        .resolve(&env.resolver.package("system_image/0", SYSTEM_IMAGE_HASH));

    let () = env.run_update().await.expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Gc,
        PackageResolve(UPDATE_PKG_URL.to_string()),
        ClearRetainedPackages,
        Gc,
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![
            SYSTEM_IMAGE_HASH.parse().unwrap(),
            UPDATE_HASH.parse().unwrap(),
        ]),
        Gc,
        PackageResolve(SYSTEM_IMAGE_URL.to_string()),
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

// Verifies that an unpinned update pkg URL causes:
//   1. The first RetainedPackages use to be Clear (instead of setting the index to the hash of the
//      update package, since the hash is missing from the URL and therefore unknown).
//   2. The second RetainedPackages use to be Replace with the system image hash (the only entry of
//      packages.json), the ZBI hash, and the update package hash. The update package hash was
//      obtained from the resolved update package (since it was missing from the URL).
//   3. The third RetainedPackages use to be Replace with the ZBI hash and the update package hash.
//      This is because the first resolve of the ZBI failed and now we are retrying after having
//      possibly freed some space by GC'ing the packages.json packages.
//   4. The fourth RetainedPackages use to be Replace with the system image and update package.
//      This is to GC the images in preparation for resolving the packages.json entries.
//
// This is a "maximal" test of RetainedPackages interactions. It triggers each possible call to the
// protocol and each call contains the most possible information:
//   1. the update package hash is missing from the URL but obtained from the resolved package
//   2. packages.json has an entry that needs to be added and removed from the index
//   3. images.json has an entry that needs to be added and removed from the index
#[fuchsia::test]
async fn unpinned_url_and_resolved_image_package_and_non_empty_packages_json() {
    let env = TestEnv::builder().build().await;

    let zbi_hash_seed = 9;
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                5,
                sha256(zbi_hash_seed),
                image_package_resource_url("zbi", zbi_hash_seed, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    env.resolver.url(UPDATE_PKG_URL).respond_serially(vec![
        // To trigger RetainedPackages interaction, the first two resolves of the update package
        // must fail.
        Err(ResolveError::NoSpace),
        Err(ResolveError::NoSpace),
        Ok(env
            .resolver
            .package("update", UPDATE_HASH)
            .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
            .add_file("epoch.json", make_current_epoch_json())
            .add_file("images.json", serde_json::to_string(&images_json).unwrap())),
    ]);

    env.resolver.url(image_package_url_to_string("zbi", zbi_hash_seed)).respond_serially(vec![
        // To trigger an additional RetainedPackages interaction, the first resolve of the ZBI
        // must fail.
        Err(ResolveError::NoSpace),
        Ok(env.resolver.package("zbi", hashstr(zbi_hash_seed)).add_file("zbi", "zbi contents")),
    ]);

    env.resolver
        .url(SYSTEM_IMAGE_URL)
        .resolve(&env.resolver.package("system_image/0", SYSTEM_IMAGE_HASH));

    let () = env.run_update().await.expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Gc,
        PackageResolve(UPDATE_PKG_URL.to_string()),
        ClearRetainedPackages,
        Gc,
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        ReplaceRetainedPackages(vec![
            SYSTEM_IMAGE_HASH.parse().unwrap(),
            hash(zbi_hash_seed).into(),
            UPDATE_HASH.parse().unwrap(),
        ]),
        Gc,
        PackageResolve(
            image_package_resource_url("zbi", zbi_hash_seed, "zbi").package_url().to_string(),
        ),
        ReplaceRetainedPackages(vec![hash(zbi_hash_seed).into(), UPDATE_HASH.parse().unwrap()]),
        Gc,
        PackageResolve(
            image_package_resource_url("zbi", zbi_hash_seed, "zbi").package_url().to_string(),
        ),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"zbi contents".to_vec(),
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![
            SYSTEM_IMAGE_HASH.parse().unwrap(),
            UPDATE_HASH.parse().unwrap(),
        ]),
        Gc,
        PackageResolve(SYSTEM_IMAGE_URL.to_string()),
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

// Verifies that:
//   1. pinned update pkg url causes both RetainedPackages uses to include the update package
//   2. second RetainedPackages only has the update package
#[fuchsia::test]
async fn pinned_url_and_empty_packages_json() {
    let env = TestEnv::builder().build().await;

    env.resolver.url(UPDATE_PKG_URL_PINNED).respond_serially(vec![
        // To trigger first RetainedPackages use, first two resolves of update package must fail.
        Err(ResolveError::NoSpace),
        Err(ResolveError::NoSpace),
        Ok(env
            .resolver
            .package("update", UPDATE_HASH)
            .add_file("packages.json", make_packages_json([]))
            .add_file("epoch.json", make_current_epoch_json())
            .add_file("images.json", make_images_json_zbi())),
    ]);

    env.resolver
        .url(SYSTEM_IMAGE_URL)
        .resolve(&env.resolver.package("system_image/0", SYSTEM_IMAGE_HASH));

    let () = env
        .run_update_with_options(UPDATE_PKG_URL_PINNED, default_options())
        .await
        .expect("run system updater");

    assert_eq!(
        env.take_interactions(),
        vec![
            Paver(PaverEvent::QueryCurrentConfiguration),
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::A,
                asset: paver::Asset::VerifiedBootMetadata,
            }),
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::A,
                asset: paver::Asset::Kernel,
            }),
            Paver(PaverEvent::QueryCurrentConfiguration),
            Paver(PaverEvent::QueryConfigurationStatus { configuration: paver::Configuration::A }),
            Paver(PaverEvent::SetConfigurationUnbootable {
                configuration: paver::Configuration::B
            }),
            Paver(PaverEvent::BootManagerFlush),
            PackageResolve(UPDATE_PKG_URL_PINNED.to_string()),
            Gc,
            PackageResolve(UPDATE_PKG_URL_PINNED.to_string()),
            ReplaceRetainedPackages(vec![UPDATE_HASH.parse().unwrap()]),
            Gc,
            PackageResolve(UPDATE_PKG_URL_PINNED.to_string()),
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
            }),
            Paver(PaverEvent::DataSinkFlush),
            ReplaceRetainedPackages(vec![UPDATE_HASH.parse().unwrap(),]),
            Gc,
            BlobfsSync,
            Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
            Paver(PaverEvent::BootManagerFlush),
            Reboot,
        ]
    );
}

// Verifies that an unpinned update pkg URL causes:
//   1. The first RetainedPackages use to be Clear (instead of setting the index to the hash of the
//      update package, since the hash is missing from the URL and therefore unknown).
//   2. The second RetainedPackages use to be Replace with just the update package hash. The hash
//      was obtained from the resolved update package (since it was missing from the URL).
//      Only the update package hash will be included because packages.json is empty.
//
// There are no images.json-related manipulations of the retained index because the only
// images.json entry is a ZBI with the empty hash, and the paver will report that the currently
// written image in the available slot has the empty hash, and system-updater does not resolve
// image packages if the desired image is already present.
#[fuchsia::test]
async fn unpinned_url_and_empty_packages_json() {
    let env = TestEnv::builder().build().await;

    env.resolver.url(UPDATE_PKG_URL).respond_serially(vec![
        // To trigger first RetainedPackages use, first two resolves of update package must fail.
        Err(ResolveError::NoSpace),
        Err(ResolveError::NoSpace),
        Ok(env
            .resolver
            .package("update", UPDATE_HASH)
            .add_file("packages.json", make_packages_json([]))
            .add_file("epoch.json", make_current_epoch_json())
            .add_file("images.json", make_images_json_zbi())),
    ]);

    let () = env.run_update().await.expect("run system updater");

    assert_eq!(
        env.take_interactions(),
        vec![
            Paver(PaverEvent::QueryCurrentConfiguration),
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::A,
                asset: paver::Asset::VerifiedBootMetadata,
            }),
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::A,
                asset: paver::Asset::Kernel,
            }),
            Paver(PaverEvent::QueryCurrentConfiguration),
            Paver(PaverEvent::QueryConfigurationStatus { configuration: paver::Configuration::A }),
            Paver(PaverEvent::SetConfigurationUnbootable {
                configuration: paver::Configuration::B
            }),
            Paver(PaverEvent::BootManagerFlush),
            PackageResolve(UPDATE_PKG_URL.to_string()),
            Gc,
            PackageResolve(UPDATE_PKG_URL.to_string()),
            ClearRetainedPackages,
            Gc,
            PackageResolve(UPDATE_PKG_URL.to_string()),
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
            }),
            Paver(PaverEvent::DataSinkFlush),
            ReplaceRetainedPackages(vec![UPDATE_HASH.parse().unwrap()]),
            Gc,
            BlobfsSync,
            Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
            Paver(PaverEvent::BootManagerFlush),
            Reboot,
        ]
    );
}

#[fuchsia::test]
async fn non_empty_blobs_packageless() {
    let content_blob = vec![1; 200];
    let content_blob_hash = fuchsia_merkle::root_from_slice(&content_blob);

    let manifest = make_manifest([manifest::Blob {
        uncompressed_size: content_blob.len() as u64,
        fuchsia_merkle_root: content_blob_hash,
    }]);
    let env = TestEnv::builder()
        .ota_manifest(manifest)
        .blob(content_blob_hash, content_blob)
        .build()
        .await;

    let () = env.run_packageless_update().await.expect("run system updater");

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![empty_merkle().into(), content_blob_hash.into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedBlobs(vec![content_blob_hash.into()]),
        Gc,
        OtaDownloader(OtaDownloaderEvent::FetchBlob(content_blob_hash.into())),
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn empty_blobs_packageless() {
    let env = TestEnv::builder().ota_manifest(make_manifest([])).build().await;

    let () = env.run_packageless_update().await.expect("run system updater");

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![empty_merkle().into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedBlobs(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}
