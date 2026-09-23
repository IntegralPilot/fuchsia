// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use fidl_fuchsia_pkg::ResolveError;
use fidl_fuchsia_update_installer_ext::{
    Progress, StageFailureReason, State, StateId, UpdateInfo, UpdateInfoAndProgress,
};
use pretty_assertions::assert_eq;
use test_case::test_case;

#[test_case(UPDATE_PKG_URL)]
#[test_case(MANIFEST_URL)]
#[fuchsia::test]
async fn fails_on_paver_connect_error(update_url: &str) {
    let env = TestEnv::builder()
        .unregister_protocol(Protocol::Paver)
        .ota_manifest(make_manifest([]))
        .build()
        .await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("images.json", make_images_json_zbi());

    let result = env.run_update_with_options(update_url, default_options()).await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    // Appmgr will close the paver service channel when it is unable to forward the channel to any
    // implementation of that protocol, but it is a race condition as to whether or not the system
    // updater will be able to send the requests to open the data sink and boot manager connections
    // before that happens. So, the update attempt will either fail very early or when it attempts
    // to query the current configuration.
    let interactions = env.take_interactions();
    assert!(
        interactions.is_empty()
            || interactions == [Gc, PackageResolve(UPDATE_PKG_URL.to_string()), Gc, BlobfsSync,],
        "expected early failure or failure while querying current configuration. Got {interactions:#?}"
    );
}

#[fuchsia::test]
async fn fails_on_image_write_error() {
    let images_json = serde_json::to_string(
        &::update_package::ImagePackagesManifest::builder()
            .fuchsia_package(
                ::update_package::ImageMetadata::new(
                    5,
                    sha256(8),
                    image_package_resource_url("update-images-fuchsia", 9, "zbi"),
                ),
                None,
            )
            .clone()
            .build(),
    )
    .unwrap();
    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::return_error(|event| match event {
                PaverEvent::WriteAsset { .. } => Err(Status::INTERNAL),
                _ => Ok(()),
            }))
        })
        .build()
        .await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", images_json);

    env.resolver
        .url(image_package_url_to_string("update-images-fuchsia", 9))
        .resolve(&env.resolver.package("fuchsia", hashstr(8)).add_file("zbi", "zbi zbi"));

    let result = env.run_update().await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    assert_eq!(
        env.get_ota_metrics().await,
        OtaMetrics {
            initiator:
                metrics::OtaResultAttemptsMigratedMetricDimensionInitiator::UserInitiatedCheck
                    as u32,
            phase: metrics::OtaResultAttemptsMigratedMetricDimensionPhase::ImageWrite as u32,
            status_code: metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::Error as u32,
        }
    );

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        ReplaceRetainedPackages(vec![hash(9).into()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-fuchsia", 9)),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"zbi zbi".to_vec(),
        }),
    ]));
}

#[fuchsia::test]
async fn fails_on_image_write_error_packageless() {
    let zbi_content = b"zbi zbi";
    let zbi_hash = fuchsia_merkle::root_from_slice(zbi_content);
    let manifest = OtaManifest {
        images: vec![manifest::Image {
            slot: manifest::Slot::AB,
            image_type: manifest::ImageType::Asset(AssetType::Zbi),
            blob: manifest::Blob {
                uncompressed_size: zbi_content.len() as u64,
                fuchsia_merkle_root: zbi_hash,
            },
        }],
        ..make_manifest([])
    };
    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::return_error(|event| match event {
                PaverEvent::WriteAsset { .. } => Err(Status::INTERNAL),
                _ => Ok(()),
            }))
        })
        .ota_manifest(manifest)
        .blob(zbi_hash, zbi_content.to_vec())
        .build()
        .await;

    let result = env.run_packageless_update().await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    assert_eq!(
        env.get_ota_metrics().await,
        OtaMetrics {
            initiator:
                metrics::OtaResultAttemptsMigratedMetricDimensionInitiator::UserInitiatedCheck
                    as u32,
            phase: metrics::OtaResultAttemptsMigratedMetricDimensionPhase::ImageWrite as u32,
            status_code: metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::Error as u32,
        }
    );

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![zbi_hash.into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        OtaDownloader(OtaDownloaderEvent::FetchBlob(zbi_hash.into())),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: zbi_content.to_vec(),
        }),
    ]));
}

#[fuchsia::test]
async fn writes_to_both_configs_if_abr_not_supported() {
    let images_json = serde_json::to_string(
        &::update_package::ImagePackagesManifest::builder()
            .fuchsia_package(
                ::update_package::ImageMetadata::new(
                    5,
                    sha256(8),
                    image_package_resource_url("update-images-fuchsia", 9, "zbi"),
                ),
                None,
            )
            .clone()
            .build(),
    )
    .unwrap();

    let env = TestEnv::builder()
        .paver_service(|builder| builder.boot_manager_close_with_epitaph(Status::NOT_SUPPORTED))
        .build()
        .await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", images_json);

    env.resolver
        .url(image_package_url_to_string("update-images-fuchsia", 9))
        .resolve(&env.resolver.package("fuchsia", hashstr(8)).add_file("zbi", "zbi zbi"));

    env.run_update().await.expect("success");

    assert_eq!(
        env.get_ota_metrics().await,
        OtaMetrics {
            initiator:
                metrics::OtaResultAttemptsMigratedMetricDimensionInitiator::UserInitiatedCheck
                    as u32,
            phase: metrics::OtaResultAttemptsMigratedMetricDimensionPhase::SuccessPendingReboot
                as u32,
            status_code: metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::Success
                as u32,
        }
    );

    env.assert_interactions([
        PackageResolve(UPDATE_PKG_URL.to_string()),
        ReplaceRetainedPackages(vec![hash(9).into()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-fuchsia", 9)),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
            payload: b"zbi zbi".to_vec(),
        }),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"zbi zbi".to_vec(),
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![]),
        Gc,
        BlobfsSync,
        Reboot,
    ]);
}

