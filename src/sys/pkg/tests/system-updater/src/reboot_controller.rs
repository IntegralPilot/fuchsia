// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use fidl_fuchsia_update_installer_ext::StateId;
use pretty_assertions::assert_eq;
use test_case::test_case;

#[test_case(UPDATE_PKG_URL)]
#[test_case(MANIFEST_URL)]
#[fuchsia::test]
async fn reboot_controller_detach_causes_deferred_reboot(update_url: &str) {
    let env = TestEnv::builder().ota_manifest(make_manifest([])).build().await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", make_images_json_zbi());

    // Start the system update.
    let (reboot_proxy, server_end) = fidl::endpoints::create_proxy();
    let attempt = env
        .start_update_with_options(update_url, default_options(), Some(server_end))
        .await
        .unwrap();

    // When we call detach, we should observe DeferReboot at the end.
    let () = reboot_proxy.detach().unwrap();
    assert_eq!(
        attempt.map(|res| res.unwrap()).collect::<Vec<_>>().await.last().unwrap().id(),
        StateId::DeferReboot
    );

    // Verify we didn't make a reboot call.
    assert!(!env.take_interactions().contains(&Reboot));
}

#[test_case(UPDATE_PKG_URL)]
#[test_case(MANIFEST_URL)]
#[fuchsia::test]
async fn reboot_controller_unblock_causes_reboot(update_url: &str) {
    let env = TestEnv::builder().ota_manifest(make_manifest([])).build().await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", make_images_json_zbi());

    // Start the system update.
    let (reboot_proxy, server_end) = fidl::endpoints::create_proxy();
    let attempt = env
        .start_update_with_options(update_url, default_options(), Some(server_end))
        .await
        .unwrap();

    // When we call unblock, we should observe Reboot at the end.
    let () = reboot_proxy.unblock().unwrap();
    assert_eq!(
        attempt.map(|res| res.unwrap()).collect::<Vec<_>>().await.last().unwrap().id(),
        StateId::Reboot
    );

    // Verify we made a reboot call.
    assert_eq!(env.take_interactions().last().unwrap(), &Reboot);
}

#[test_case(UPDATE_PKG_URL)]
#[test_case(MANIFEST_URL)]
#[fuchsia::test]
async fn reboot_controller_dropped_causes_reboot(update_url: &str) {
    let env = TestEnv::builder().ota_manifest(make_manifest([])).build().await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", make_images_json_zbi());

    // Start the system update.
    let (reboot_proxy, server_end) = fidl::endpoints::create_proxy();
    let attempt = env
        .start_update_with_options(update_url, default_options(), Some(server_end))
        .await
        .unwrap();

    // When we drop the reboot controller, we should observe Reboot at the end.
    drop(reboot_proxy);
    assert_eq!(
        attempt.map(|res| res.unwrap()).collect::<Vec<_>>().await.last().unwrap().id(),
        StateId::Reboot
    );

    // Verify we made a reboot call.
    assert_eq!(env.take_interactions().last().unwrap(), &Reboot);
}
