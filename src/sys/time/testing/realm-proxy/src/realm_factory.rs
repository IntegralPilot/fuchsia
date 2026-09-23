// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_metrics_test::MetricEventLoggerQuerierProxy;
use fidl_test_time_realm::RealmOptions;
use fuchsia_component_test::RealmInstance;

use std::sync::Arc;
use timekeeper_integration_lib::{
    FakeClockController, NestedTimekeeper, PushSourcePuppet, RtcOptions, RtcUpdates,
};

/// Creates a test realm using the parameters supplied.
///
/// Args:
/// - `options`: the optional elements of the configuration. See [RealmOptions]
///   for option details.
/// - `fake_utc_clock`: the clock handle that Timekeeper will manage.
pub async fn create_realm(
    options: RealmOptions,
    fake_utc_clock: zx::Clock,
) -> Result<(
    RealmInstance,
    Arc<PushSourcePuppet>,
    RtcUpdates,
    MetricEventLoggerQuerierProxy,
    Option<FakeClockController>,
)> {
    log::debug!("create_realm: options: {:?}, fake_utc_clock={:?}", options, fake_utc_clock);
    let (nested_timekeeper, push_source_puppet, rtc_updates, metric_proxy, fake_clock_controller) =
        NestedTimekeeper::new(
            Arc::new(fake_utc_clock),
            options.rtc.map(|r| r.into()).unwrap_or(RtcOptions::None),
            !options.use_real_reference_clock.unwrap_or(false),
        )
        .await;
    Ok((
        nested_timekeeper.into(),
        push_source_puppet,
        rtc_updates,
        metric_proxy,
        fake_clock_controller,
    ))
}