#[fuchsia::test]
async fn writes_to_both_configs_if_abr_not_supported_packageless() {
    let zbi_content = b"zbi zbi";
    let zbi_hash = fuchsia_merkle::root_from_slice(zbi_content);
    let manifest = OtaManifest {
        images: vec![manifest::Image {
            slot: manifest::Slot::AB,
            image_type: manifest::ImageType::Asset(AssetType::Zbi),
            blob: manifest::Blob {
                uncompressed_size: zbi_content.len() as u64,
                fuchsia_merkle_root: zbi_hash,
            },
        }],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| builder.boot_manager_close_with_epitaph(Status::NOT_SUPPORTED))
        .ota_manifest(manifest)
        .blob(zbi_hash, zbi_content.to_vec())
        .build()
        .await;

    env.run_packageless_update().await.expect("success");

    assert_eq!(
        env.get_ota_metrics().await,
        OtaMetrics {
            initiator:
                metrics::OtaResultAttemptsMigratedMetricDimensionInitiator::UserInitiatedCheck
                    as u32,
            phase: metrics::OtaResultAttemptsMigratedMetricDimensionPhase::SuccessPendingReboot
                as u32,
            status_code: metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::Success
                as u32,
        }
    );

    env.assert_interactions([
        ReplaceRetainedBlobs(vec![zbi_hash.into()]),
        Gc,
        OtaDownloader(OtaDownloaderEvent::FetchBlob(zbi_hash.into())),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
            payload: zbi_content.to_vec(),
        }),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: zbi_content.to_vec(),
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedBlobs(vec![]),
        Gc,
        BlobfsSync,
        Reboot,
    ]);
}

// If the current partition isn't healthy, the system-updater aborts.
#[test_case(UPDATE_PKG_URL)]
#[test_case(MANIFEST_URL)]
#[fuchsia::test]
async fn does_not_update_with_unhealthy_current_partition(update_url: &str) {
    let current_config = paver::Configuration::A;

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder
                .insert_hook(mphooks::config_status(|_| Ok(paver::ConfigurationStatus::Pending)))
                .current_config(current_config)
        })
        .ota_manifest(make_manifest([]))
        .build()
        .await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("images.json", make_images_json_zbi());

    let result = env.run_update_with_options(update_url, default_options()).await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

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

    env.assert_interactions([
        Paver(PaverEvent::QueryCurrentConfiguration),
        Paver(PaverEvent::ReadAsset {
            configuration: current_config,
            asset: paver::Asset::VerifiedBootMetadata,
        }),
        Paver(PaverEvent::ReadAsset { configuration: current_config, asset: paver::Asset::Kernel }),
        Paver(PaverEvent::QueryCurrentConfiguration),
        Paver(PaverEvent::QueryConfigurationStatus { configuration: current_config }),
    ]);
}

// If the alternate configuration can't be marked unbootable, the system-updater fails.
#[test_case(UPDATE_PKG_URL)]
#[test_case(MANIFEST_URL)]
#[fuchsia::test]
async fn does_not_update_if_alternate_cant_be_marked_unbootable(update_url: &str) {
    let current_config = paver::Configuration::A;

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder
                .insert_hook(mphooks::return_error(|event| match event {
                    PaverEvent::SetConfigurationUnbootable { .. } => Err(Status::INTERNAL),
                    _ => Ok(()),
                }))
                .current_config(current_config)
        })
        .ota_manifest(make_manifest([]))
        .build()
        .await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("images.json", make_images_json_zbi());

    let result = env.run_update_with_options(update_url, default_options()).await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

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

    env.assert_interactions([
        Paver(PaverEvent::QueryCurrentConfiguration),
        Paver(PaverEvent::ReadAsset {
            configuration: current_config,
            asset: paver::Asset::VerifiedBootMetadata,
        }),
        Paver(PaverEvent::ReadAsset { configuration: current_config, asset: paver::Asset::Kernel }),
        Paver(PaverEvent::QueryCurrentConfiguration),
        Paver(PaverEvent::QueryConfigurationStatus { configuration: current_config }),
        Paver(PaverEvent::SetConfigurationUnbootable { configuration: paver::Configuration::B }),
        // Make sure we flush, even if marking Unbootable failed.
        Paver(PaverEvent::BootManagerFlush),
    ]);
}

