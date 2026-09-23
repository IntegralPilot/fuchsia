// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use crate::{merkle_str, pinned_pkg_url};
use fidl_fuchsia_update_installer_ext::{
    FetchFailureReason, PrepareFailureReason, Progress, StageFailureReason, State, StateId,
    UpdateInfo, UpdateInfoAndProgress,
};
use pretty_assertions::assert_eq;
use test_case::test_case;

#[fuchsia::test]
async fn fails_on_package_resolver_connect_error() {
    let env = TestEnvBuilder::new().unregister_protocol(Protocol::PackageResolver).build().await;

    let result = env.run_update().await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    env.assert_interactions(crate::initial_interactions().chain([
        // The connect succeeds, so the system updater only notices the resolver is not there when
        // it tries to resolve a package
    ]));
}

#[fuchsia::test]
async fn package_resolver_not_needed_for_packageless_update() {
    let env = TestEnvBuilder::new()
        .unregister_protocol(Protocol::PackageResolver)
        .ota_manifest(make_manifest([]))
        .build()
        .await;

    env.run_packageless_update().await.unwrap();
}

#[fuchsia::test]
async fn fails_on_ota_downloader_connect_error_packageless() {
    let env = TestEnvBuilder::new()
        .unregister_protocol(Protocol::OtaDownloader)
        .ota_manifest(make_manifest([manifest::Blob {
            uncompressed_size: 1,
            fuchsia_merkle_root: hash(0),
        }]))
        .build()
        .await;

    let result = env.run_packageless_update().await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![empty_merkle().into(), hash(0).into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedBlobs(vec![hash(0).into()]),
        Gc,
    ]));
}

#[fuchsia::test]
async fn fails_on_system_image_package_fetch_error() {
    let env = TestEnv::builder().build().await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", make_images_json_zbi());

    let system_image_url = SYSTEM_IMAGE_URL;
    env.resolver
        .mock_resolve_failure(system_image_url, fidl_fuchsia_pkg::ResolveError::PackageNotFound);

    let result = env.run_update().await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    assert_eq!(
        env.get_ota_metrics().await,
        OtaMetrics {
            initiator:
                metrics::OtaResultAttemptsMigratedMetricDimensionInitiator::UserInitiatedCheck
                    as u32,
            phase: metrics::OtaResultAttemptsMigratedMetricDimensionPhase::PackageDownload as u32,
            status_code: metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::Error as u32,
        }
    );

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![SYSTEM_IMAGE_HASH.parse().unwrap()]),
        Gc,
        PackageResolve(system_image_url.to_string()),
    ]));
}

#[fuchsia::test]
async fn fails_on_blob_fetch_error_packageless() {
    let env = TestEnvBuilder::new()
        .ota_manifest(make_manifest([manifest::Blob {
            uncompressed_size: 1,
            fuchsia_merkle_root: hash(0),
        }]))
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
            phase: metrics::OtaResultAttemptsMigratedMetricDimensionPhase::PackageDownload as u32,
            status_code: metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::Error as u32,
        }
    );

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![empty_merkle().into(), hash(0).into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedBlobs(vec![hash(0).into()]),
        Gc,
        OtaDownloader(OtaDownloaderEvent::FetchBlob(hash(0).into())),
    ]));
}

