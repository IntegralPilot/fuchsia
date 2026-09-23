// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error_from_metrics_error;
use anyhow::Result;
use attribution_processing::digest::{BucketDefinition, Digest};
use cobalt_client::traits::{AsEventCode, AsEventCodes};
use cobalt_registry::MemoryLeakMigratedMetricDimensionTimeSinceBoot as TimeSinceBoot;
use fidl_fuchsia_kernel as fkernel;
use fidl_fuchsia_metrics as fmetrics;
use memory_metrics_registry::cobalt_registry;
use std::collections::HashMap;

/// Sorted list mapping durations to the largest event that is lower.
const UPTIME_LEVEL_INDEX: &[(zx::BootDuration, TimeSinceBoot)] = &[
    (zx::BootDuration::from_minutes(1), TimeSinceBoot::Up),
    (zx::BootDuration::from_minutes(30), TimeSinceBoot::UpOneMinute),
    (zx::BootDuration::from_hours(1), TimeSinceBoot::UpThirtyMinutes),
    (zx::BootDuration::from_hours(6), TimeSinceBoot::UpOneHour),
    (zx::BootDuration::from_hours(12), TimeSinceBoot::UpSixHours),
    (zx::BootDuration::from_hours(24), TimeSinceBoot::UpTwelveHours),
    (zx::BootDuration::from_hours(48), TimeSinceBoot::UpOneDay),
    (zx::BootDuration::from_hours(72), TimeSinceBoot::UpTwoDays),
    (zx::BootDuration::from_hours(144), TimeSinceBoot::UpThreeDays),
];

/// Convert an instant to the code corresponding to the largest uptime that is smaller.
fn get_uptime_event_code(capture_time: zx::BootInstant) -> TimeSinceBoot {
    let uptime = zx::Duration::from_nanos(capture_time.into_nanos());
    UPTIME_LEVEL_INDEX
        .iter()
        .find(|&&(time, _)| uptime < time)
        .map(|(_, code)| *code)
        .unwrap_or(TimeSinceBoot::UpSixDays)
}

fn kmem_events(kmem_stats: &fkernel::MemoryStats) -> impl Iterator<Item = fmetrics::MetricEvent> {
    use cobalt_registry::MemoryGeneralBreakdownMigratedMetricDimensionGeneralBreakdown as Breakdown;
    let make_event = |code: Breakdown, value| {
        Some(fmetrics::MetricEvent {
            metric_id: cobalt_registry::MEMORY_GENERAL_BREAKDOWN_MIGRATED_METRIC_ID,
            event_codes: vec![code.as_event_code()],
            payload: fmetrics::MetricEventPayload::IntegerValue(value? as i64),
        })
    };
    vec![
        make_event(Breakdown::TotalBytes, kmem_stats.total_bytes),
        make_event(
            Breakdown::UsedBytes,
            (|| Some((kmem_stats.total_bytes? as i64 - kmem_stats.free_bytes? as i64) as u64))(),
        ),
        make_event(Breakdown::FreeBytes, kmem_stats.free_bytes),
        make_event(Breakdown::VmoBytes, kmem_stats.vmo_bytes),
        make_event(Breakdown::KernelFreeHeapBytes, kmem_stats.free_heap_bytes),
        make_event(Breakdown::MmuBytes, kmem_stats.mmu_overhead_bytes),
        make_event(Breakdown::IpcBytes, kmem_stats.ipc_bytes),
        make_event(Breakdown::KernelTotalHeapBytes, kmem_stats.total_heap_bytes),
        make_event(Breakdown::WiredBytes, kmem_stats.wired_bytes),
        make_event(Breakdown::OtherBytes, kmem_stats.other_bytes),
    ]
    .into_iter()
    .flatten()
}