// Run an update with a given current config, assert that it succeeded, and return its interactions.
#[test_case(paver::Configuration::A, paver::Configuration::B; "writes_to_b_if_abr_supported_and_current_config_a")]
#[test_case(paver::Configuration::B, paver::Configuration::A; "writes_to_a_if_abr_supported_and_current_config_b")]
#[test_case(paver::Configuration::Recovery, paver::Configuration::A; "writes_to_a_if_abr_supported_and_current_config_r")]
#[fuchsia::test]
async fn assert_writes_for_current_and_target(
    current_config: paver::Configuration,
    target_config: paver::Configuration,
) {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                5,
                sha256(2),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();
    let env = TestEnv::builder()
        .paver_service(|builder| builder.current_config(current_config))
        .build()
        .await;

    env.resolver
        .register_package("update", crate::UPDATE_HASH)
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver
        .url(image_package_url_to_string("update-images-fuchsia", 9))
        .resolve(&env.resolver.package("zbi", hashstr(7)).add_file("zbi", "zbi contents"));

    env.run_update().await.expect("success");

    assert_eq!(
        env.get_ota_metrics().await,
        OtaMetrics {
            initiator:
                metrics::OtaResultAttemptsMigratedMetricDimensionInitiator::UserInitiatedCheck
                    as u32,
            phase: metrics::OtaResultAttemptsMigratedMetricDimensionPhase::SuccessPendingReboot
                as u32,
            status_code: metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::Success
                as u32,
        }
    );

    env.assert_interactions([
        Paver(PaverEvent::QueryCurrentConfiguration),
        Paver(PaverEvent::ReadAsset {
            configuration: current_config,
            asset: paver::Asset::VerifiedBootMetadata,
        }),
        Paver(PaverEvent::ReadAsset { configuration: current_config, asset: paver::Asset::Kernel }),
        Paver(PaverEvent::QueryCurrentConfiguration),
        Paver(PaverEvent::QueryConfigurationStatus { configuration: current_config }),
        Paver(PaverEvent::SetConfigurationUnbootable { configuration: target_config }),
        Paver(PaverEvent::BootManagerFlush),
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Paver(PaverEvent::ReadAsset { configuration: target_config, asset: paver::Asset::Kernel }),
        Paver(PaverEvent::ReadAsset { configuration: current_config, asset: paver::Asset::Kernel }),
        ReplaceRetainedPackages(vec![hash(9).into(), UPDATE_HASH.parse().unwrap()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-fuchsia", 9)),
        Paver(PaverEvent::WriteAsset {
            configuration: target_config,
            asset: paver::Asset::Kernel,
            payload: b"zbi contents".to_vec(),
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![UPDATE_HASH.parse().unwrap()]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: target_config }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]);
}

// Run an update with a given current config, assert that it succeeded, and return its interactions.
#[test_case(paver::Configuration::A, paver::Configuration::B; "writes_to_b_if_abr_supported_and_current_config_a")]
#[test_case(paver::Configuration::B, paver::Configuration::A; "writes_to_a_if_abr_supported_and_current_config_b")]
#[test_case(paver::Configuration::Recovery, paver::Configuration::A; "writes_to_a_if_abr_supported_and_current_config_r")]
#[fuchsia::test]
async fn assert_writes_for_current_and_target_packageless(
    current_config: paver::Configuration,
    target_config: paver::Configuration,
) {
    let zbi_content = b"zbi contents";
    let zbi_hash = fuchsia_merkle::root_from_slice(zbi_content);
    let manifest = OtaManifest {
        images: vec![manifest::Image {
            slot: manifest::Slot::AB,
            image_type: manifest::ImageType::Asset(AssetType::Zbi),
            blob: manifest::Blob {
                uncompressed_size: zbi_content.len() as u64,
                fuchsia_merkle_root: zbi_hash,
            },
        }],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| builder.current_config(current_config))
        .ota_manifest(manifest)
        .blob(zbi_hash, zbi_content.to_vec())
        .build()
        .await;

    env.run_packageless_update().await.expect("success");

    assert_eq!(
        env.get_ota_metrics().await,
        OtaMetrics {
            initiator:
                metrics::OtaResultAttemptsMigratedMetricDimensionInitiator::UserInitiatedCheck
                    as u32,
            phase: metrics::OtaResultAttemptsMigratedMetricDimensionPhase::SuccessPendingReboot
                as u32,
            status_code: metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::Success
                as u32,
        }
    );

    env.assert_interactions([
        Paver(PaverEvent::QueryCurrentConfiguration),
        Paver(PaverEvent::ReadAsset {
            configuration: current_config,
            asset: paver::Asset::VerifiedBootMetadata,
        }),
        Paver(PaverEvent::ReadAsset { configuration: current_config, asset: paver::Asset::Kernel }),
        Paver(PaverEvent::QueryCurrentConfiguration),
        Paver(PaverEvent::QueryConfigurationStatus { configuration: current_config }),
        Paver(PaverEvent::SetConfigurationUnbootable { configuration: target_config }),
        Paver(PaverEvent::BootManagerFlush),
        ReplaceRetainedBlobs(vec![zbi_hash.into()]),
        Gc,
        Paver(PaverEvent::ReadAsset { configuration: target_config, asset: paver::Asset::Kernel }),
        Paver(PaverEvent::ReadAsset { configuration: current_config, asset: paver::Asset::Kernel }),
        OtaDownloader(OtaDownloaderEvent::FetchBlob(zbi_hash.into())),
        Paver(PaverEvent::WriteAsset {
            configuration: target_config,
            asset: paver::Asset::Kernel,
            payload: b"zbi contents".to_vec(),
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedBlobs(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: target_config }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]);
}

/// Verifies that when we fail to resolve images, we get a Stage failure with the
/// expected `StageFailureReason`.
#[test_case(fidl_fuchsia_pkg::ResolveError::NoSpace, StageFailureReason::OutOfSpace; "out_of_space")]
#[test_case(fidl_fuchsia_pkg::ResolveError::AccessDenied, StageFailureReason::Internal; "internal_access_denied")]
#[test_case(fidl_fuchsia_pkg::ResolveError::RepoNotFound, StageFailureReason::Internal; "internal_repo_not_found")]
#[test_case(fidl_fuchsia_pkg::ResolveError::Internal, StageFailureReason::Internal; "internal_internal")]
#[test_case(fidl_fuchsia_pkg::ResolveError::Io, StageFailureReason::Internal; "internal_io")]
#[test_case(fidl_fuchsia_pkg::ResolveError::PackageNotFound, StageFailureReason::Internal; "internal_package_not_found")]
#[test_case(fidl_fuchsia_pkg::ResolveError::UnavailableBlob, StageFailureReason::Internal; "internal_unavailable_blob")]
#[fuchsia::test]
async fn stage_failure_reason(
    resolve_error: fidl_fuchsia_pkg::ResolveError,
    expected_reason: StageFailureReason,
) {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                5,
                sha256(2),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder().build().await;

    // ResolveError is only raised if images.json is present.
    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver.mock_resolve_failure(
        image_package_url_to_string("update-images-fuchsia", 9),
        resolve_error,
    );

    let mut attempt = env.start_update().await.unwrap();

    let info = UpdateInfo::builder().download_size(0).build();
    let progress = Progress::builder().fraction_completed(0.0).bytes_downloaded(0).build();
    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Stage(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(Progress::builder().fraction_completed(0.0).bytes_downloaded(0).build())
                .build()
        )
    );
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::FailStage(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(progress)
                .build()
                .with_stage_reason(expected_reason)
        )
    );
}

#[fuchsia::test]
async fn retry_image_package_resolve_once() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                5,
                sha256(2),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder().build().await;

    let base_package = "fuchsia-pkg://fuchsia.com/system_image/0?hash=beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead";

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([base_package]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver
        .url(base_package)
        .resolve(&env.resolver.package("deadbeef", hashstr(7)).add_file("beef", "dead"));

    env.resolver.url(image_package_url_to_string("update-images-fuchsia", 9)).respond_serially(
        vec![
            Err(ResolveError::NoSpace),
            Ok(env.resolver.package("zbi", hashstr(8)).add_file("zbi", "real zbi contents")),
        ],
    );

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        // Verify that base packages are removed from the retained package index if
        // image fails to resolve with OutOfSpace.
        ReplaceRetainedPackages(vec![
            "beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead".parse().unwrap(),
            hash(9).into(),
        ]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-fuchsia", 9)),
        ReplaceRetainedPackages(vec![hash(9).into()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-fuchsia", 9)),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"real zbi contents".to_vec(),
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![
            "beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead".parse().unwrap(),
        ]),
        Gc,
        PackageResolve(base_package.to_string()),
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn retry_image_blob_fetch_once_packageless() {
    let zbi_content = b"real zbi contents";
    let zbi_hash = fuchsia_merkle::root_from_slice(zbi_content);
    let content_blob = vec![1; 200];
    let content_blob_hash = fuchsia_merkle::root_from_slice(&content_blob);

    let manifest = OtaManifest {
        images: vec![manifest::Image {
            slot: manifest::Slot::AB,
            image_type: manifest::ImageType::Asset(AssetType::Zbi),
            blob: manifest::Blob {
                uncompressed_size: zbi_content.len() as u64,
                fuchsia_merkle_root: zbi_hash,
            },
        }],
        ..make_manifest([manifest::Blob {
            uncompressed_size: content_blob.len() as u64,
            fuchsia_merkle_root: content_blob_hash,
        }])
    };

    let env = TestEnv::builder()
        .ota_manifest(manifest)
        .blob(content_blob_hash, content_blob)
        .build()
        .await;

    let handle_zbi_blob = env.ota_downloader_service.block_once(zbi_hash);

    let task = fasync::Task::spawn({
        let ota_downloader_service = Arc::clone(&env.ota_downloader_service);
        let blobfs = Arc::clone(&env.blobfs);
        async move {
            let sender = handle_zbi_blob.await.unwrap();
            let handle_zbi_blob_2 = ota_downloader_service.block_once(zbi_hash);
            sender.send(Err(ResolveError::NoSpace)).unwrap();
            let sender_2 = handle_zbi_blob_2.await.unwrap();
            let () = blobfs.write_blob(zbi_hash, zbi_content).await.unwrap();
            sender_2.send(Ok(zbi_content.len() as u64)).unwrap();
        }
    });

    env.run_packageless_update().await.expect("success");

    task.await;

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![zbi_hash.into(), content_blob_hash.into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        // Verify that base blobs are removed from the retained blob index if
        // image fails to resolve with OutOfSpace.
        OtaDownloader(OtaDownloaderEvent::FetchBlob(zbi_hash.into())),
        ReplaceRetainedBlobs(vec![zbi_hash.into()]),
        Gc,
        OtaDownloader(OtaDownloaderEvent::FetchBlob(zbi_hash.into())),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"real zbi contents".to_vec(),
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
async fn retry_image_package_resolve_twice_fails_update() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                5,
                sha256(2),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder().build().await;

    let base_package = "fuchsia-pkg://fuchsia.com/system_image/0?hash=beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead";

    env.resolver
        .url(base_package)
        .resolve(&env.resolver.package("deadbeef", hashstr(7)).add_file("beef", "dead"));

    env.resolver.url(image_package_url_to_string("update-images-fuchsia", 9)).respond_serially(
        vec![
            Err(ResolveError::NoSpace),
            Err(ResolveError::NoSpace),
            Ok(env.resolver.package("zbi", hashstr(8)).add_file("zbi", "real zbi contents")),
        ],
    );

    env.resolver.url(UPDATE_PKG_URL).resolve(
        &env.resolver
            .package("update", "upd4t3")
            .add_file("packages.json", make_packages_json([base_package]))
            .add_file("epoch.json", make_current_epoch_json())
            .add_file("images.json", serde_json::to_string(&images_json).unwrap()),
    );

    let mut attempt = env.start_update().await.unwrap();

    let info = UpdateInfo::builder().download_size(0).build();

    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);

    assert_eq!(attempt.next().await.unwrap().unwrap().id(), StateId::Stage);

    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::FailStage(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(Progress::none())
                .build()
                .with_stage_reason(StageFailureReason::OutOfSpace)
        )
    );

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        // Verify that base packages are removed from the retained package index if
        // image fails to resolve with OutOfSpace.
        ReplaceRetainedPackages(vec![
            "beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead".parse().unwrap(),
            hash(9).into(),
        ]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-fuchsia", 9)),
        ReplaceRetainedPackages(vec![hash(9).into()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-fuchsia", 9)),
    ]));
}

#[fuchsia::test]
async fn retry_image_blob_fetch_twice_fails_update_packageless() {
    let zbi_content = b"real zbi contents";
    let zbi_hash = fuchsia_merkle::root_from_slice(zbi_content);
    let content_blob = vec![1; 200];
    let content_blob_len = content_blob.len() as u64;
    let content_blob_hash = fuchsia_merkle::root_from_slice(&content_blob);

    let manifest = OtaManifest {
        images: vec![manifest::Image {
            slot: manifest::Slot::AB,
            image_type: manifest::ImageType::Asset(AssetType::Zbi),
            blob: manifest::Blob {
                uncompressed_size: zbi_content.len() as u64,
                fuchsia_merkle_root: zbi_hash,
            },
        }],
        ..make_manifest([manifest::Blob {
            uncompressed_size: content_blob_len,
            fuchsia_merkle_root: content_blob_hash,
        }])
    };

    let env = TestEnv::builder()
        .ota_manifest(manifest)
        .blob(content_blob_hash, content_blob)
        .build()
        .await;

    env.ota_downloader_service.set_fetch_blob_response(Err(ResolveError::NoSpace));

    let mut attempt = env.start_packageless_update().await.unwrap();

    let info = UpdateInfo::builder().download_size(0).build();
    let manifest_size = env.ota_manifest_size() as u64;
    let total_goal = manifest_size + zbi_content.len() as u64 * 2 + content_blob_len;
    let progress = Progress::builder()
        .fraction_completed(manifest_size as f32 / total_goal as f32)
        .bytes_downloaded(manifest_size)
        .build();

    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);

    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Stage(
            UpdateInfoAndProgress::builder().info(info).progress(Progress::none()).build()
        )
    );
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Stage(UpdateInfoAndProgress::builder().info(info).progress(progress).build())
    );

    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::FailStage(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(progress)
                .build()
                .with_stage_reason(StageFailureReason::OutOfSpace)
        )
    );

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![zbi_hash.into(), content_blob_hash.into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        // Verify that base blobs are removed from the retained blob index if
        // image fails to resolve with OutOfSpace.
        OtaDownloader(OtaDownloaderEvent::FetchBlob(zbi_hash.into())),
        ReplaceRetainedBlobs(vec![zbi_hash.into()]),
        Gc,
        OtaDownloader(OtaDownloaderEvent::FetchBlob(zbi_hash.into())),
    ]));
}

