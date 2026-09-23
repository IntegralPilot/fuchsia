// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use fidl_fuchsia_pkg::ResolveError;
use fidl_fuchsia_update_installer_ext::{PrepareFailureReason, State};
use maplit::btreemap;
use pretty_assertions::assert_eq;

#[fuchsia::test]
async fn rejects_invalid_package_name() {
    let env = TestEnv::builder().build().await;

    // Name the update package something other than "update" and assert that the process fails to
    // validate the update package.
    env.resolver
        .register_custom_package("not_update", "not_update", "upd4t3", "fuchsia.com")
        .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
        .add_file("images.json", make_images_json_zbi())
        .add_file("version", "build version");

    let not_update_package_url = "fuchsia-pkg://fuchsia.com/not_update";

    let result = env.run_update_with_options(not_update_package_url, default_options()).await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    // Expect to have failed prior to downloading images.
    // The overall result should be similar to an invalid board, and we should have used
    // the not_update package URL, not `fuchsia.com/update`.
    env.assert_interactions(
        crate::initial_interactions().chain([PackageResolve(not_update_package_url.to_string())]),
    );

    assert_eq!(
        env.get_ota_metrics().await,
        OtaMetrics {
            initiator:
                metrics::OtaResultAttemptsMigratedMetricDimensionInitiator::UserInitiatedCheck
                    as u32,
            phase: metrics::OtaResultAttemptsMigratedMetricDimensionPhase::Tufupdate as u32,
            status_code: metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::Error as u32,
        }
    );
}

#[fuchsia::test]
async fn fails_if_package_unavailable() {
    let env = TestEnv::builder().build().await;

    env.resolver.mock_resolve_failure(UPDATE_PKG_URL, ResolveError::PackageNotFound);

    let result = env.run_update().await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    env.assert_interactions(
        crate::initial_interactions().chain([PackageResolve(UPDATE_PKG_URL.to_string())]),
    );
}

#[fuchsia::test]
async fn fails_if_manifest_unavailable_packageless() {
    let env = TestEnv::builder().build().await;

    let result =
        env.run_update_with_options("http://fuchsia.com/unavailable", default_options()).await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    env.assert_interactions(initial_interactions());
}

