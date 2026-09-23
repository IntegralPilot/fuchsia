// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use pretty_assertions::assert_eq;
use test_case::test_case;

#[test_case(
    UPDATE_PKG_URL,
    fidl_fuchsia_pkg::ResolveError::AccessDenied,
    metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::ErrorUntrustedTufRepo
)]
#[test_case(
    UPDATE_PKG_URL,
    fidl_fuchsia_pkg::ResolveError::NoSpace,
    metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::ErrorStorageOutOfSpace
)]
#[test_case(
    UPDATE_PKG_URL,
    fidl_fuchsia_pkg::ResolveError::Io,
    metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::ErrorStorage
)]
#[test_case(
    UPDATE_PKG_URL,
    fidl_fuchsia_pkg::ResolveError::UnavailableBlob,
    metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::ErrorNetworking
)]
#[test_case(
    MANIFEST_URL,
    fidl_fuchsia_pkg::ResolveError::AccessDenied,
    metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::ErrorUntrustedTufRepo
)]
#[test_case(
    MANIFEST_URL,
    fidl_fuchsia_pkg::ResolveError::NoSpace,
    metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::ErrorStorageOutOfSpace
)]
#[test_case(
    MANIFEST_URL,
    fidl_fuchsia_pkg::ResolveError::Io,
    metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::ErrorStorage
)]
#[test_case(
    MANIFEST_URL,
    fidl_fuchsia_pkg::ResolveError::UnavailableBlob,
    metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::ErrorNetworking
)]
#[fuchsia::test]
async fn test_resolve_error_maps_to_cobalt_status_code(
    update_url: &str,
    error: fidl_fuchsia_pkg::ResolveError,
    expected_status_code: metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode,
) {
    let env = TestEnv::builder()
        .ota_manifest(make_manifest([manifest::Blob {
            uncompressed_size: 1,
            fuchsia_merkle_root: hash(0),
        }]))
        .build()
        .await;
    env.ota_downloader_service.set_fetch_blob_response(Err(error));

    let pkg_url = "fuchsia-pkg://fuchsia.com/failure/0?hash=00112233445566778899aabbccddeeffffeeddccbbaa99887766554433221100";

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([pkg_url]))
        .add_file("images.json", make_images_json_zbi())
        .add_file("epoch.json", make_current_epoch_json());

    env.resolver.url(pkg_url).fail(error);

    let result = env.run_update_with_options(update_url, default_options()).await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    assert_eq!(
        env.get_ota_metrics().await,
        OtaMetrics {
            initiator:
                metrics::OtaResultAttemptsMigratedMetricDimensionInitiator::UserInitiatedCheck
                    as u32,
            phase: metrics::OtaResultAttemptsMigratedMetricDimensionPhase::PackageDownload as u32,
            status_code: expected_status_code as u32,
        }
    );
}

#[fuchsia::test]
async fn succeeds_even_if_metrics_fail_to_send() {
    let env = TestEnv::builder().unregister_protocol(Protocol::FuchsiaMetrics).build().await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("images.json", make_images_json_zbi())
        .add_file("epoch.json", make_current_epoch_json());

    env.run_update().await.expect("run system updater");

    let loggers = env.metric_event_logger_factory.clone_loggers();
    assert_eq!(loggers.len(), 0);

    env.assert_interactions(crate::initial_interactions().chain([
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
async fn succeeds_even_if_metrics_fail_to_send_packageless() {
    let env = TestEnv::builder()
        .unregister_protocol(Protocol::FuchsiaMetrics)
        .ota_manifest(make_manifest([]))
        .build()
        .await;

    env.run_packageless_update().await.expect("run system updater");

    let loggers = env.metric_event_logger_factory.clone_loggers();
    assert_eq!(loggers.len(), 0);

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