fn construct_events(
    middle: impl IntoIterator<Item = SystemUpdaterInteraction>,
) -> Vec<SystemUpdaterInteraction> {
    crate::initial_interactions()
        .chain([PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string())])
        .chain(middle)
        .chain([
            Paver(PaverEvent::DataSinkFlush),
            ReplaceRetainedPackages(vec![]),
            Gc,
            BlobfsSync,
            Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
            Paver(PaverEvent::BootManagerFlush),
            Reboot,
        ])
        .collect()
}

#[fuchsia::test]
async fn writes_fuchsia() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                5,
                sha256(2),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset(|configuration, asset| {
                match (configuration, asset) {
                    (paver::Configuration::A, paver::Asset::Kernel) => {
                        Ok(b"not the right zbi".to_vec())
                    }
                    (paver::Configuration::B, paver::Asset::Kernel) => Ok(b"bad zbi".to_vec()),
                    (_, _) => Ok(vec![]),
                }
            }))
        })
        .build()
        .await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver
        .url(image_package_url_to_string("update-images-fuchsia", 9))
        .resolve(&env.resolver.package("zbi", hashstr(7)).add_file("zbi", "zbi contents"));

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        // Check that we read from both configurations and write resolved zbi contents
        // to desired configuration.
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        ReplaceRetainedPackages(vec![hash(9).into()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-fuchsia", 9)),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"zbi contents".to_vec(),
        }),
        // Rest of update flow.
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
async fn writes_fuchsia_packageless() {
    let zbi_content = b"zbi contents";
    let zbi_hash = fuchsia_merkle::root_from_slice(zbi_content);
    let manifest = OtaManifest {
        images: vec![manifest::Image {
            slot: manifest::Slot::AB,
            image_type: manifest::ImageType::Asset(AssetType::Zbi),
            blob: manifest::Blob {
                uncompressed_size: zbi_content.len() as u64,
                fuchsia_merkle_root: zbi_hash,
            },
        }],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset(|configuration, asset| {
                match (configuration, asset) {
                    (paver::Configuration::A, paver::Asset::Kernel) => {
                        Ok(b"not the right zbi".to_vec())
                    }
                    (paver::Configuration::B, paver::Asset::Kernel) => Ok(b"bad zbi".to_vec()),
                    (_, _) => Ok(vec![]),
                }
            }))
        })
        .ota_manifest(manifest)
        .blob(zbi_hash, zbi_content.to_vec())
        .build()
        .await;

    env.run_packageless_update().await.expect("run system updater");

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![zbi_hash.into()]),
        Gc,
        // Check that we read from both configurations and write resolved zbi contents
        // to desired configuration.
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        OtaDownloader(OtaDownloaderEvent::FetchBlob(zbi_hash.into())),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"zbi contents".to_vec(),
        }),
        // Rest of update flow.
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedBlobs(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn writes_fuchsia_vbmeta() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                5,
                sha256(2),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            Some(::update_package::ImageMetadata::new(
                5,
                sha256(1),
                image_package_resource_url("update-images-fuchsia", 9, "vbmeta"),
            )),
        )
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
            .package("zbi", hashstr(7))
            .add_file("zbi", "zbi contents")
            .add_file("vbmeta", "vbmeta contents"),
    );

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        // Events we care about.
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
        ReplaceRetainedPackages(vec![hash(9).into()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-fuchsia", 9)),
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
        // Rest of update flow.
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
async fn writes_fuchsia_vbmeta_packageless() {
    let zbi_content = b"zbi contents";
    let zbi_hash = fuchsia_merkle::root_from_slice(zbi_content);
    let vbmeta_content = b"vbmeta contents";
    let vbmeta_hash = fuchsia_merkle::root_from_slice(vbmeta_content);

    let manifest = OtaManifest {
        images: vec![
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
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .ota_manifest(manifest)
        .blob(zbi_hash, zbi_content.to_vec())
        .blob(vbmeta_hash, vbmeta_content.to_vec())
        .build()
        .await;

    env.run_packageless_update().await.expect("run system updater");

    env.assert_unordered_interactions(
        initial_interactions()
            .chain([ReplaceRetainedBlobs(vec![zbi_hash.into(), vbmeta_hash.into()]), Gc]),
        [
            // Events we care about.
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
            OtaDownloader(OtaDownloaderEvent::FetchBlob(zbi_hash.into())),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(vbmeta_hash.into())),
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
        ],
        [
            // Rest of update flow.
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

#[fuchsia::test]
async fn zbi_match_in_desired_config() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                8,
                MATCHING_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset(|configuration, asset| {
                match (configuration, asset) {
                    (paver::Configuration::B, paver::Asset::Kernel) => Ok(b"matching".to_vec()),
                    (_, _) => Ok(vec![]),
                }
            }))
        })
        .build()
        .await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    let events = vec![Paver(PaverEvent::ReadAsset {
        configuration: paver::Configuration::B,
        asset: paver::Asset::Kernel,
    })];
    assert_eq!(env.take_interactions(), construct_events(events));
}

#[fuchsia::test]
async fn zbi_match_in_desired_config_packageless() {
    let zbi_hash = fuchsia_merkle::root_from_slice(b"matching");
    let manifest = OtaManifest {
        images: vec![manifest::Image {
            slot: manifest::Slot::AB,
            image_type: manifest::ImageType::Asset(AssetType::Zbi),
            blob: manifest::Blob { uncompressed_size: 8, fuchsia_merkle_root: zbi_hash },
        }],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset(|configuration, asset| {
                match (configuration, asset) {
                    (paver::Configuration::B, paver::Asset::Kernel) => Ok(b"matching".to_vec()),
                    (_, _) => Ok(vec![]),
                }
            }))
        })
        .ota_manifest(manifest)
        .build()
        .await;

    env.run_packageless_update().await.expect("run system updater");

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![zbi_hash.into()]),
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

