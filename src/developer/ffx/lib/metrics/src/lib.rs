// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use analytics::metrics_state::MetricsStatus;
use analytics::{
    AnalyticsError, add_custom_event, ga4_metrics, initialize_ga4_metrics_service,
    redact_host_and_user_from,
};
use anyhow::Context;
use ffx_build_version::VersionInfo;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

pub type Result<T> = std::result::Result<T, AnalyticsError>;

pub const GA4_PROPERTY_ID: &str = "G-L10R82HSYT";
pub const GA4_KEY: &str = "mHeVJ5GxQTCvAVCmVHn_dw";

pub async fn init_metrics_svc(
    analytics_path: Option<PathBuf>,
    build_info: VersionInfo,
    invoker: Option<String>,
    sdk_version: String,
) {
    let build_version = build_info.build_version;
    let _ = initialize_ga4_metrics_service(
        String::from("ffx"),
        analytics_path,
        build_version,
        sdk_version,
        GA4_PROPERTY_ID.to_string(),
        GA4_KEY.to_string(),
        invoker,
    )
    .await
    .with_context(|| "Could not initialize metrics service");
}

pub async fn enhanced_analytics() -> bool {
    match ga4_metrics().await {
        Ok(metrics_svc) => metrics_svc.opt_in_status() == MetricsStatus::OptedInEnhanced,
        Err(_) => false,
    }
}

pub fn sanitize(parameter: &str) -> String {
    redact_host_and_user_from(parameter)
}

pub async fn add_ffx_launch_event(
    connection_mode: Option<&str>,
    redacted_args: String,
    enhanced_args: Option<String>,
    time: u128,
    exit_code: i32,
    error_message: Option<String>,
) -> Result<()> {
    let u64_time = u64::try_from(time).unwrap_or(0);
    let call_stack = std::env::var("FUCHSIA_METRICS_CALL_STACK").unwrap_or_default();
    let custom_dimensions = BTreeMap::from([
        ("time", u64_time.into()),
        ("exit_code", exit_code.to_string().into()),
        ("error_message", error_message.unwrap_or_else(|| "".to_string()).into()),
        ("redacted_args", redacted_args.into()),
        ("call_stack", call_stack.into()),
    ]);
    let mut metrics_svc = ga4_metrics().await?;
    if let Some(connection_mode) = connection_mode {
        let _ = metrics_svc
            .add_custom_event(
                Some("ffx_connection_mode"),
                Some(connection_mode),
                None,
                BTreeMap::new(),
                Some("ffx_connection_mode"),
            )
            .await;
    }
    metrics_svc
        .add_custom_event(
            None,
            enhanced_args.as_ref().map(String::as_str),
            None,
            custom_dimensions,
            Some("invoke"),
        )
        .await?;
    metrics_svc.send_events().await?;
    Ok(())
}

pub async fn add_flash_partition_event(
    partition_name: &String,
    product_name: &String,
    board_name: &String,
    file_size: u64,
    flash_time: &Duration,
) -> Result<()> {
    let u64_time = u64::try_from(flash_time.as_millis()).unwrap_or(0);
    let custom_dimensions = BTreeMap::from([
        ("partition_name", partition_name.clone().into()),
        ("product_name", product_name.clone().into()),
        ("board_name", board_name.clone().into()),
        ("file_size", file_size.into()),
        ("flash_time", u64_time.into()),
    ]);
    add_custom_event(Some("ffx_flash"), None, None, custom_dimensions).await?;
    Ok(())
}
