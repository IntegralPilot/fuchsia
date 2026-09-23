// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_kernel as fkernel;
use fuchsia_async::DurationExt as _;
use futures::prelude::*;
use std::sync::Arc;
use zx::{self as zx, Resource};

/// An implementation of the `fuchsia.kernel.Stats` protocol.
pub struct KernelStats {
    resource: Resource,
}

impl KernelStats {
    /// `resource` must be the info resource.
    pub fn new(resource: Resource) -> Arc<Self> {
        Arc::new(Self { resource })
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::StatsRequestStream,
    ) -> Result<(), Error> {
        while let Some(stats_request) = stream.try_next().await? {
            match stats_request {
                fkernel::StatsRequest::GetMemoryStats { responder } => {
                    let mem_stats = &self.resource.mem_stats()?;
                    let stats = fkernel::MemoryStats {
                        total_bytes: Some(mem_stats.total_bytes),
                        free_bytes: Some(mem_stats.free_bytes),
                        wired_bytes: Some(mem_stats.wired_bytes),
                        total_heap_bytes: Some(mem_stats.total_heap_bytes),
                        free_heap_bytes: Some(mem_stats.free_heap_bytes),
                        vmo_bytes: Some(mem_stats.vmo_bytes),
                        mmu_overhead_bytes: Some(mem_stats.mmu_overhead_bytes),
                        ipc_bytes: Some(mem_stats.ipc_bytes),
                        other_bytes: Some(mem_stats.other_bytes),
                        free_loaned_bytes: Some(mem_stats.free_loaned_bytes),
                        cache_bytes: Some(mem_stats.cache_bytes),
                        slab_bytes: Some(mem_stats.slab_bytes),
                        zram_bytes: Some(mem_stats.zram_bytes),
                        vmo_reclaim_total_bytes: Some(mem_stats.vmo_reclaim_total_bytes),
                        vmo_reclaim_newest_bytes: Some(mem_stats.vmo_reclaim_newest_bytes),
                        vmo_reclaim_oldest_bytes: Some(mem_stats.vmo_reclaim_oldest_bytes),
                        vmo_reclaim_disabled_bytes: Some(mem_stats.vmo_reclaim_disabled_bytes),
                        vmo_discardable_locked_bytes: Some(mem_stats.vmo_discardable_locked_bytes),
                        vmo_discardable_unlocked_bytes: Some(
                            mem_stats.vmo_discardable_unlocked_bytes,
                        ),
                        ..Default::default()
                    };
                    responder.send(&stats)?;
                }
                fkernel::StatsRequest::GetMemoryStatsExtended { responder } => {
                    let mem_stats_extended = &self.resource.mem_stats_extended()?;
                    let stats = fkernel::MemoryStatsExtended {
                        total_bytes: Some(mem_stats_extended.total_bytes),
                        free_bytes: Some(mem_stats_extended.free_bytes),
                        wired_bytes: Some(mem_stats_extended.wired_bytes),
                        total_heap_bytes: Some(mem_stats_extended.total_heap_bytes),
                        free_heap_bytes: Some(mem_stats_extended.free_heap_bytes),
                        vmo_bytes: Some(mem_stats_extended.vmo_bytes),
                        vmo_pager_total_bytes: Some(mem_stats_extended.vmo_pager_total_bytes),
                        vmo_pager_newest_bytes: Some(mem_stats_extended.vmo_pager_newest_bytes),
                        vmo_pager_oldest_bytes: Some(mem_stats_extended.vmo_pager_oldest_bytes),
                        vmo_discardable_locked_bytes: Some(
                            mem_stats_extended.vmo_discardable_locked_bytes,
                        ),
                        vmo_discardable_unlocked_bytes: Some(
                            mem_stats_extended.vmo_discardable_unlocked_bytes,
                        ),
                        mmu_overhead_bytes: Some(mem_stats_extended.mmu_overhead_bytes),
                        ipc_bytes: Some(mem_stats_extended.ipc_bytes),
                        other_bytes: Some(mem_stats_extended.other_bytes),
                        ..Default::default()
                    };
                    responder.send(&stats)?;
                }
                fkernel::StatsRequest::GetMemoryStatsCompression { responder } => {
                    let mem_stats_compression = &self.resource.mem_stats_compression()?;
                    let stats = fkernel::MemoryStatsCompression {
                        uncompressed_storage_bytes: Some(
                            mem_stats_compression.uncompressed_storage_bytes,
                        ),
                        compressed_storage_bytes: Some(
                            mem_stats_compression.compressed_storage_bytes,
                        ),
                        compressed_fragmentation_bytes: Some(
                            mem_stats_compression.compressed_fragmentation_bytes,
                        ),
                        compression_time: Some(mem_stats_compression.compression_time),
                        decompression_time: Some(mem_stats_compression.decompression_time),
                        total_page_compression_attempts: Some(
                            mem_stats_compression.total_page_compression_attempts,
                        ),
                        failed_page_compression_attempts: Some(
                            mem_stats_compression.failed_page_compression_attempts,
                        ),
                        total_page_decompressions: Some(
                            mem_stats_compression.total_page_decompressions,
                        ),
                        compressed_page_evictions: Some(
                            mem_stats_compression.compressed_page_evictions,
                        ),
                        eager_page_compressions: Some(
                            mem_stats_compression.eager_page_compressions,
                        ),
                        memory_pressure_page_compressions: Some(
                            mem_stats_compression.memory_pressure_page_compressions,
                        ),
                        critical_memory_page_compressions: Some(
                            mem_stats_compression.critical_memory_page_compressions,
                        ),
                        pages_decompressed_unit_ns: Some(
                            mem_stats_compression.pages_decompressed_unit_ns,
                        ),
                        pages_decompressed_within_log_time: Some(
                            mem_stats_compression.pages_decompressed_within_log_time,
                        ),
                        ..Default::default()
                    };
                    responder.send(&stats)?;
                }
                fkernel::StatsRequest::GetCpuStats { responder } => {
                    let cpu_stats = &self.resource.cpu_stats()?;
                    let mut per_cpu_stats: Vec<fkernel::PerCpuStats> =
                        Vec::with_capacity(cpu_stats.len());
                    for cpu_stat in cpu_stats.iter() {
                        per_cpu_stats.push(fkernel::PerCpuStats {
                            cpu_number: Some(cpu_stat.cpu_number),
                            flags: Some(cpu_stat.flags),
                            idle_time: Some(cpu_stat.idle_time),
                            reschedules: Some(cpu_stat.reschedules),
                            context_switches: Some(cpu_stat.context_switches),
                            irq_preempts: Some(cpu_stat.irq_preempts),
                            yields: Some(cpu_stat.yields),
                            ints: Some(cpu_stat.ints),
                            timer_ints: Some(cpu_stat.timer_ints),
                            timers: Some(cpu_stat.timers),
                            page_faults: Some(cpu_stat.page_faults),
                            exceptions: Some(cpu_stat.exceptions),
                            syscalls: Some(cpu_stat.syscalls),
                            reschedule_ipis: Some(cpu_stat.reschedule_ipis),
                            generic_ipis: Some(cpu_stat.generic_ipis),
                            normalized_busy_time: Some(cpu_stat.normalized_busy_time),
                            active_energy_consumption_nj: Some(
                                cpu_stat.active_energy_consumption_nj,
                            ),
                            idle_energy_consumption_nj: Some(cpu_stat.idle_energy_consumption_nj),
                            ..Default::default()
                        });
                    }
                    let stats = fkernel::CpuStats {
                        actual_num_cpus: per_cpu_stats.len() as u64,
                        per_cpu_stats: Some(per_cpu_stats),
                    };
                    responder.send(&stats)?;
                }
                fkernel::StatsRequest::GetCpuLoad { duration, responder } => {
                    if duration <= 0 {
                        return Err(anyhow::anyhow!("Duration must be greater than 0"));
                    }

                    // Record `start_time` before the first stats query, and `end_time` *after* the
                    // second stats query completes. This ensures the "total time" (`end_time` -
                    // `start_time`) will never be less than the duration spanned by `start_stats`
                    // to `end_stats`, which would be invalid.
                    let start_time = fuchsia_async::MonotonicInstant::now();
                    let start_stats = self.resource.cpu_stats()?;
                    fuchsia_async::Timer::new(
                        zx::MonotonicDuration::from_nanos(duration).after_now(),
                    )
                    .await;
                    let end_stats = self.resource.cpu_stats()?;
                    let end_time = fuchsia_async::MonotonicInstant::now();

                    let loads = calculate_cpu_loads(start_time, start_stats, end_time, end_stats);
                    responder.send(&loads)?;
                }
            }
        }
        Ok(())
    }
}