#[fuchsia::test]
async fn zbi_match_in_active_config() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                8,
                MATCHING_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset(|configuration, asset| {
                match (configuration, asset) {
                    (paver::Configuration::A, paver::Asset::Kernel) => Ok(b"matching".to_vec()),
                    (paver::Configuration::B, paver::Asset::Kernel) => {
                        Ok(b"not a match sorry".to_vec())
                    }
                    (_, _) => Ok(vec![]),
                }
            }))
        })
        .build()
        .await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    let events = [
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"matching".to_vec(),
        }),
    ];
    assert_eq!(env.take_interactions(), construct_events(events));
}

#[fuchsia::test]
async fn zbi_match_in_active_config_packageless() {
    let zbi_hash = fuchsia_merkle::root_from_slice(b"matching");
    let manifest = OtaManifest {
        images: vec![manifest::Image {
            slot: manifest::Slot::AB,
            image_type: manifest::ImageType::Asset(AssetType::Zbi),
            blob: manifest::Blob { uncompressed_size: 8, fuchsia_merkle_root: zbi_hash },
        }],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset(|configuration, asset| {
                match (configuration, asset) {
                    (paver::Configuration::A, paver::Asset::Kernel) => Ok(b"matching".to_vec()),
                    (paver::Configuration::B, paver::Asset::Kernel) => {
                        Ok(b"not a match sorry".to_vec())
                    }
                    (_, _) => Ok(vec![]),
                }
            }))
        })
        .ota_manifest(manifest)
        .build()
        .await;

    env.run_packageless_update().await.expect("run system updater");

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![zbi_hash.into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"matching".to_vec(),
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

#[fuchsia::test]
async fn zbi_match_in_active_config_error_in_desired_config() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                8,
                MATCHING_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset(|configuration, asset| {
                match (configuration, asset) {
                    (paver::Configuration::A, paver::Asset::Kernel) => Ok(b"matching".to_vec()),
                    (paver::Configuration::B, paver::Asset::Kernel) => Ok(vec![]),
                    (_, _) => Ok(vec![]),
                }
            }))
        })
        .build()
        .await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    let events = [
        // Even though an error is encountered while reading B (the VMO is smaller than the image),
        // system-updater still tries to read A instead of bailing immediately to the images
        // package.
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"matching".to_vec(),
        }),
    ];
    assert_eq!(env.take_interactions(), construct_events(events));
}

