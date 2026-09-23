// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use assert_matches::assert_matches;
use fidl_fuchsia_update_installer_ext::State;
use pretty_assertions::assert_eq;

#[fuchsia::test]
async fn cancel_update() {
    let env = TestEnv::builder().build().await;

    let mut handle_update_pkg = env.resolver.url(UPDATE_PKG_URL).block_once();

    let mut attempt = env.start_update().await.unwrap();
    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);
    handle_update_pkg.wait().await;

    env.installer_proxy().cancel_update(None).await.unwrap().unwrap();
    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Canceled);
    assert_matches!(attempt.next().await, None);

    assert_eq!(
        env.get_ota_metrics().await,
        OtaMetrics {
            initiator:
                metrics::OtaResultAttemptsMigratedMetricDimensionInitiator::UserInitiatedCheck
                    as u32,
            phase: metrics::OtaResultAttemptsMigratedMetricDimensionPhase::Tufupdate as u32,
            status_code: metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::Canceled
                as u32,
        }
    );
}

#[fuchsia::test]
async fn cancel_update_packageless() {
    let env = TestEnv::builder().ota_manifest(make_manifest([])).build().await;

    let handle_ota_manifest = env.http_loader_service.block_once();

    let mut attempt = env.start_packageless_update().await.unwrap();
    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);

    let _resume_handle = handle_ota_manifest.await.unwrap();

    env.installer_proxy().cancel_update(None).await.unwrap().unwrap();
    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Canceled);
    assert_matches!(attempt.next().await, None);

    assert_eq!(
        env.get_ota_metrics().await,
        OtaMetrics {
            initiator:
                metrics::OtaResultAttemptsMigratedMetricDimensionInitiator::UserInitiatedCheck
                    as u32,
            phase: metrics::OtaResultAttemptsMigratedMetricDimensionPhase::Tufupdate as u32,
            status_code: metrics::OtaResultAttemptsMigratedMetricDimensionStatusCode::Canceled
                as u32,
        }
    );
}
