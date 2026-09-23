// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::resource_annotator::ResourceAnnotator;
use anyhow::Result;
use attribution_processing::{
    AttributionData, InflatedPrincipal, InflatedResource, Principal, ProcessedAttributionData,
    Resource, ZXName, digest, fplugin_serde,
};
use fdomain_fuchsia_memory_attribution_plugin as fplugin;
use regex_lite::Regex;
use serde::Serialize;

#[derive(Serialize)]
pub struct ComponentDetailedProfileResult {
    pub kernel: fplugin_serde::KernelStatistics,
    pub principals: Vec<InflatedPrincipal>,
    pub resources: Vec<InflatedResource>,
    pub resource_names: Vec<ZXName>,
    #[serde(with = "fplugin_serde::PerformanceImpactMetricsDef")]
    pub performance: fplugin::PerformanceImpactMetrics,
    pub digest: digest::Digest,
}

pub fn process_snapshot_detailed(
    snapshot: fplugin::Snapshot,
    resource_annotator: &ResourceAnnotator,
    list_vmos: bool,
) -> Result<ComponentDetailedProfileResult> {
    // Map from moniker token ID to Principal struct.
    let principals: Vec<Principal> =
        snapshot.principals.into_iter().flatten().map(|p| p.into()).collect();

    // Map from kernel resource koid to Resource struct.
    let resources: Vec<Resource> =
        snapshot.resources.into_iter().flatten().map(|r| r.into()).collect();
    // Map from subject moniker token ID to Attribution struct.
    let attributions =
        snapshot.attributions.unwrap_or_default().into_iter().map(|a| a.into()).collect();
    let bucket_definitions: Vec<digest::BucketDefinition> = snapshot
        .bucket_definitions
        .as_ref()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|bd| {
            let process = bd.process.as_ref().map(|p| Regex::new(&p)).transpose()?;
            let vmo = bd.vmo.as_ref().map(|p| Regex::new(&p)).transpose()?;
            let principal = bd.principal.as_ref().map(|a| Regex::new(&a)).transpose()?;
            Ok(digest::BucketDefinition {
                name: bd.name.clone().unwrap_or_default(),
                process,
                vmo,
                principal,
                event_code: 0, // The information is unavailable client side.
            })
        })
        .collect::<Result<_>>()?;
    let attribution_data = AttributionData {
        principals_vec: principals,
        resources_vec: resources,
        resource_names: snapshot
            .resource_names
            .unwrap_or_default()
            .iter()
            .map(|n| ZXName::from_bytes_lossy(n))
            .collect(),
        attributions,
    };

    let processed_attribution_data: ProcessedAttributionData =
        attribution_processing::attribute_vmos(attribution_data);
    let digest = digest::Digest::compute(
        &processed_attribution_data,
        snapshot
            .kernel_statistics
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing kernel statistics"))?
            .memory_stats
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing memory statistics"))?,
        snapshot
            .kernel_statistics
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing kernel statistics"))?
            .compression_stats
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing compression statistics"))?,
        &bucket_definitions,
        list_vmos,
    )
    .expect("Digest computation should succeed");

    let processed_attribution_data = resource_annotator.annotate(processed_attribution_data);

    Ok(ComponentDetailedProfileResult {
        kernel: snapshot.kernel_statistics.unwrap_or_default().into(),
        principals: processed_attribution_data.principals.into_values().collect(),
        resources: processed_attribution_data.resources.into_values().collect(),
        resource_names: processed_attribution_data.resource_names,
        digest,
        performance: snapshot.performance_metrics.unwrap_or_default(),
    })
}