#[fuchsia::test]
async fn fails_on_content_package_fetch_error() {
    let env = TestEnv::builder().build().await;

    let pkg1_url = pinned_pkg_url!("package1/0", "aa");
    let pkg2_url = pinned_pkg_url!("package2/0", "00");
    let pkg3_url = pinned_pkg_url!("package3/0", "bb");
    let pkg4_url = pinned_pkg_url!("package4/0", "cc");
    let pkg5_url = pinned_pkg_url!("package5/0", "dd");
    let pkg6_url = pinned_pkg_url!("package6/0", "ee");

    let pkg1 = env.resolver.package("package1", merkle_str!("aa"));
    let pkg3 = env.resolver.package("package3", merkle_str!("bb"));
    let pkg4 = env.resolver.package("package4", merkle_str!("cc"));
    let pkg5 = env.resolver.package("package5", merkle_str!("dd"));

    env.resolver.url("fuchsia-pkg://fuchsia.com/update").resolve(
        &env.resolver
            .package("update", UPDATE_HASH)
            .add_file(
                "packages.json",
                make_packages_json([
                    SYSTEM_IMAGE_URL,
                    pkg1_url,
                    pkg2_url,
                    pkg3_url,
                    pkg4_url,
                    pkg5_url,
                    pkg6_url,
                ]),
            )
            .add_file("epoch.json", make_current_epoch_json())
            .add_file("images.json", make_images_json_zbi()),
    );
    env.resolver
        .url(SYSTEM_IMAGE_URL)
        .resolve(&env.resolver.package("system_image", SYSTEM_IMAGE_HASH));
    let mut handle_pkg1 = env.resolver.url(pkg1_url).block_once();
    let mut handle_pkg2 = env.resolver.url(pkg2_url).block_once();
    let mut handle_pkg3 = env.resolver.url(pkg3_url).block_once();
    let mut handle_pkg4 = env.resolver.url(pkg4_url).block_once();
    let mut handle_pkg5 = env.resolver.url(pkg5_url).block_once();
    env.resolver.url(pkg6_url).resolve(&env.resolver.package("package6", merkle_str!("ee")));

    let ((), ()) = future::join(
        async {
            let result = env.run_update().await;
            assert!(result.is_err(), "system updater succeeded when it should fail");
        },
        async move {
            // The system updater will try to resolve 5 packages concurrently. Wait for the
            // requests to come in.
            handle_pkg1.wait().await;
            handle_pkg2.wait().await;
            handle_pkg3.wait().await;
            handle_pkg4.wait().await;
            handle_pkg5.wait().await;

            // Return a failure for pkg2, and success for the other blocked packages.
            handle_pkg2.fail(fidl_fuchsia_pkg::ResolveError::PackageNotFound).await;

            handle_pkg1.resolve(&pkg1).await;
            handle_pkg3.resolve(&pkg3).await;
            handle_pkg4.resolve(&pkg4).await;
            handle_pkg5.resolve(&pkg5).await;
        },
    )
    .await;

    assert_eq!(
        env.get_ota_metrics().await,
        OtaMetrics {
            initiator:
                metrics::OtaResultAttemptsMigratedMetricDimensionInitiator::UserInitiatedCheck
                    as u32,
            phase: metrics::OtaResultAttemptsMigratedMetricDimensionPhase::PackageDownload as u32,
            status_code: metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::Error as u32,
        }
    );

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![
            SYSTEM_IMAGE_HASH.parse().unwrap(),
            merkle_str!("aa").parse().unwrap(),
            merkle_str!("00").parse().unwrap(),
            merkle_str!("bb").parse().unwrap(),
            merkle_str!("cc").parse().unwrap(),
            merkle_str!("dd").parse().unwrap(),
            merkle_str!("ee").parse().unwrap(),
            UPDATE_HASH.parse().unwrap(),
        ]),
        Gc,
        PackageResolve(SYSTEM_IMAGE_URL.to_string()),
        PackageResolve(pkg1_url.to_string()),
        PackageResolve(pkg2_url.to_string()),
        PackageResolve(pkg3_url.to_string()),
        PackageResolve(pkg4_url.to_string()),
        PackageResolve(pkg5_url.to_string()),
        // pkg6_url is never resolved, as pkg2's resolve finishes first with an error.
    ]));
}

#[fuchsia::test]
async fn fails_when_package_cache_sync_fails() {
    let env = TestEnv::builder().build().await;
    env.cache_service.set_sync_response(Err(Status::INTERNAL));
    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", make_images_json_zbi());
    env.resolver
        .url(SYSTEM_IMAGE_URL)
        .resolve(&env.resolver.package("system_image/0", SYSTEM_IMAGE_HASH));

    let result = env.run_update().await;

    assert!(result.is_err(), "system updater succeeded when it should fail");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![SYSTEM_IMAGE_HASH.parse().unwrap()]),
        Gc,
        PackageResolve(SYSTEM_IMAGE_URL.to_string()),
        BlobfsSync,
    ]));
}

