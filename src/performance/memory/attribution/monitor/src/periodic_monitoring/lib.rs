// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::{Context, Result};
use attribution_processing::digest::{BucketDefinition, Digest};
use attribution_processing::summary::MemorySummary;
use attribution_processing::{AttributionDataProvider, ProcessedAttributionData, attribute_vmos};
use fuchsia_async::WakeupTime;
use fuchsia_inspect::{ArrayProperty, Node, StringProperty};
use fuchsia_inspect_contrib::nodes::BoundedListNode;
use fuchsia_trace::duration;
use futures::{TryFutureExt, join, try_join};
use humansize::{BINARY, FormatSizeOptions, format_size};
use stalls::StallProvider;
use traces::CATEGORY_MEMORY_CAPTURE;

use fidl_fuchsia_kernel as fkernel;
use fidl_fuchsia_metrics as fmetrics;

/// Periodically collect and report memory attribution data.
///
/// This produces a simplified schedule where, instead of having
/// independent reports on their own cadence, we collect based on
/// cobalt's frequency and produce all other reports based on that one
/// collection, saving significant CPU at the expense of flexibility.
pub async fn periodic_monitoring(
    kernel_stats_proxy: fkernel::StatsProxy,
    attribution_data_service: &impl AttributionDataProvider,
    stall_provider: &impl StallProvider,
    metric_event_logger: &fmetrics::MetricEventLoggerProxy,
    bucket_definitions: &[BucketDefinition],
    inspect_root: Node,
) -> Result<()> {
    let mut _current; // Ensure the inspect property is kept as long as necessary.
    let mut bucket_list_node = std::cell::OnceCell::new();
    let bucket_names = std::cell::OnceCell::new();
    let bucket_codes = cobalt::prepare_bucket_codes(bucket_definitions);
    loop {
        {
            duration!(CATEGORY_MEMORY_CAPTURE, c"periodic_monitoring");
            let timestamp = zx::BootInstant::get();
            // Retrieve (concurrently) the data necessary to perform the aggregation.
            let (kmem_stats, kmem_stats_compression) = try_join!(
                kernel_stats_proxy.get_memory_stats().map_err(anyhow::Error::from),
                kernel_stats_proxy.get_memory_stats_compression().map_err(anyhow::Error::from)
            )
            .with_context(|| "Failed to get kernel memory stats")?;
            // This is the very expensive operation.
            let attribution_data = attribute_vmos(attribution_data_service.get_attribution_data()?);
            let digest = Digest::compute(
                &attribution_data,
                &kmem_stats,
                &kmem_stats_compression,
                bucket_definitions,
                false,
            )?;
            _current =
                update_inspect_summary(attribution_data, timestamp, &kmem_stats, &inspect_root);
            cobalt::upload_metrics(
                timestamp,
                &kmem_stats,
                metric_event_logger,
                &digest,
                &bucket_codes,
            )
            .await?;
            {
                // Initialize the inspect property containing the buckets names, if necessary.
                let _ = bucket_names.get_or_init(|| {
                    // Create inspect node to store buckets related information.
                    let bucket_names =
                        inspect_root.create_string_array("buckets", digest.buckets.len());
                    for (i, attribution_processing::digest::Bucket { name, .. }) in
                        digest.buckets.iter().enumerate()
                    {
                        bucket_names.set(i, name);
                    }
                    bucket_names
                });
            }
            update_inspect_history(
                timestamp,
                &digest,
                stall_provider,
                &mut bucket_list_node,
                &inspect_root,
            )?;
        }
        join!(
            fuchsia_async::Task::local(async {
                let _ = scudo::mallopt(scudo::M_PURGE_ALL, 0);
            }),
            zx::MonotonicDuration::from_minutes(5).into_timer()
        );
    }
}

fn update_inspect_summary(
    attribution_data: ProcessedAttributionData,
    timestamp: zx::BootInstant,
    kmem_stats: &fkernel::MemoryStats,
    inspect_root: &Node,
) -> StringProperty {
    inspect_root.create_string(
        "current",
        record_summary(attribution_data.summary(), timestamp, &kmem_stats),
    )
}

/// Update inspect data with collected memory information.
fn update_inspect_history(
    timestamp: zx::BootInstant,
    digest: &Digest,
    stall_provider: &impl StallProvider,
    bucket_list_node: &mut std::cell::OnceCell<BoundedListNode>,
    inspect_root: &Node,
) -> Result<()> {
    let stall_values =
        stall_provider.get_stall_info().with_context(|| "Unable to retrieve stall information")?;
    // Add an entry for the current aggregation.
    let _ = bucket_list_node
        .get_or_init(|| BoundedListNode::new(inspect_root.create_child("measurements"), 100));
    bucket_list_node.get_mut().unwrap().add_entry(|n| {
        n.record_int("timestamp", timestamp.into_nanos());
        {
            let committed_sizes = n.create_uint_array("bucket_sizes", digest.buckets.len());
            let populated_sizes =
                n.create_uint_array("bucket_sizes_populated", digest.buckets.len());
            for (i, b) in digest.buckets.iter().enumerate() {
                committed_sizes.set(i, b.committed_size as u64);
                populated_sizes.set(i, b.populated_size as u64);
            }
            n.record(committed_sizes);
            n.record(populated_sizes);
        }

        n.record_child("stalls", |child| {
            child.record_uint(
                "some_ms",
                stall_values.some.as_millis().try_into().unwrap_or(u64::MAX),
            );
            child.record_uint(
                "full_ms",
                stall_values.full.as_millis().try_into().unwrap_or(u64::MAX),
            );
        });
    });
    Ok(())
}

