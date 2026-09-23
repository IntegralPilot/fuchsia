// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// This module tests the Cobalt metrics reporting.
use {
    crate::TestEnv,
    assert_matches::assert_matches,
    cobalt_client::traits::AsEventCodes,
    cobalt_sw_delivery_registry as metrics,
    fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload},
    fidl_fuchsia_pkg_ext as fpkg_ext,
    fuchsia_pkg_testing::SystemImageBuilder,
};

async fn assert_count_events(
    env: &TestEnv,
    expected_metric_id: u32,
    expected_event_codes: Vec<impl AsEventCodes>,
) {
    let actual_events = env
        .mocks
        .logger_factory
        .wait_for_at_least_n_events_with_metric_id(expected_event_codes.len(), expected_metric_id)
        .await;
    assert_eq!(
        actual_events.len(),
        expected_event_codes.len(),
        "event count different than expected, actual_events: {actual_events:?}"
    );

    for (event, expected_codes) in
        actual_events.into_iter().zip(expected_event_codes.into_iter().map(|c| c.as_event_codes()))
    {
        assert_matches!(
            event,
            MetricEvent {
                metric_id,
                event_codes,
                payload: MetricEventPayload::Count(1),
            } if metric_id == expected_metric_id && event_codes == expected_codes
        )
    }
}

#[fuchsia::test]
async fn pkg_cache_open_failure() {
    let env = TestEnv::builder().build().await;

    assert_matches!(
        env.get_already_cached("0000000000000000000000000000000000000000000000000000000000000000")
            .await
            .map(|_| ()),
        Err(fpkg_ext::cache::GetAlreadyCachedError::MissingMetaFar)
    );
    assert_count_events(
        &env,
        metrics::PKG_CACHE_OPEN_MIGRATED_METRIC_ID,
        vec![metrics::PkgCacheOpenMigratedMetricDimensionResult::Io],
    )
    .await;
}

#[fuchsia::test]
async fn pkg_cache_open_success() {
    let system_image_package = SystemImageBuilder::new().build().await;
    let env =
        TestEnv::builder().blobfs_from_system_image(&system_image_package).await.build().await;

    let _ = env.get_already_cached(&system_image_package.hash().clone().to_string()).await.unwrap();

    assert_count_events(
        &env,
        metrics::PKG_CACHE_OPEN_MIGRATED_METRIC_ID,
        vec![metrics::PkgCacheOpenMigratedMetricDimensionResult::Success],
    )
    .await;
}
