// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use crate::progress_reporting::assert_success_monitor_states;
use fidl_fuchsia_update_installer_ext::StateId;
use pretty_assertions::assert_eq;

#[fuchsia::test]
async fn updates_the_system() {
    let env = TestEnv::builder().build().await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", make_images_json_zbi())
        .add_file("version", "1.2.3.4");
    env.resolver
        .url(SYSTEM_IMAGE_URL)
        .resolve(&env.resolver.package("system_image/0", SYSTEM_IMAGE_HASH));

    env.run_update().await.expect("run system updater");

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
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn updates_the_system_packageless() {
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

    env.run_packageless_update().await.unwrap();

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
async fn requires_zbi() {
    let env = TestEnv::builder().build().await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("bootloader", "new bootloader");
    env.resolver
        .url(SYSTEM_IMAGE_URL)
        .resolve(&env.resolver.package("system_image/0", SYSTEM_IMAGE_HASH));

    let result = env.run_update().await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    env.assert_interactions(
        crate::initial_interactions().chain([PackageResolve(UPDATE_PKG_URL.to_string())]),
    );
}

#[fuchsia::test]
async fn requires_zbi_packageless() {
    let manifest = OtaManifest { images: vec![], ..make_manifest([]) };
    let env = TestEnv::builder().ota_manifest(manifest).build().await;

    let result = env.run_packageless_update().await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    env.assert_interactions(initial_interactions());
}

#[fuchsia::test]
async fn updates_the_system_with_progress() {
    let env = TestEnv::builder().build().await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", make_images_json_zbi())
        .add_file("version", "5.6.7.8");
    env.resolver
        .url(SYSTEM_IMAGE_URL)
        .resolve(&env.resolver.package("system_image/0", SYSTEM_IMAGE_HASH));

    // Start the system update.
    let attempt = env.start_update().await.unwrap();

    // Verify progress reporting events.
    assert_success_monitor_states(
        attempt.map(|res| res.unwrap()).collect().await,
        &[
            StateId::Prepare,
            StateId::Stage,
            StateId::Fetch,
            StateId::Commit,
            StateId::WaitToReboot,
            StateId::Reboot,
        ],
    );

    // Verify metrics reported.
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
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn updates_the_system_with_progress_packageless() {
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

    // Start the system update.
    let attempt = env.start_packageless_update().await.unwrap();

    // Verify progress reporting events.
    assert_success_monitor_states(
        attempt.map(|res| res.unwrap()).collect().await,
        &[
            StateId::Prepare,
            StateId::Stage,
            StateId::Fetch,
            StateId::Commit,
            StateId::WaitToReboot,
            StateId::Reboot,
        ],
    );

    // Verify metrics reported.
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
