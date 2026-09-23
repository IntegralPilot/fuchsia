// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use fidl_fuchsia_update_installer_ext::StateId;
use pretty_assertions::assert_eq;
use test_case::test_case;

#[fuchsia::test]
async fn writes_recovery_and_force_reboots_into_it() {
    let env = TestEnv::builder().build().await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("update-mode", force_recovery_json())
        .add_file("images.json", make_images_json_recovery());

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
            configuration: paver::Configuration::Recovery,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![]),
        Gc,
        Paver(PaverEvent::SetConfigurationUnbootable { configuration: paver::Configuration::A }),
        Paver(PaverEvent::SetConfigurationUnbootable { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn writes_recovery_and_force_reboots_into_it_packageless() {
    let env = TestEnv::builder().ota_manifest(make_forced_recovery_manifest()).build().await;

    env.run_packageless_update().await.expect("run system updater");

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
        ReplaceRetainedBlobs(vec![empty_merkle().into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::Recovery,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedBlobs(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationUnbootable { configuration: paver::Configuration::A }),
        Paver(PaverEvent::SetConfigurationUnbootable { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[test_case(UPDATE_PKG_URL)]
#[test_case(MANIFEST_URL)]
#[fuchsia::test]
async fn reboots_regardless_of_reboot_controller(update_url: &str) {
    let env = TestEnv::builder().ota_manifest(make_forced_recovery_manifest()).build().await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("update-mode", force_recovery_json())
        .add_file("images.json", make_images_json_recovery());

    // Start the system update.
    let (reboot_proxy, server_end) = fidl::endpoints::create_proxy();
    let attempt = env
        .start_update_with_options(update_url, default_options(), Some(server_end))
        .await
        .unwrap();
    let () = reboot_proxy.detach().unwrap();

    // Ensure the update attempt has completed.
    assert_eq!(
        attempt.map(|res| res.unwrap()).collect::<Vec<_>>().await.last().unwrap().id(),
        StateId::Reboot
    );
    assert_eq!(env.take_interactions().last().unwrap(), &Reboot);
}

#[fuchsia::test]
async fn rejects_zbi() {
    let env = TestEnv::builder().build().await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", make_images_json_zbi())
        .add_file("update-mode", force_recovery_json());

    let result = env.run_update().await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    env.assert_interactions(
        crate::initial_interactions().chain([PackageResolve(UPDATE_PKG_URL.to_string())]),
    );
}

#[fuchsia::test]
async fn rejects_zbi_packageless() {
    let manifest = OtaManifest { images: vec![], ..make_forced_recovery_manifest() };
    let env = TestEnv::builder().ota_manifest(manifest).build().await;

    let result = env.run_packageless_update().await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    env.assert_interactions(initial_interactions());
}

#[test_case(UPDATE_PKG_URL)]
#[test_case(MANIFEST_URL)]
#[fuchsia::test]
async fn rejects_skip_recovery_flag(update_url: &str) {
    let env = TestEnv::builder().ota_manifest(make_forced_recovery_manifest()).build().await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages", make_packages_json([]))
        .add_file("update-mode", force_recovery_json());

    let result = env
        .run_update_with_options(
            update_url,
            Options {
                initiator: Initiator::User,
                allow_attach_to_existing_attempt: true,
                should_write_recovery: false,
                manifest_range: None,
                manifest_headers: vec![],
            },
        )
        .await;
    assert!(result.is_err(), "system updater succeeded when it should fail");
}