#[fuchsia::test]
async fn uses_custom_update_package() {
    let env = TestEnv::builder().build().await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", make_images_json_zbi());

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn fails_on_malformed_images_manifest_update_package() {
    let env_with_bad_images_json = TestEnv::builder().build().await;

    // if images.json is malformed, do not proceed down the path of using it!
    env_with_bad_images_json
        .resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("zbi", "fake zbi")
        .add_file("images.json", "fake manifest");

    assert!(
        env_with_bad_images_json
            .run_update_with_options(
                "fuchsia-pkg://fuchsia.com/another-update/4",
                default_options()
            )
            .await
            .is_err(),
        "system updater succeeded when it should fail"
    );

    let env_no_images_json = TestEnv::builder().build().await;

    env_no_images_json
        .resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("zbi", "fake zbi");

    assert!(
        env_no_images_json
            .run_update_with_options(
                "fuchsia-pkg://fuchsia.com/another-update/4",
                default_options()
            )
            .await
            .is_err(),
        "system updater succeeded when it should fail"
    );

    env_with_bad_images_json.assert_interactions(
        crate::initial_interactions()
            .chain([PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".into())]),
    );
    env_no_images_json.assert_interactions(
        crate::initial_interactions()
            .chain([PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".into())]),
    );
}

#[fuchsia::test]
async fn fails_on_malformed_ota_manifest_packageless() {
    let env_with_bad_manifest_json =
        TestEnv::builder().ota_manifest_raw("fake manifest").build().await;

    assert!(
        env_with_bad_manifest_json.run_packageless_update().await.is_err(),
        "system updater succeeded when it should fail"
    );

    let env_no_manifest_json = TestEnv::builder().ota_manifest_raw("").build().await;

    assert!(
        env_no_manifest_json.run_packageless_update().await.is_err(),
        "system updater succeeded when it should fail"
    );

    env_with_bad_manifest_json.assert_interactions(initial_interactions());
    env_no_manifest_json.assert_interactions(initial_interactions());
}

#[fuchsia::test]
async fn retry_update_package_resolve_once() {
    let env = TestEnv::builder().build().await;

    env.resolver.url(UPDATE_PKG_URL).respond_serially(vec![
        // First resolve should fail with NoSpace.
        Err(ResolveError::NoSpace),
        // Second resolve should succeed.
        Ok(env
            .resolver
            .package("update", "upd4t3")
            .add_file("packages.json", make_packages_json([]))
            .add_file("epoch.json", make_current_epoch_json())
            .add_file("images.json", make_images_json_zbi())),
    ]);

    env.run_update().await.expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        // First resolve should fail with NoSpace, so we GC and try the resolve again.
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Gc,
        // Second resolve should succeed!
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn retry_update_package_resolve_twice() {
    let env = TestEnv::builder().build().await;

    env.resolver.url(UPDATE_PKG_URL).respond_serially(vec![
        // First two resolves should fail with NoSpace.
        Err(ResolveError::NoSpace),
        Err(ResolveError::NoSpace),
        // Third resolve should succeed.
        Ok(env
            .resolver
            .package("update", "upd4t3")
            .add_file("packages.json", make_packages_json([]))
            .add_file("epoch.json", make_current_epoch_json())
            .add_file("images.json", make_images_json_zbi())),
    ]);

    env.run_update().await.expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        // First resolve should fail with NoSpace, so we GC and try the resolve again.
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Gc,
        // Second resolve should fail with NoSpace, so we clear the retained packages set then
        // GC and try the resolve again.
        PackageResolve(UPDATE_PKG_URL.to_string()),
        ClearRetainedPackages,
        Gc,
        // Third resolve should succeed!
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn retry_update_package_resolve_thrice_fails_update_attempt() {
    let env = TestEnv::builder().build().await;

    env.resolver.url(UPDATE_PKG_URL).respond_serially(vec![
        // Each resolve should fail with NoSpace.
        Err(ResolveError::NoSpace),
        Err(ResolveError::NoSpace),
        Err(ResolveError::NoSpace),
    ]);

    let mut attempt = env.start_update().await.unwrap();

    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::FailPrepare(PrepareFailureReason::OutOfSpace)
    );

    env.assert_interactions(crate::initial_interactions().chain([
        // First resolve should fail with out of space, so we GC and try the resolve again.
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Gc,
        // Second resolve should fail with out of space, so we clear retained packages set then
        // GC and try the resolve again.
        PackageResolve(UPDATE_PKG_URL.to_string()),
        ClearRetainedPackages,
        Gc,
        // Third resolve should fail with out of space, so the update fails.
        PackageResolve(UPDATE_PKG_URL.to_string()),
    ]));
}

#[fuchsia::test]
async fn fully_populated_images_manifest() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .recovery_package(
            ::update_package::ImageMetadata::new(
                5,
                sha256(1),
                image_package_resource_url("update-images-recovery", 3, "rzbi"),
            ),
            Some(::update_package::ImageMetadata::new(
                5,
                sha256(2),
                image_package_resource_url("update-images-recovery", 3, "rvbmeta"),
            )),
        )
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                5,
                sha256(3),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            Some(::update_package::ImageMetadata::new(
                5,
                sha256(4),
                image_package_resource_url("update-images-fuchsia", 9, "vbmeta"),
            )),
        )
        .firmware_package(btreemap! {
            "".to_owned() => ::update_package::ImageMetadata::new(
                5, sha256(5), image_package_resource_url("update-images-firmware", 5, "a")
            ),
            "bl2".to_owned() => ::update_package::ImageMetadata::new(
                5, sha256(6), image_package_resource_url("update-images-firmware", 5, "bl2")
            ),
        })
        .clone()
        .build();

    let env = TestEnv::builder().build().await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver.url(image_package_url_to_string("update-images-fuchsia", 9)).resolve(
        &env.resolver
            .package("fuchsia", hashstr(8))
            .add_file("zbi", "zbi contents")
            .add_file("vbmeta", "vbmeta contents"),
    );

    env.resolver.url(image_package_url_to_string("update-images-recovery", 3)).resolve(
        &env.resolver
            .package("recovery", hashstr(4))
            .add_file("rzbi", "rzbi contents")
            .add_file("rvbmeta", "rvbmeta contents"),
    );

    env.resolver.url(image_package_url_to_string("update-images-firmware", 5)).resolve(
        &env.resolver
            .package("firmware", hashstr(2))
            .add_file("a", "afirmware")
            .add_file("bl2", "bl2bl2"),
    );

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    let beginning_events = vec![
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
        Paver(PaverEvent::SetConfigurationUnbootable { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::VerifiedBootMetadata,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::VerifiedBootMetadata,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::Recovery,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::Recovery,
            asset: paver::Asset::VerifiedBootMetadata,
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "".to_string(),
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::A,
            firmware_type: "".to_string(),
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "bl2".to_string(),
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::A,
            firmware_type: "bl2".to_string(),
        }),
    ];

    // Resolving the images is done nondeterministically.
    // Below we assert that the events still happened.

    let end_events = [
        Paver(PaverEvent::WriteFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "".to_string(),
            payload: b"afirmware".to_vec(),
        }),
        Paver(PaverEvent::WriteFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "bl2".to_string(),
            payload: b"bl2bl2".to_vec(),
        }),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"zbi contents".to_vec(),
        }),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::VerifiedBootMetadata,
            payload: b"vbmeta contents".to_vec(),
        }),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::Recovery,
            asset: paver::Asset::Kernel,
            payload: b"rzbi contents".to_vec(),
        }),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::Recovery,
            asset: paver::Asset::VerifiedBootMetadata,
            payload: b"rvbmeta contents".to_vec(),
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ];

    let all_events = env.take_interactions();

    let all_events_start = all_events[..beginning_events.len()].to_vec();
    assert_eq!(all_events_start, beginning_events);

    let all_events_end = all_events[all_events.len() - end_events.len()..].to_vec();
    assert_eq!(all_events_end, end_events);

    // The 5 is for the nondeterministic events.
    // ReplaceRetainedPackages (of which we cannot guarantee the order of the image package hashes),
    // a Gc occurs, and the 3 PackageResolves for the image packages happen concurrently.
    assert!(all_events.len() == beginning_events.len() + end_events.len() + 5);

    let all_events_middle = all_events[beginning_events.len()..beginning_events.len() + 5].to_vec();

    // it's got to be one of these combinatorics!
    assert!(
        all_events_middle[0]
            == ReplaceRetainedPackages(vec![hash(9).into(), hash(3).into(), hash(5).into()])
            || all_events_middle[0]
                == ReplaceRetainedPackages(vec![hash(9).into(), hash(5).into(), hash(3).into()])
            || all_events_middle[0]
                == ReplaceRetainedPackages(vec![hash(5).into(), hash(3).into(), hash(9).into()])
            || all_events_middle[0]
                == ReplaceRetainedPackages(vec![hash(5).into(), hash(9).into(), hash(3).into()])
            || all_events_middle[0]
                == ReplaceRetainedPackages(vec![hash(3).into(), hash(5).into(), hash(9).into()])
            || all_events_middle[0]
                == ReplaceRetainedPackages(vec![hash(3).into(), hash(9).into(), hash(5).into()])
    );

    assert!(all_events_middle[1] == Gc);

    assert!(
        all_events_middle
            .contains(&PackageResolve(image_package_url_to_string("update-images-recovery", 3)))
    );
    assert!(
        all_events_middle
            .contains(&PackageResolve(image_package_url_to_string("update-images-firmware", 5)))
    );
    assert!(
        all_events_middle
            .contains(&PackageResolve(image_package_url_to_string("update-images-fuchsia", 9)))
    );
}

