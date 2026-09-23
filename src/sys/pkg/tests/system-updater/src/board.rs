// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use pretty_assertions::assert_eq;
use test_case::test_case;

#[test_case(UPDATE_PKG_URL, vec![UPDATE_PKG_URL])]
#[test_case(MANIFEST_URL, vec![])]
#[fuchsia::test]
async fn validates_board(update_url: &str, expected_resolved_urls: Vec<&str>) {
    let env = TestEnv::builder()
        .ota_manifest(OtaManifest { board: "x64".into(), ..make_manifest([]) })
        .build()
        .await;

    env.set_board_name("x64");

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", make_images_json_zbi())
        .add_file("board", "x64");

    env.run_update_with_options(update_url, default_options()).await.expect("success");

    assert_eq!(resolved_urls(Arc::clone(&env.interactions)), expected_resolved_urls);
}

#[test_case(UPDATE_PKG_URL, vec![UPDATE_PKG_URL])]
#[test_case(MANIFEST_URL, vec![])]
#[fuchsia::test]
async fn rejects_mismatched_board(update_url: &str, expected_resolved_urls: Vec<&str>) {
    let env = TestEnv::builder()
        .ota_manifest(OtaManifest { board: "arm".into(), ..make_manifest([]) })
        .build()
        .await;

    env.set_board_name("x64");

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([SYSTEM_IMAGE_URL]))
        .add_file("board", "arm")
        .add_file("images.json", make_images_json_zbi())
        .add_file("version", "1.2.3.4");

    let result = env.run_update_with_options(update_url, default_options()).await;
    assert!(result.is_err(), "system updater succeeded when it should fail");

    // Expect to have failed prior to downloading images.
    assert_eq!(resolved_urls(Arc::clone(&env.interactions)), expected_resolved_urls);

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