#[fuchsia::test]
async fn fails_when_package_cache_sync_fails_packageless() {
    let env = TestEnv::builder().ota_manifest(make_manifest([])).build().await;
    env.cache_service.set_sync_response(Err(Status::INTERNAL));

    let result = env.run_packageless_update().await;

    assert!(result.is_err(), "system updater succeeded when it should fail");

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
    ]));
}

/// Verifies that when we fail to resolve the update package, we get a Prepare failure with the
/// expected `PrepareFailureReason`.
#[test_case(fidl_fuchsia_pkg::ResolveError::NoSpace, PrepareFailureReason::OutOfSpace)]
#[test_case(fidl_fuchsia_pkg::ResolveError::AccessDenied, PrepareFailureReason::Internal)]
#[test_case(fidl_fuchsia_pkg::ResolveError::RepoNotFound, PrepareFailureReason::Internal)]
#[test_case(fidl_fuchsia_pkg::ResolveError::Internal, PrepareFailureReason::Internal)]
#[test_case(fidl_fuchsia_pkg::ResolveError::Io, PrepareFailureReason::Internal)]
#[test_case(fidl_fuchsia_pkg::ResolveError::PackageNotFound, PrepareFailureReason::Internal)]
#[test_case(fidl_fuchsia_pkg::ResolveError::UnavailableBlob, PrepareFailureReason::Internal)]
#[fuchsia::test]
async fn test_prepare_failure_reason(
    resolve_error: fidl_fuchsia_pkg::ResolveError,
    expected_reason: PrepareFailureReason,
) {
    let env = TestEnv::builder().build().await;
    env.resolver.mock_resolve_failure(UPDATE_PKG_URL, resolve_error);

    let mut attempt = env.start_update().await.unwrap();

    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);
    assert_eq!(attempt.next().await.unwrap().unwrap(), State::FailPrepare(expected_reason));
}

/// Verifies that when we fail to fetch an image blob, we get a Stage failure with the
/// expected `StageFailureReason`.
#[test_case(fidl_fuchsia_pkg::ResolveError::NoSpace, StageFailureReason::OutOfSpace)]
#[test_case(fidl_fuchsia_pkg::ResolveError::AccessDenied, StageFailureReason::Internal)]
#[test_case(fidl_fuchsia_pkg::ResolveError::Internal, StageFailureReason::Internal)]
#[test_case(fidl_fuchsia_pkg::ResolveError::Io, StageFailureReason::Internal)]
#[test_case(fidl_fuchsia_pkg::ResolveError::BlobNotFound, StageFailureReason::Internal)]
#[test_case(fidl_fuchsia_pkg::ResolveError::UnavailableBlob, StageFailureReason::Internal)]
#[fuchsia::test]
async fn test_stage_failure_reason_packageless(
    resolve_error: fidl_fuchsia_pkg::ResolveError,
    expected_reason: StageFailureReason,
) {
    let mut manifest = make_manifest([]);
    manifest.images[0].blob.uncompressed_size = 1000;
    let env = TestEnv::builder().ota_manifest(manifest).build().await;
    env.ota_downloader_service.set_fetch_blob_response(Err(resolve_error));

    let mut attempt = env.start_packageless_update().await.unwrap();

    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);
    let info = UpdateInfo::builder().download_size(0).build();
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Stage(
            UpdateInfoAndProgress::builder().info(info).progress(Progress::none()).build()
        )
    );
    let manifest_size = env.ota_manifest_size() as u64;
    let progress = Progress::builder()
        .fraction_completed(manifest_size as f32 / (manifest_size as f32 + 2000.0))
        .bytes_downloaded(manifest_size)
        .build();
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
                .with_stage_reason(expected_reason)
        )
    );
}