#[fuchsia::test]
async fn fully_populated_images_manifest_packageless() {
    let zbi_content = b"zbi contents";
    let zbi_hash = fuchsia_merkle::root_from_slice(zbi_content);
    let vbmeta_content = b"vbmeta contents";
    let vbmeta_hash = fuchsia_merkle::root_from_slice(vbmeta_content);
    let recovery_zbi_content = b"rzbi contents";
    let recovery_zbi_hash = fuchsia_merkle::root_from_slice(recovery_zbi_content);
    let recovery_vbmeta_content = b"rvbmeta contents";
    let recovery_vbmeta_hash = fuchsia_merkle::root_from_slice(recovery_vbmeta_content);
    let firmware_content = b"afirmware";
    let firmware_hash = fuchsia_merkle::root_from_slice(firmware_content);
    let firmware_bl2_content = b"bl2bl2";
    let firmware_bl2_hash = fuchsia_merkle::root_from_slice(firmware_bl2_content);

    let manifest = OtaManifest {
        images: vec![
            manifest::Image {
                slot: manifest::Slot::R,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob {
                    uncompressed_size: recovery_zbi_content.len() as u64,
                    fuchsia_merkle_root: recovery_zbi_hash,
                },
            },
            manifest::Image {
                slot: manifest::Slot::R,
                image_type: manifest::ImageType::Asset(AssetType::Vbmeta),
                blob: manifest::Blob {
                    uncompressed_size: recovery_vbmeta_content.len() as u64,
                    fuchsia_merkle_root: recovery_vbmeta_hash,
                },
            },
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob {
                    uncompressed_size: zbi_content.len() as u64,
                    fuchsia_merkle_root: zbi_hash,
                },
            },
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Vbmeta),
                blob: manifest::Blob {
                    uncompressed_size: vbmeta_content.len() as u64,
                    fuchsia_merkle_root: vbmeta_hash,
                },
            },
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Firmware("".to_string()),
                blob: manifest::Blob {
                    uncompressed_size: firmware_content.len() as u64,
                    fuchsia_merkle_root: firmware_hash,
                },
            },
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Firmware("bl2".to_string()),
                blob: manifest::Blob {
                    uncompressed_size: firmware_bl2_content.len() as u64,
                    fuchsia_merkle_root: firmware_bl2_hash,
                },
            },
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .ota_manifest(manifest)
        .blob(zbi_hash, zbi_content.to_vec())
        .blob(vbmeta_hash, vbmeta_content.to_vec())
        .blob(recovery_zbi_hash, recovery_zbi_content.to_vec())
        .blob(recovery_vbmeta_hash, recovery_vbmeta_content.to_vec())
        .blob(firmware_hash, firmware_content.to_vec())
        .blob(firmware_bl2_hash, firmware_bl2_content.to_vec())
        .build()
        .await;

    env.run_packageless_update().await.expect("run system updater");

    env.assert_unordered_interactions(
        initial_interactions().chain([
            ReplaceRetainedBlobs(vec![
                recovery_zbi_hash.into(),
                recovery_vbmeta_hash.into(),
                zbi_hash.into(),
                vbmeta_hash.into(),
                firmware_hash.into(),
                firmware_bl2_hash.into(),
            ]),
            Gc,
        ]),
        [
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
            }),
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::A,
                asset: paver::Asset::Kernel,
            }),
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::VerifiedBootMetadata,
            }),
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::A,
                asset: paver::Asset::VerifiedBootMetadata,
            }),
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::Recovery,
                asset: paver::Asset::Kernel,
            }),
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::Recovery,
                asset: paver::Asset::VerifiedBootMetadata,
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "".to_string(),
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::A,
                firmware_type: "".to_string(),
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "bl2".to_string(),
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::A,
                firmware_type: "bl2".to_string(),
            }),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(recovery_zbi_hash.into())),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(recovery_vbmeta_hash.into())),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(zbi_hash.into())),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(vbmeta_hash.into())),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(firmware_hash.into())),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(firmware_bl2_hash.into())),
            Paver(PaverEvent::WriteFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "".to_string(),
                payload: b"afirmware".to_vec(),
            }),
            Paver(PaverEvent::WriteFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "bl2".to_string(),
                payload: b"bl2bl2".to_vec(),
            }),
            Paver(PaverEvent::WriteAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
                payload: b"zbi contents".to_vec(),
            }),
            Paver(PaverEvent::WriteAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::VerifiedBootMetadata,
                payload: b"vbmeta contents".to_vec(),
            }),
            Paver(PaverEvent::WriteAsset {
                configuration: paver::Configuration::Recovery,
                asset: paver::Asset::Kernel,
                payload: b"rzbi contents".to_vec(),
            }),
            Paver(PaverEvent::WriteAsset {
                configuration: paver::Configuration::Recovery,
                asset: paver::Asset::VerifiedBootMetadata,
                payload: b"rvbmeta contents".to_vec(),
            }),
        ],
        [
            Paver(PaverEvent::DataSinkFlush),
            ReplaceRetainedBlobs(vec![]),
            Gc,
            BlobfsSync,
            Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
            Paver(PaverEvent::BootManagerFlush),
            Reboot,
        ],
    );
}
