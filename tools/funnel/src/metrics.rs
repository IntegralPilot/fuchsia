// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use analytics::{ga4_metrics, get_analytics_dir, initialize_ga4_metrics_service};
use anyhow::Context;
use std::collections::BTreeMap;
use thiserror::Error;

pub const GA4_PROPERTY_ID: &str = "G-L10R82HSYT";
pub const GA4_KEY: &str = "mHeVJ5GxQTCvAVCmVHn_dw";

#[derive(Debug, Error)]
pub enum MetricsError {
    #[error("{}",.0)]
    CouldNotRecordMetrics(#[from] anyhow::Error),

    #[error("{}",.0)]
    Analytics(#[from] analytics::AnalyticsError),
}

pub trait MetricsService {
    #[allow(dead_code)]
    async fn record_invocation(
        &self,
        sub_command: String,
        exit_code: i32,
        error_message: Option<String>,
    ) -> Result<(), MetricsError>;
}

pub struct GaMetricsService {}

impl GaMetricsService {
    pub async fn new(version: String) -> Result<Self, MetricsError> {
        let _ = initialize_ga4_metrics_service(
            String::from("funnel"),
            get_analytics_dir().ok(),
            Some(version),
            "no_sdk".to_string(),
            GA4_PROPERTY_ID.to_string(),
            GA4_KEY.to_string(),
            None,
        )
        .await
        .with_context(|| "Could not initialize metrics service")?;
        Ok(Self {})
    }
}

impl MetricsService for GaMetricsService {
    async fn record_invocation(
        &self,
        sub_command: String,
        exit_code: i32,
        error_message: Option<String>,
    ) -> Result<(), MetricsError> {
        let custom_dimensions = BTreeMap::from([
            ("exit_code", exit_code.to_string().into()),
            ("error_message", error_message.unwrap_or_else(|| "".to_string()).into()),
        ]);
        let mut metrics_svc = ga4_metrics().await?;
        metrics_svc
            .add_custom_event(None, Some(&sub_command), None, custom_dimensions, Some("invoke"))
            .await?;
        metrics_svc.send_events().await.map_err(MetricsError::from)
    }
}