/// Verifies that when we fail to resolve a non-update package, we get a Fetch failure with the
/// expected `FetchFailureReason`.
#[test_case(
    UPDATE_PKG_URL,
    fidl_fuchsia_pkg::ResolveError::NoSpace,
    FetchFailureReason::OutOfSpace
)]
#[test_case(
    UPDATE_PKG_URL,
    fidl_fuchsia_pkg::ResolveError::AccessDenied,
    FetchFailureReason::Internal
)]
#[test_case(
    UPDATE_PKG_URL,
    fidl_fuchsia_pkg::ResolveError::RepoNotFound,
    FetchFailureReason::Internal
)]
#[test_case(UPDATE_PKG_URL, fidl_fuchsia_pkg::ResolveError::Internal, FetchFailureReason::Internal)]
#[test_case(UPDATE_PKG_URL, fidl_fuchsia_pkg::ResolveError::Io, FetchFailureReason::Internal)]
#[test_case(
    UPDATE_PKG_URL,
    fidl_fuchsia_pkg::ResolveError::PackageNotFound,
    FetchFailureReason::Internal
)]
#[test_case(
    UPDATE_PKG_URL,
    fidl_fuchsia_pkg::ResolveError::UnavailableBlob,
    FetchFailureReason::Internal
)]
#[test_case(MANIFEST_URL, fidl_fuchsia_pkg::ResolveError::NoSpace, FetchFailureReason::OutOfSpace)]
#[test_case(
    MANIFEST_URL,
    fidl_fuchsia_pkg::ResolveError::AccessDenied,
    FetchFailureReason::Internal
)]
#[test_case(MANIFEST_URL, fidl_fuchsia_pkg::ResolveError::Internal, FetchFailureReason::Internal)]
#[test_case(MANIFEST_URL, fidl_fuchsia_pkg::ResolveError::Io, FetchFailureReason::Internal)]
#[test_case(
    MANIFEST_URL,
    fidl_fuchsia_pkg::ResolveError::BlobNotFound,
    FetchFailureReason::Internal
)]
#[test_case(
    MANIFEST_URL,
    fidl_fuchsia_pkg::ResolveError::UnavailableBlob,
    FetchFailureReason::Internal
)]
#[fuchsia::test]
async fn test_fetch_failure_reason(
    update_url: &str,
    resolve_error: fidl_fuchsia_pkg::ResolveError,
    expected_reason: FetchFailureReason,
) {
    let env = TestEnv::builder()
        .ota_manifest(make_manifest([manifest::Blob {
            uncompressed_size: 1,
            fuchsia_merkle_root: hash(0),
        }]))
        .build()
        .await;
    env.ota_downloader_service.set_fetch_blob_response(Err(resolve_error));
    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", make_images_json_zbi());
    env.resolver.mock_resolve_failure(SYSTEM_IMAGE_URL, resolve_error);

    let mut attempt =
        env.start_update_with_options(update_url, default_options(), None).await.unwrap();

    let (fraction_completed, expected_bytes_downloaded) = if update_url == UPDATE_PKG_URL {
        (0.5, 0)
    } else {
        let manifest_size = env.ota_manifest_size() as u64;
        (manifest_size as f32 / (manifest_size as f32 + 1.0), manifest_size)
    };
    let info = UpdateInfo::builder().download_size(0).build();
    let progress = Progress::builder()
        .fraction_completed(fraction_completed)
        .bytes_downloaded(expected_bytes_downloaded)
        .build();
    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);
    let state = loop {
        let state = attempt.next().await.unwrap().unwrap();
        if state.id() != StateId::Stage {
            break state;
        }
    };
    assert_eq!(
        state,
        State::Fetch(UpdateInfoAndProgress::builder().info(info).progress(progress).build())
    );
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::FailFetch(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(progress)
                .build()
                .with_fetch_reason(expected_reason)
        )
    );
}