fn record_summary(
    mut summary: MemorySummary,
    timestamp: zx::Instant<zx::BootTimeline>,
    kmem_stats: &fkernel::MemoryStats,
) -> String {
    let size_options = FormatSizeOptions::from(BINARY).space_after_value(false);
    summary.principals.sort_by_key(|p| std::cmp::Reverse(p.populated_private));
    format!(
        "Time: {} VMO: {} Free: {}\n{}",
        timestamp.into_nanos(),
        kmem_stats
            .vmo_bytes
            .and_then(|b| Some(format_size(b, size_options)))
            .unwrap_or_else(|| "?".to_string()),
        kmem_stats
            .free_bytes
            .and_then(|b| Some(format_size(b, size_options)))
            .unwrap_or_else(|| "?".to_string()),
        summary
            .principals
            .iter_mut()
            .filter_map(|principal| {
                if principal.populated_total == 0 {
                    return None;
                }
                let (populated_private, populated_scaled, populated_total) = match (|| {
                    Some((
                        format_size(principal.populated_private, size_options),
                        format_size(principal.populated_scaled as u64, size_options),
                        format_size(principal.populated_total, size_options),
                    ))
                })(
                ) {
                    Some(ok) => ok,
                    None => return None,
                };
                let mut vmos = principal.vmos.iter().collect::<Vec<_>>();
                vmos.sort_by_key(|(_, vmo)| {
                    std::cmp::Reverse((vmo.committed_private, vmo.committed_scaled as u64))
                });
                let sizes = if populated_total == populated_private {
                    format_args!("{}", populated_total)
                } else {
                    format_args!("{} {} {}", populated_private, populated_scaled, populated_total)
                };
                Some(format!(
                    "{}: {}; {}",
                    principal.name,
                    sizes,
                    vmos.iter()
                        .filter_map(|(name, vmo)| {
                            if vmo.committed_total == 0 {
                                None
                            } else {
                                Some(format!(
                                    "{} {} {} {}",
                                    name,
                                    format_size(vmo.populated_private, size_options),
                                    format_size(vmo.populated_scaled as u64, size_options),
                                    format_size(vmo.populated_total, size_options)
                                ))
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("; ")
                ))
            })
            .collect::<Vec<_>>()
            .join("\n")
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use attribution_processing::{
        Attribution, AttributionData, GlobalPrincipalIdentifier, Principal, PrincipalDescription,
        PrincipalType, Resource, ResourceReference, ZXName,
    };
    use diagnostics_assertions::{NonZeroIntProperty, assert_data_tree};
    use std::num::NonZero;
    use std::time::Duration;

    use fidl_fuchsia_memory_attribution_plugin as fplugin;

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

    fn get_attribution_data() -> ProcessedAttributionData {
        attribute_vmos(AttributionData {
            principals_vec: vec![Principal {
                identifier: GlobalPrincipalIdentifier(NonZero::new(1).unwrap()),
                description: Some(PrincipalDescription::Component("principal".to_owned())),
                principal_type: PrincipalType::Runnable,
                parent: None,
            }],
            resources_vec: vec![Resource {
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
            }],
            resource_names: vec![ZXName::from_string_lossy("resource")],
            attributions: vec![Attribution {
                source: GlobalPrincipalIdentifier(NonZero::new(1).unwrap()),
                subject: GlobalPrincipalIdentifier(NonZero::new(1).unwrap()),
                resources: vec![ResourceReference::KernelObject(10)],
            }],
        })
    }

    #[derive(Clone)]
    struct FakeStallProvider {}
    impl StallProvider for FakeStallProvider {
        fn get_stall_info(&self) -> Result<stalls::MemoryStallMetrics, anyhow::Error> {
            Ok(stalls::MemoryStallMetrics {
                some: Duration::from_millis(10),
                full: Duration::from_millis(20),
            })
        }
    }

    #[fuchsia::test]
    async fn test_update_inspect() -> Result<()> {
        let inspector = fuchsia_inspect::Inspector::default();
        let digest_node = inspector.root().create_child("logger");
        let timestamp = zx::BootInstant::get();
        let attribution_data = get_attribution_data();
        let (kernel_stats, kernel_stats_compression) = get_kernel_stats();
        let digest = Digest::compute(
            &attribution_data,
            &kernel_stats,
            &kernel_stats_compression,
            &vec![],
            false,
        )?;
        let mut bucket_list_node = std::cell::OnceCell::new();
        // Update inspect history twice, and ensure both instances are recorded.
        let _summary =
            update_inspect_summary(attribution_data, timestamp, &kernel_stats, &digest_node);
        update_inspect_history(
            timestamp,
            &digest,
            &FakeStallProvider {},
            &mut bucket_list_node,
            &digest_node,
        )?;

        update_inspect_history(
            timestamp,
            &digest,
            &FakeStallProvider {},
            &mut bucket_list_node,
            &digest_node,
        )?;
        assert_data_tree!(inspector, root: {
            logger: {
                measurements: {
                    // First update.
                    "0": {
                        timestamp: NonZeroIntProperty,
                        bucket_sizes: vec![
                            1024u64, // Undigested: matches the single unmatched VMO
                            // Orphaned: vmo_bytes reported by the kernel but not covered by any
                            // bucket => 6 - 1024 => 0 (saturating, cannot be negative)
                            0u64,
                            54u64,   // Kernel: 3 wired + 4 heap + 7 mmu + 8 IPC + 9 other + 12 slab + 11 cache = 54
                            2u64,    // Free
                            14u64,   // [Addl]PagerTotal
                            15u64,   // [Addl]PagerNewest
                            16u64,   // [Addl]PagerOldest
                            18u64,   // [Addl]DiscardableLocked
                            19u64,   // [Addl]DiscardableUnlocked
                            21u64,   // [Addl]ZramCompressedBytes
                            6u64,    // [Addl]PopulatedAnonymousBytes
                        ],
                        bucket_sizes_populated: vec![
                            2048u64, // Undigested: matches the single unmatched VMO
                            // Orphaned: vmo_bytes reported by the kernel but not covered by any
                            // bucket => 6 - 1024 => 0 (saturating, cannot be negative)
                            0u64,
                            54u64,   // Kernel: 3 wired + 4 heap + 7 mmu + 8 IPC + 9 other + 12 slab + 11 cache = 54
                            2u64,    // Free
                            14u64,   // [Addl]PagerTotal
                            15u64,   // [Addl]PagerNewest
                            16u64,   // [Addl]PagerOldest
                            18u64,   // [Addl]DiscardableLocked
                            19u64,   // [Addl]DiscardableUnlocked
                            21u64,   // [Addl]ZramCompressedBytes
                            6u64,    // [Addl]PopulatedAnonymousBytes
                        ],

                        stalls: {
                            some_ms: 10u64,
                            full_ms: 20u64,
                        },
                    },
                    // Second update.
                    "1": {
                        timestamp: NonZeroIntProperty,
                        bucket_sizes: vec![
                            1024u64, // Undigested: matches the single unmatched VMO
                            // Orphaned: vmo_bytes reported by the kernel but not covered by any
                            // bucket => 6 - 1024 => 0 (saturating, cannot be negative)
                            0u64,
                            54u64,   // Kernel: 3 wired + 4 heap + 7 mmu + 8 IPC + 9 other + 12 slab + 11 cache = 54
                            2u64,    // Free
                            14u64,   // [Addl]PagerTotal
                            15u64,   // [Addl]PagerNewest
                            16u64,   // [Addl]PagerOldest
                            18u64,   // [Addl]DiscardableLocked
                            19u64,   // [Addl]DiscardableUnlocked
                            21u64,   // [Addl]ZramCompressedBytes
                            6u64,    // [Addl]PopulatedAnonymousBytes
                        ],
                        bucket_sizes_populated: vec![
                            2048u64, // Undigested: matches the single unmatched VMO
                            // Orphaned: vmo_bytes reported by the kernel but not covered by any
                            // bucket => 6 - 1024 => 0 (saturating, cannot be negative)
                            0u64,
                            54u64,   // Kernel: 3 wired + 4 heap + 7 mmu + 8 IPC + 9 other + 12 slab + 11 cache = 54
                            2u64,    // Free
                            14u64,   // [Addl]PagerTotal
                            15u64,   // [Addl]PagerNewest
                            16u64,   // [Addl]PagerOldest
                            18u64,   // [Addl]DiscardableLocked
                            19u64,   // [Addl]DiscardableUnlocked
                            21u64,   // [Addl]ZramCompressedBytes
                            6u64,    // [Addl]PopulatedAnonymousBytes
                        ],
                        stalls: {
                            some_ms: 10u64,
                            full_ms: 20u64,
                        },
                    },
                },
                current: regex::Regex::new(r"^Time: \d+ VMO: 6B Free: 2B\nprincipal: 2KiB; resource 2KiB 2KiB 2KiB")?,
            },
        });
        Ok(())
    }
}