#[fuchsia::test]
async fn zbi_match_in_active_config_error_in_desired_config_packageless() {
    let zbi_hash = fuchsia_merkle::root_from_slice(b"matching");
    let manifest = OtaManifest {
        images: vec![manifest::Image {
            slot: manifest::Slot::AB,
            image_type: manifest::ImageType::Asset(AssetType::Zbi),
            blob: manifest::Blob { uncompressed_size: 8, fuchsia_merkle_root: zbi_hash },
        }],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset(|configuration, asset| {
                match (configuration, asset) {
                    (paver::Configuration::A, paver::Asset::Kernel) => Ok(b"matching".to_vec()),
                    (paver::Configuration::B, paver::Asset::Kernel) => Ok(vec![]),
                    (_, _) => Ok(vec![]),
                }
            }))
        })
        .ota_manifest(manifest)
        .build()
        .await;

    env.run_packageless_update().await.expect("run system updater");

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![zbi_hash.into()]),
        Gc,
        // Even though an error is encountered while reading B (the VMO is smaller than the image),
        // system-updater still tries to read A instead of bailing immediately to the images
        // package.
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"matching".to_vec(),
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

#[fuchsia::test]
async fn asset_comparing_respects_fuchsia_mem_buffer_size() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                8,
                MATCHING_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset_custom_buffer_size(|configuration, asset| {
                match (configuration, asset) {
                    // The read will return a VMO with the correct contents, but the
                    // fuchsia.mem.Buffer size is too small so system-updater will not use it.
                    (paver::Configuration::A, paver::Asset::Kernel) => Ok((b"matching".into(), 7)),
                    (paver::Configuration::B, paver::Asset::Kernel) => {
                        Ok((b"not a match sorry".to_vec(), 17))
                    }
                    (_, _) => Ok((vec![], 0)),
                }
            }))
        })
        .build()
        .await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver.url(image_package_url_to_string("update-images-fuchsia", 9)).resolve(
        &env.resolver.package("update-images-fuchsia", hashstr(9)).add_file("zbi", "matching"),
    );

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        // ZBI in A is read.
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        // But because the Buffer size is respected, it is not a match and the ZBI is resolved.
        ReplaceRetainedPackages(vec![hash(9).into()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-fuchsia", 9)),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"matching".to_vec(),
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
async fn asset_comparing_respects_fuchsia_mem_buffer_size_packageless() {
    let zbi_content = b"matching";
    let zbi_hash = fuchsia_merkle::root_from_slice(zbi_content);
    let manifest = OtaManifest {
        images: vec![manifest::Image {
            slot: manifest::Slot::AB,
            image_type: manifest::ImageType::Asset(AssetType::Zbi),
            blob: manifest::Blob { uncompressed_size: 8, fuchsia_merkle_root: zbi_hash },
        }],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset_custom_buffer_size(|configuration, asset| {
                match (configuration, asset) {
                    // The read will return a VMO with the correct contents, but the
                    // fuchsia.mem.Buffer size is too small so system-updater will not use it.
                    (paver::Configuration::A, paver::Asset::Kernel) => Ok((b"matching".into(), 7)),
                    (paver::Configuration::B, paver::Asset::Kernel) => {
                        Ok((b"not a match sorry".to_vec(), 17))
                    }
                    (_, _) => Ok((vec![], 0)),
                }
            }))
        })
        .ota_manifest(manifest)
        .blob(zbi_hash, zbi_content.to_vec())
        .build()
        .await;

    env.run_packageless_update().await.expect("run system updater");

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![zbi_hash.into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        // ZBI in A is read.
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        // But because the Buffer size is respected, it is not a match and the ZBI is downloaded.
        OtaDownloader(OtaDownloaderEvent::FetchBlob(zbi_hash.into())),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"matching".to_vec(),
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

#[fuchsia::test]
async fn asset_copying_sets_fuchsia_mem_buffer_size() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                8,
                MATCHING_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset(|configuration, asset| {
                match (configuration, asset) {
                    // The Buffer contains an extra 0u8, but ReadAsset can return the VMO of
                    // the entire partition (not just the image), so system-updater should use
                    // the size from the manifest in the update package and still find a match and
                    // write only the 8 bytes of the image.
                    (paver::Configuration::A, paver::Asset::Kernel) => Ok(b"matching\0".into()),
                    (paver::Configuration::B, paver::Asset::Kernel) => {
                        Ok(b"not a match sorry".to_vec())
                    }
                    (_, _) => Ok(vec![]),
                }
            }))
        })
        .build()
        .await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    let events = [
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            // Only 8 bytes are written, the trailing 0u8 returned by ReadAsset is ignored.
            payload: b"matching".to_vec(),
        }),
    ];
    assert_eq!(env.take_interactions(), construct_events(events));
}