/// Uses start / end times and corresponding PerCpuStats to calculate and return a vector of per-CPU
/// load values as floats in the range 0.0 - 100.0.
fn calculate_cpu_loads(
    start_time: fuchsia_async::MonotonicInstant,
    start_stats: Vec<zx::PerCpuStats>,
    end_time: fuchsia_async::MonotonicInstant,
    end_stats: Vec<zx::PerCpuStats>,
) -> Vec<f32> {
    let elapsed_time = (end_time - start_time).into_nanos();
    start_stats
        .iter()
        .zip(end_stats.iter())
        .map(|(start, end)| {
            let normalized_busy_time = end.normalized_busy_time - start.normalized_busy_time;
            let load_pct = normalized_busy_time as f64 / elapsed_time as f64 * 100.0;
            load_pct as f32
        })
        .collect::<Vec<f32>>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_component::client::connect_to_protocol;
    use {fidl_fuchsia_kernel as fkernel, fuchsia_async as fasync};

    async fn get_info_resource() -> Result<Resource, Error> {
        let info_resource_provider = connect_to_protocol::<fkernel::InfoResourceMarker>()?;
        let info_resource_handle = info_resource_provider.get().await?;
        Ok(Resource::from(info_resource_handle))
    }

    enum OnError {
        Panic,
        Ignore,
    }

    async fn serve_kernel_stats(on_error: OnError) -> Result<fkernel::StatsProxy, Error> {
        let info_resource = get_info_resource().await?;

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fkernel::StatsMarker>();
        fasync::Task::local(KernelStats::new(info_resource).serve(stream).unwrap_or_else(
            move |e| match on_error {
                OnError::Panic => panic!("Error while serving kernel stats: {}", e),
                _ => {}
            },
        ))
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn get_mem_stats() -> Result<(), Error> {
        let kernel_stats_provider = serve_kernel_stats(OnError::Panic).await?;
        let mem_stats = kernel_stats_provider.get_memory_stats().await?;

        assert!(mem_stats.total_bytes.unwrap() > 0);
        assert!(mem_stats.total_heap_bytes.unwrap() > 0);
        assert!(mem_stats.slab_bytes.unwrap() > 0);
        Ok(())
    }

    #[fuchsia::test]
    async fn get_mem_stats_extended() -> Result<(), Error> {
        let kernel_stats_provider = serve_kernel_stats(OnError::Panic).await?;
        let mem_stats_extended = kernel_stats_provider.get_memory_stats_extended().await?;

        assert!(mem_stats_extended.total_bytes.unwrap() > 0);
        assert!(mem_stats_extended.total_heap_bytes.unwrap() > 0);

        Ok(())
    }

    #[fuchsia::test]
    async fn get_cpu_stats() -> Result<(), Error> {
        let kernel_stats_provider = serve_kernel_stats(OnError::Panic).await?;
        let cpu_stats = kernel_stats_provider.get_cpu_stats().await?;
        let actual_num_cpus = cpu_stats.actual_num_cpus;
        assert!(actual_num_cpus > 0);
        let per_cpu_stats = cpu_stats.per_cpu_stats.unwrap();

        let mut idle_time_sum = 0;
        let mut syscalls_sum = 0;

        for per_cpu_stat in per_cpu_stats.iter() {
            idle_time_sum += per_cpu_stat.idle_time.unwrap();
            syscalls_sum += per_cpu_stat.syscalls.unwrap();
        }

        assert!(idle_time_sum > 0);
        assert!(syscalls_sum > 0);

        Ok(())
    }

    #[fuchsia::test]
    async fn get_cpu_load_invalid_duration() {
        let kernel_stats_provider = serve_kernel_stats(OnError::Ignore).await.unwrap();

        // The server should close the channel when it receives an invalid argument
        assert_matches::assert_matches!(
            kernel_stats_provider.get_cpu_load(0).await,
            Err(fidl::Error::ClientChannelClosed { .. })
        );
    }

    #[fuchsia::test]
    async fn get_cpu_load() -> Result<(), Error> {
        let kernel_stats_provider = serve_kernel_stats(OnError::Panic).await?;
        let cpu_loads = kernel_stats_provider
            .get_cpu_load(zx::MonotonicDuration::from_seconds(1).into_nanos())
            .await?;

        assert!(
            cpu_loads.iter().all(|l| l > &0.0 && l <= &100.0),
            "Invalid CPU load value (expected range 0.0 - 100.0, received {:?}",
            cpu_loads
        );

        Ok(())
    }

    // Takes a vector of CPU loads and generates the necessary parameters that can be fed into
    // `calculate_cpu_loads` to result in those load calculations.
    fn parameters_for_expected_cpu_loads(
        cpu_loads: Vec<f32>,
    ) -> (
        fuchsia_async::MonotonicInstant,
        Vec<zx::PerCpuStats>,
        fuchsia_async::MonotonicInstant,
        Vec<zx::PerCpuStats>,
    ) {
        let start_time = fuchsia_async::MonotonicInstant::from_nanos(0);
        let end_time = fuchsia_async::MonotonicInstant::from_nanos(1000000000);

        let (start_stats, end_stats) = std::iter::repeat(zx::PerCpuStats::default())
            .zip(cpu_loads.into_iter().map(|load| {
                let end_time_f32 = end_time.into_nanos() as f32;
                let normalized_busy_time = (load / 100.0 * end_time_f32) as i64;
                zx::PerCpuStats { normalized_busy_time, ..zx::PerCpuStats::default() }
            }))
            .unzip();

        (start_time, start_stats, end_time, end_stats)
    }

    #[fuchsia::test]
    fn test_calculate_cpu_loads() -> Result<(), Error> {
        // CPU0 loaded to 75%
        let (start_time, start_stats, end_time, end_stats) =
            parameters_for_expected_cpu_loads(vec![75.0, 0.0]);
        assert_eq!(
            calculate_cpu_loads(start_time, start_stats, end_time, end_stats),
            vec![75.0, 0.0]
        );

        // CPU1 loaded to 75%
        let (start_time, start_stats, end_time, end_stats) =
            parameters_for_expected_cpu_loads(vec![0.0, 75.0]);
        assert_eq!(
            calculate_cpu_loads(start_time, start_stats, end_time, end_stats),
            vec![0.0, 75.0]
        );

        Ok(())
    }
}