fn kmem_events_with_uptime(
    kmem_stats: &fkernel::MemoryStats,
    capture_time: zx::BootInstant,
) -> impl Iterator<Item = fmetrics::MetricEvent> {
    use cobalt_registry::MemoryLeakMigratedMetricDimensionGeneralBreakdown as Breakdown;
    let make_event = |code: Breakdown, value| {
        Some(fmetrics::MetricEvent {
            metric_id: cobalt_registry::MEMORY_LEAK_MIGRATED_METRIC_ID,
            event_codes: cobalt_registry::MemoryLeakMigratedEventCodes {
                general_breakdown: code,
                time_since_boot: get_uptime_event_code(capture_time),
            }
            .as_event_codes(),
            payload: fmetrics::MetricEventPayload::IntegerValue(value? as i64),
        })
    };
    vec![
        make_event(Breakdown::TotalBytes, kmem_stats.total_bytes),
        make_event(
            Breakdown::UsedBytes,
            (|| Some((kmem_stats.total_bytes? as i64 - kmem_stats.free_bytes? as i64) as u64))(),
        ),
        make_event(Breakdown::FreeBytes, kmem_stats.free_bytes),
        make_event(Breakdown::VmoBytes, kmem_stats.vmo_bytes),
        make_event(Breakdown::KernelFreeHeapBytes, kmem_stats.free_heap_bytes),
        make_event(Breakdown::MmuBytes, kmem_stats.mmu_overhead_bytes),
        make_event(Breakdown::IpcBytes, kmem_stats.ipc_bytes),
        make_event(Breakdown::KernelTotalHeapBytes, kmem_stats.total_heap_bytes),
        make_event(Breakdown::WiredBytes, kmem_stats.wired_bytes),
        make_event(Breakdown::OtherBytes, kmem_stats.other_bytes),
    ]
    .into_iter()
    .flatten()
}

fn digest_events<'a>(
    digest: &'a Digest,
    bucket_name_to_code: &'a HashMap<String, u32>,
) -> impl 'a + Iterator<Item = fmetrics::MetricEvent> {
    digest.buckets.iter().filter_map(|bucket| {
        Some(fmetrics::MetricEvent {
            metric_id: cobalt_registry::MEMORY_MIGRATED_METRIC_ID,
            event_codes: vec![*bucket_name_to_code.get(&bucket.name)?],
            payload: fmetrics::MetricEventPayload::IntegerValue(bucket.populated_size as i64),
        })
    })
}

pub fn prepare_bucket_codes(bucket_definitions: &[BucketDefinition]) -> HashMap<String, u32> {
    let mut bucket_name_to_code = HashMap::from([
        (
            "TotalBytes".to_string(),
            cobalt_registry::MemoryMigratedMetricDimensionBucket::TotalBytes.as_event_code(),
        ),
        (
            "Free".to_string(),
            cobalt_registry::MemoryMigratedMetricDimensionBucket::Free.as_event_code(),
        ),
        (
            "Kernel".to_string(),
            cobalt_registry::MemoryMigratedMetricDimensionBucket::Kernel.as_event_code(),
        ),
        (
            "Orphaned".to_string(),
            cobalt_registry::MemoryMigratedMetricDimensionBucket::Orphaned.as_event_code(),
        ),
        (
            "Undigested".to_string(),
            cobalt_registry::MemoryMigratedMetricDimensionBucket::Undigested.as_event_code(),
        ),
        (
            "[Addl]PagerTotal".to_string(),
            cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_PagerTotal.as_event_code(),
        ),
        (
            "[Addl]PagerNewest".to_string(),
            cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_PagerNewest
                .as_event_code(),
        ),
        (
            "[Addl]PagerOldest".to_string(),
            cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_PagerOldest
                .as_event_code(),
        ),
        (
            "[Addl]DiscardableLocked".to_string(),
            cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_DiscardableLocked
                .as_event_code(),
        ),
        (
            "[Addl]DiscardableUnlocked".to_string(),
            cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_DiscardableUnlocked
                .as_event_code(),
        ),
        (
            "[Addl]ZramCompressedBytes".to_string(),
            cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_ZramCompressedBytes
                .as_event_code(),
        ),
        (
            "[Addl]PopulatedAnonymousBytes".to_string(),
            cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_PopulatedAnonymousBytes
                .as_event_code(),
        ),
    ]);
    bucket_definitions.iter().for_each(|bucket_definition| {
        bucket_name_to_code
            .entry(bucket_definition.name.clone())
            .or_insert(bucket_definition.event_code as u32);
    });
    bucket_name_to_code
}