#[fuchsia::test]
async fn asset_copying_sets_fuchsia_mem_buffer_size_packageless() {
    let zbi_hash = fuchsia_merkle::root_from_slice(b"matching");
    let manifest = OtaManifest {
        images: vec![manifest::Image {
            slot: manifest::Slot::AB,
            image_type: manifest::ImageType::Asset(AssetType::Zbi),
            blob: manifest::Blob { uncompressed_size: 8, fuchsia_merkle_root: zbi_hash },
        }],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset(|configuration, asset| {
                match (configuration, asset) {
                    // The Buffer contains an extra 0u8, but ReadAsset can return the VMO of
                    // the entire partition (not just the image), so system-updater should use
                    // the size from the manifest in the update package and still find a match and
                    // write only the 8 bytes of the image.
                    (paver::Configuration::A, paver::Asset::Kernel) => Ok(b"matching\0".into()),
                    (paver::Configuration::B, paver::Asset::Kernel) => {
                        Ok(b"not a match sorry".to_vec())
                    }
                    (_, _) => Ok(vec![]),
                }
            }))
        })
        .ota_manifest(manifest)
        .build()
        .await;

    env.run_packageless_update().await.expect("run system updater");

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![zbi_hash.into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            // Only 8 bytes are written, the trailing 0u8 returned by ReadAsset is ignored.
            payload: b"matching".to_vec(),
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

#[fuchsia::test]
async fn recovery_already_present() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .recovery_package(
            ::update_package::ImageMetadata::new(
                8,
                MATCHING_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                0,
                EMPTY_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset(|configuration, asset| {
                match (configuration, asset) {
                    (paver::Configuration::Recovery, paver::Asset::Kernel) => {
                        Ok(b"matching".to_vec())
                    }
                    (_, _) => Ok(vec![]),
                }
            }))
        })
        .build()
        .await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        // Events we really care about testing
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::Recovery,
            asset: paver::Asset::Kernel,
        }),
        // rest of the events.
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
async fn recovery_already_present_packageless() {
    let rzbi_hash = fuchsia_merkle::root_from_slice(b"matching");
    let manifest = OtaManifest {
        images: vec![
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob { uncompressed_size: 0, fuchsia_merkle_root: empty_merkle() },
            },
            manifest::Image {
                slot: manifest::Slot::R,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob { uncompressed_size: 8, fuchsia_merkle_root: rzbi_hash },
            },
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_asset(|configuration, asset| {
                match (configuration, asset) {
                    (paver::Configuration::Recovery, paver::Asset::Kernel) => {
                        Ok(b"matching".to_vec())
                    }
                    (_, _) => Ok(vec![]),
                }
            }))
        })
        .ota_manifest(manifest)
        .build()
        .await;

    env.run_update_with_options(MANIFEST_URL, default_options()).await.expect("run system updater");

    env.assert_unordered_interactions(
        initial_interactions()
            .chain([ReplaceRetainedBlobs(vec![empty_merkle().into(), rzbi_hash.into()]), Gc]),
        [
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
            }),
            // Events we really care about testing
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::Recovery,
                asset: paver::Asset::Kernel,
            }),
        ],
        [
            // rest of the events.
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

#[fuchsia::test]
async fn writes_recovery() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .recovery_package(
            ::update_package::ImageMetadata::new(
                5,
                sha256(2),
                image_package_resource_url("update-images-recovery", 9, "recovery"),
            ),
            None,
        )
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                0,
                EMPTY_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder().build().await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver.url(image_package_url_to_string("update-images-recovery", 9)).resolve(
        &env.resolver.package("recovery", hashstr(8)).add_file("recovery", "recovery zbi"),
    );

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        // Events we care about testing
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::Recovery,
            asset: paver::Asset::Kernel,
        }),
        ReplaceRetainedPackages(vec![hash(9).into()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-recovery", 9)),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::Recovery,
            asset: paver::Asset::Kernel,
            payload: b"recovery zbi".to_vec(),
        }),
        // rest of the events
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
async fn writes_recovery_packageless() {
    let rzbi_content = b"recovery zbi";
    let rzbi_hash = fuchsia_merkle::root_from_slice(rzbi_content);

    let manifest = OtaManifest {
        images: vec![
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob { uncompressed_size: 0, fuchsia_merkle_root: empty_merkle() },
            },
            manifest::Image {
                slot: manifest::Slot::R,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob {
                    uncompressed_size: rzbi_content.len() as u64,
                    fuchsia_merkle_root: rzbi_hash,
                },
            },
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .ota_manifest(manifest)
        .blob(rzbi_hash, rzbi_content.to_vec())
        .build()
        .await;

    env.run_update_with_options(MANIFEST_URL, default_options()).await.expect("run system updater");

    env.assert_unordered_interactions(
        initial_interactions()
            .chain([ReplaceRetainedBlobs(vec![empty_merkle().into(), rzbi_hash.into()]), Gc]),
        [
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
            }),
            // Events we care about testing
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::Recovery,
                asset: paver::Asset::Kernel,
            }),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(rzbi_hash.into())),
            Paver(PaverEvent::WriteAsset {
                configuration: paver::Configuration::Recovery,
                asset: paver::Asset::Kernel,
                payload: rzbi_content.to_vec(),
            }),
        ],
        [
            // rest of the events
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

#[fuchsia::test]
async fn writes_recovery_vbmeta() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .recovery_package(
            ::update_package::ImageMetadata::new(
                5,
                sha256(2),
                image_package_resource_url("update-images-recovery", 9, "recovery"),
            ),
            Some(::update_package::ImageMetadata::new(
                1,
                sha256(1),
                image_package_resource_url("update-images-recovery", 9, "recovery_vbmeta"),
            )),
        )
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                0,
                EMPTY_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 3, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder().build().await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver.url(image_package_url_to_string("update-images-recovery", 9)).resolve(
        &env.resolver
            .package("recovery", hashstr(8))
            .add_file("recovery", "recovery zbi")
            .add_file("recovery_vbmeta", "rvbmeta"),
    );

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        // Events we care about testing
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::Recovery,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::Recovery,
            asset: paver::Asset::VerifiedBootMetadata,
        }),
        ReplaceRetainedPackages(vec![hash(9).into()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-recovery", 9)),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::Recovery,
            asset: paver::Asset::Kernel,
            payload: b"recovery zbi".to_vec(),
        }),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::Recovery,
            asset: paver::Asset::VerifiedBootMetadata,
            payload: b"rvbmeta".to_vec(),
        }),
        // rest of the events
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
async fn writes_recovery_vbmeta_packageless() {
    let rzbi_content = b"recovery zbi";
    let rzbi_hash = fuchsia_merkle::root_from_slice(rzbi_content);
    let rvbmeta_content = b"rvbmeta";
    let rvbmeta_hash = fuchsia_merkle::root_from_slice(rvbmeta_content);

    let manifest = OtaManifest {
        images: vec![
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob { uncompressed_size: 0, fuchsia_merkle_root: empty_merkle() },
            },
            manifest::Image {
                slot: manifest::Slot::R,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob {
                    uncompressed_size: rzbi_content.len() as u64,
                    fuchsia_merkle_root: rzbi_hash,
                },
            },
            manifest::Image {
                slot: manifest::Slot::R,
                image_type: manifest::ImageType::Asset(AssetType::Vbmeta),
                blob: manifest::Blob {
                    uncompressed_size: rvbmeta_content.len() as u64,
                    fuchsia_merkle_root: rvbmeta_hash,
                },
            },
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .ota_manifest(manifest)
        .blob(rzbi_hash, rzbi_content.to_vec())
        .blob(rvbmeta_hash, rvbmeta_content.to_vec())
        .build()
        .await;

    env.run_update_with_options(MANIFEST_URL, default_options()).await.expect("run system updater");

    env.assert_unordered_interactions(
        initial_interactions().chain([
            ReplaceRetainedBlobs(vec![
                empty_merkle().into(),
                rzbi_hash.into(),
                rvbmeta_hash.into(),
            ]),
            Gc,
        ]),
        [
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
            }),
            // Events we care about testing
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::Recovery,
                asset: paver::Asset::Kernel,
            }),
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::Recovery,
                asset: paver::Asset::VerifiedBootMetadata,
            }),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(rzbi_hash.into())),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(rvbmeta_hash.into())),
            Paver(PaverEvent::WriteAsset {
                configuration: paver::Configuration::Recovery,
                asset: paver::Asset::Kernel,
                payload: rzbi_content.to_vec(),
            }),
            Paver(PaverEvent::WriteAsset {
                configuration: paver::Configuration::Recovery,
                asset: paver::Asset::VerifiedBootMetadata,
                payload: rvbmeta_content.to_vec(),
            }),
        ],
        [
            // rest of the events
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

#[fuchsia::test]
async fn recovery_present_but_should_write_recovery_is_false() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .recovery_package(
            ::update_package::ImageMetadata::new(
                5,
                sha256(2),
                image_package_resource_url("update-images-recovery", 3, "zbi"),
            ),
            None,
        )
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                5,
                sha256(1),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder().build().await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver
        .url(image_package_url_to_string("update-images-fuchsia", 9))
        .resolve(&env.resolver.package("fuchsia", hashstr(8)).add_file("zbi", "zbi contents"));

    env.run_update_with_options(
        "fuchsia-pkg://fuchsia.com/another-update/4",
        Options {
            initiator: Initiator::User,
            allow_attach_to_existing_attempt: true,
            should_write_recovery: false,
            manifest_range: None,
            manifest_headers: vec![],
        },
    )
    .await
    .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        // Note that we never look at recovery because the flag indicated it should be skipped!
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        ReplaceRetainedPackages(vec![hash(9).into()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-fuchsia", 9)),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"zbi contents".to_vec(),
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
async fn recovery_present_but_should_write_recovery_is_false_packageless() {
    let zbi_content = b"zbi contents";
    let zbi_hash = fuchsia_merkle::root_from_slice(zbi_content);
    let rzbi_content = b"rzbi_content";
    let rzbi_hash = fuchsia_merkle::root_from_slice(rzbi_content);

    let manifest = OtaManifest {
        images: vec![
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob {
                    uncompressed_size: zbi_content.len() as u64,
                    fuchsia_merkle_root: zbi_hash,
                },
            },
            manifest::Image {
                slot: manifest::Slot::R,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob {
                    uncompressed_size: rzbi_content.len() as u64,
                    fuchsia_merkle_root: rzbi_hash,
                },
            },
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .ota_manifest(manifest)
        .blob(zbi_hash, zbi_content.to_vec())
        .blob(rzbi_hash, rzbi_content.to_vec())
        .build()
        .await;

    env.run_update_with_options(
        MANIFEST_URL,
        Options {
            initiator: Initiator::User,
            allow_attach_to_existing_attempt: true,
            should_write_recovery: false,
            manifest_range: None,
            manifest_headers: vec![],
        },
    )
    .await
    .expect("success");

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![zbi_hash.into(), rzbi_hash.into()]),
        Gc,
        // Note that we never look at recovery because the flag indicated it should be skipped!
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        OtaDownloader(OtaDownloaderEvent::FetchBlob(zbi_hash.into())),
        Paver(PaverEvent::WriteAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
            payload: b"zbi contents".to_vec(),
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