/// Upload cobalt data based on collected memory data.
pub async fn upload_metrics(
    timestamp: zx::BootInstant,
    kmem_stats: &fkernel::MemoryStats,
    metric_event_logger: &fmetrics::MetricEventLoggerProxy,
    digest: &Digest,
    bucket_codes: &HashMap<String, u32>,
) -> Result<()> {
    let events = kmem_events(kmem_stats)
        .chain(kmem_events_with_uptime(kmem_stats, timestamp))
        .chain(digest_events(digest, &bucket_codes));
    metric_event_logger
        .log_metric_events(&events.collect::<Vec<fmetrics::MetricEvent>>())
        .await?
        .map_err(error_from_metrics_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use attribution_processing::{
        Attribution, AttributionData, GlobalPrincipalIdentifier, Principal, PrincipalDescription,
        PrincipalType, ProcessedAttributionData, Resource, ResourceReference, ZXName,
        attribute_vmos,
    };
    use fidl_fuchsia_memory_attribution_plugin as fplugin;
    use futures::{TryFutureExt, TryStreamExt, try_join};
    use regex_lite::Regex;

    fn get_data() -> ProcessedAttributionData {
        let attribution_data = AttributionData {
            principals_vec: vec![Principal {
                identifier: GlobalPrincipalIdentifier::new_for_test(1),
                description: Some(PrincipalDescription::Component("principal".to_owned())),
                principal_type: PrincipalType::Runnable,
                parent: None,
            }],
            resources_vec: vec![
                // Orphaned VMO.
                Resource {
                    koid: 10,
                    name_index: 0,
                    resource_type: fplugin::ResourceType::Vmo(fplugin::Vmo {
                        parent: None,
                        private_committed_bytes: Some(1024),
                        private_populated_bytes: Some(2048),
                        scaled_committed_bytes: Some(1024),
                        scaled_populated_bytes: Some(2048),
                        total_committed_bytes: Some(1024),
                        total_populated_bytes: Some(2048),
                        ..Default::default()
                    }),
                },
                // VMO belonging to bucket1.
                Resource {
                    koid: 20,
                    name_index: 1,
                    resource_type: fplugin::ResourceType::Vmo(fplugin::Vmo {
                        parent: None,
                        private_committed_bytes: Some(1024),
                        private_populated_bytes: Some(2048),
                        scaled_committed_bytes: Some(1024),
                        scaled_populated_bytes: Some(2048),
                        total_committed_bytes: Some(1024),
                        total_populated_bytes: Some(2048),
                        ..Default::default()
                    }),
                },
                // Process owning bucket1's VMO.
                Resource {
                    koid: 30,
                    name_index: 1,
                    resource_type: fplugin::ResourceType::Process(fplugin::Process {
                        vmos: Some(vec![20]),
                        ..Default::default()
                    }),
                },
            ],
            resource_names: vec![
                ZXName::from_string_lossy("resource"),
                ZXName::from_string_lossy("bucket1_resource"),
            ],
            attributions: vec![Attribution {
                source: GlobalPrincipalIdentifier::new_for_test(1),
                subject: GlobalPrincipalIdentifier::new_for_test(1),
                resources: vec![ResourceReference::KernelObject(10)],
            }],
        };
        attribute_vmos(attribution_data)
    }

    fn get_kernel_stats() -> (fkernel::MemoryStats, fkernel::MemoryStatsCompression) {
        (
            fkernel::MemoryStats {
                total_bytes: Some(1),
                free_bytes: Some(2),
                wired_bytes: Some(3),
                total_heap_bytes: Some(4),
                free_heap_bytes: Some(5),
                vmo_bytes: Some(6),
                mmu_overhead_bytes: Some(7),
                ipc_bytes: Some(8),
                other_bytes: Some(9),
                free_loaned_bytes: Some(10),
                cache_bytes: Some(11),
                slab_bytes: Some(12),
                zram_bytes: Some(13),
                vmo_reclaim_total_bytes: Some(14),
                vmo_reclaim_newest_bytes: Some(15),
                vmo_reclaim_oldest_bytes: Some(16),
                vmo_reclaim_disabled_bytes: Some(17),
                vmo_discardable_locked_bytes: Some(18),
                vmo_discardable_unlocked_bytes: Some(19),
                ..Default::default()
            },
            fkernel::MemoryStatsCompression {
                uncompressed_storage_bytes: Some(20),
                compressed_storage_bytes: Some(21),
                compressed_fragmentation_bytes: Some(22),
                compression_time: Some(23),
                decompression_time: Some(24),
                total_page_compression_attempts: Some(25),
                failed_page_compression_attempts: Some(26),
                total_page_decompressions: Some(27),
                compressed_page_evictions: Some(28),
                eager_page_compressions: Some(29),
                memory_pressure_page_compressions: Some(30),
                critical_memory_page_compressions: Some(31),
                pages_decompressed_unit_ns: Some(32),
                pages_decompressed_within_log_time: Some([40, 41, 42, 43, 44, 45, 46, 47]),
                ..Default::default()
            },
        )
    }

    #[fuchsia::test]
    async fn test_upload_metrics() -> anyhow::Result<()> {
        let bucket_definitions = [BucketDefinition {
            name: "bucket1".to_string(),
            vmo: Some(Regex::new("bucket1.*")?),
            event_code: 1,
            process: None,
            principal: None,
        }];

        let (metric_event_logger, mut metric_event_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fmetrics::MetricEventLoggerMarker>();

        let (kmem_stats, kmem_stats_compression) = get_kernel_stats();
        let digest = Digest::compute(
            &get_data(),
            &kmem_stats,
            &kmem_stats_compression,
            &bucket_definitions,
            false,
        )?;
        let bucket_codes = prepare_bucket_codes(&bucket_definitions);
        let upload = upload_metrics(
            zx::BootInstant::get(),
            &kmem_stats,
            &metric_event_logger,
            &digest,
            &bucket_codes,
        );
        let uptime = get_uptime_event_code(zx::BootInstant::get());
        try_join!(metric_event_request_stream.try_next().and_then(|event| async {
            match event.unwrap() {
                fmetrics::MetricEventLoggerRequest::LogMetricEvents { events, responder } => {
                    responder.send(Ok(()))?;
                    // Kernel metrics
                    assert_eq!(
                        &events[0..10],
                        vec![
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_GENERAL_BREAKDOWN_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryGeneralBreakdownMigratedMetricDimensionGeneralBreakdown::TotalBytes.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(1)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_GENERAL_BREAKDOWN_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryGeneralBreakdownMigratedMetricDimensionGeneralBreakdown::UsedBytes.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(-1)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_GENERAL_BREAKDOWN_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryGeneralBreakdownMigratedMetricDimensionGeneralBreakdown::FreeBytes.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(2)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_GENERAL_BREAKDOWN_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryGeneralBreakdownMigratedMetricDimensionGeneralBreakdown::VmoBytes.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(6)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_GENERAL_BREAKDOWN_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryGeneralBreakdownMigratedMetricDimensionGeneralBreakdown::KernelFreeHeapBytes.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(5)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_GENERAL_BREAKDOWN_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryGeneralBreakdownMigratedMetricDimensionGeneralBreakdown::MmuBytes.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(7)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_GENERAL_BREAKDOWN_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryGeneralBreakdownMigratedMetricDimensionGeneralBreakdown::IpcBytes.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(8)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_GENERAL_BREAKDOWN_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryGeneralBreakdownMigratedMetricDimensionGeneralBreakdown::KernelTotalHeapBytes.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(4)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_GENERAL_BREAKDOWN_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryGeneralBreakdownMigratedMetricDimensionGeneralBreakdown::WiredBytes.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(3)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_GENERAL_BREAKDOWN_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryGeneralBreakdownMigratedMetricDimensionGeneralBreakdown::OtherBytes.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(9)
                            },]);
                    // Kernel metrics with uptime
                    assert_eq!(
                        &events[10..20],
                        vec![
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_LEAK_MIGRATED_METRIC_ID,
                                event_codes: cobalt_registry::MemoryLeakMigratedEventCodes {
                                    general_breakdown: cobalt_registry::MemoryLeakMigratedMetricDimensionGeneralBreakdown::TotalBytes, time_since_boot: uptime}.as_event_codes(),
                                payload: fmetrics::MetricEventPayload::IntegerValue(1)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_LEAK_MIGRATED_METRIC_ID,
                                event_codes: cobalt_registry::MemoryLeakMigratedEventCodes {
                                    general_breakdown: cobalt_registry::MemoryLeakMigratedMetricDimensionGeneralBreakdown::UsedBytes, time_since_boot:uptime}.as_event_codes(),
                                payload: fmetrics::MetricEventPayload::IntegerValue(-1)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_LEAK_MIGRATED_METRIC_ID,
                                event_codes: cobalt_registry::MemoryLeakMigratedEventCodes {
                                    general_breakdown: cobalt_registry::MemoryLeakMigratedMetricDimensionGeneralBreakdown::FreeBytes, time_since_boot:uptime}.as_event_codes(),
                                payload: fmetrics::MetricEventPayload::IntegerValue(2)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_LEAK_MIGRATED_METRIC_ID,
                                event_codes: cobalt_registry::MemoryLeakMigratedEventCodes {
                                    general_breakdown: cobalt_registry::MemoryLeakMigratedMetricDimensionGeneralBreakdown::VmoBytes, time_since_boot:uptime}.as_event_codes(),
                                payload: fmetrics::MetricEventPayload::IntegerValue(6)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_LEAK_MIGRATED_METRIC_ID,
                                event_codes: cobalt_registry::MemoryLeakMigratedEventCodes {
                                    general_breakdown: cobalt_registry::MemoryLeakMigratedMetricDimensionGeneralBreakdown::KernelFreeHeapBytes, time_since_boot:uptime}.as_event_codes(),
                                payload: fmetrics::MetricEventPayload::IntegerValue(5)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_LEAK_MIGRATED_METRIC_ID,
                                event_codes: cobalt_registry::MemoryLeakMigratedEventCodes {
                                    general_breakdown: cobalt_registry::MemoryLeakMigratedMetricDimensionGeneralBreakdown::MmuBytes, time_since_boot:uptime}.as_event_codes(),
                                payload: fmetrics::MetricEventPayload::IntegerValue(7)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_LEAK_MIGRATED_METRIC_ID,
                                event_codes: cobalt_registry::MemoryLeakMigratedEventCodes {
                                    general_breakdown: cobalt_registry::MemoryLeakMigratedMetricDimensionGeneralBreakdown::IpcBytes, time_since_boot:uptime}.as_event_codes(),
                                payload: fmetrics::MetricEventPayload::IntegerValue(8)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_LEAK_MIGRATED_METRIC_ID,
                                event_codes: cobalt_registry::MemoryLeakMigratedEventCodes {
                                    general_breakdown: cobalt_registry::MemoryLeakMigratedMetricDimensionGeneralBreakdown::KernelTotalHeapBytes, time_since_boot:uptime}.as_event_codes(),
                                payload: fmetrics::MetricEventPayload::IntegerValue(4)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_LEAK_MIGRATED_METRIC_ID,
                                event_codes: cobalt_registry::MemoryLeakMigratedEventCodes {
                                    general_breakdown: cobalt_registry::MemoryLeakMigratedMetricDimensionGeneralBreakdown::WiredBytes, time_since_boot:uptime}.as_event_codes(),
                                payload: fmetrics::MetricEventPayload::IntegerValue(3)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_LEAK_MIGRATED_METRIC_ID,
                                event_codes: cobalt_registry::MemoryLeakMigratedEventCodes {
                                    general_breakdown: cobalt_registry::MemoryLeakMigratedMetricDimensionGeneralBreakdown::OtherBytes, time_since_boot:uptime}.as_event_codes(),
                                payload: fmetrics::MetricEventPayload::IntegerValue(9)
                            },
                        ]
                    );
                    // Digest metrics
                    assert_eq!(
                        &events[20..],
                        vec![
                            // Buckets with custom definitions
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_MIGRATED_METRIC_ID,
                                event_codes: vec![1], // Corresponds to the "bucket1" bucket
                                payload: fmetrics::MetricEventPayload::IntegerValue(2048)
                            },
                            // Default buckets
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryMigratedMetricDimensionBucket::Undigested.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(2048)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryMigratedMetricDimensionBucket::Orphaned.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(0)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryMigratedMetricDimensionBucket::Kernel.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(54)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryMigratedMetricDimensionBucket::Free.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(2)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_PagerTotal.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(14)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_PagerNewest.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(15)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_PagerOldest.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(16)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_DiscardableLocked.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(18)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_DiscardableUnlocked.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(19)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_ZramCompressedBytes.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(21)
                            },
                            fmetrics::MetricEvent {
                                metric_id: cobalt_registry::MEMORY_MIGRATED_METRIC_ID,
                                event_codes: vec![cobalt_registry::MemoryMigratedMetricDimensionBucket::__Addl_PopulatedAnonymousBytes.as_event_code()],
                                payload: fmetrics::MetricEventPayload::IntegerValue(6)
                            }
                        ]
                    )
                }
                _ => panic!("Unexpected metric event"),
            }
            Ok(())}).map_err(|err| anyhow!(err)), upload)?;
        Ok(())
    }
}
