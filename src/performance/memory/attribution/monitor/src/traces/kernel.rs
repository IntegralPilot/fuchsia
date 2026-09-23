// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::Result;
use fuchsia_async::{Interval, MonotonicDuration};
use fuchsia_trace::{category_enabled, counter};
use fuchsia_trace_observer::TraceObserver;
use futures::future::FutureExt;
use futures::{StreamExt, select};
use log::debug;
use stalls::StallProvider;
use stalls::refaults::RefaultProvider;
use std::ffi::CStr;

const CATEGORY_MEMORY_KERNEL: &'static CStr = c"memory:kernel";
// LINT.IfChange
const SAMPLE_INTERVAL: MonotonicDuration = MonotonicDuration::from_millis(200);
// LINT.ThenChange(//src/performance/memory/attribution/monitor/tests/traces/lib.rs)

// Continuously monitors the 'memory:kernel' trace category on a dedicated thread.
// Once enabled, it periodically records memory statistics until the category is disabled.
// This function runs indefinitely
pub async fn serve_forever(
    kernel_stats: impl fidl_fuchsia_kernel::StatsProxyInterface + Send + 'static,
    stall_provider: impl StallProvider + Send + 'static,
    page_refault_tracker: impl RefaultProvider + Send + 'static,
) {
    let (tx, rx) = futures::channel::oneshot::channel::<()>();
    std::thread::Builder::new()
        .name("memory:tracing".to_string())
        .spawn(move || {
            let mut executor = fuchsia_async::LocalExecutor::default();
            executor.run_singlethreaded(serve_forever_loop(
                kernel_stats,
                stall_provider,
                page_refault_tracker,
            ));
            // The task failed, send a message.
            let _ = tx.send(());
        })
        .expect("failed to spawn memory:tracing thread");
    let _ = rx.await;
}

// Internal loop, kept public for testing in
// //src/performance/memory/attribution/monitor/tests/traces.
pub async fn serve_forever_loop(
    kernel_stats: impl fidl_fuchsia_kernel::StatsProxyInterface,
    stall_provider: impl StallProvider,
    page_refault_tracker: impl RefaultProvider,
) {
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    fuchsia_trace_provider::trace_provider_wait_for_init();
    debug!("Start serving traces");
    let trace_observer = TraceObserver::new();
    loop {
        let mut interval = Interval::new(SAMPLE_INTERVAL);
        while category_enabled(CATEGORY_MEMORY_KERNEL) {
            if let Err(err) = publish_one_sample(
                &kernel_stats,
                stall_provider.clone(),
                page_refault_tracker.clone(),
            )
            .await
            {
                log::warn!("Failed to trace on category {:?} : {:?}", CATEGORY_MEMORY_KERNEL, err);
            }
            debug!("Wait for {} ms", SAMPLE_INTERVAL.into_millis());
            select! {
                _ = interval.next() => (),
                _ = trace_observer.on_state_changed().fuse() => (),
            };
        }
        debug!("Trace category {:?} not active. Waiting.", CATEGORY_MEMORY_KERNEL);
        let _ = trace_observer.on_state_changed().await;
        debug!("Trace event detected");
    }
}

async fn publish_one_sample(
    kernel_stats: &impl fidl_fuchsia_kernel::StatsProxyInterface,
    stall_provider: impl StallProvider,
    page_refault_tracker: impl RefaultProvider,
) -> Result<()> {
    debug!("Publish trace records for category {:?}", CATEGORY_MEMORY_KERNEL);
    let mem_stats = kernel_stats.get_memory_stats().await?;
    // Statistics are split into two records to comply with the 15-argument limit.
    counter!(CATEGORY_MEMORY_KERNEL, c"kmem_stats_a",0,
        "total_bytes"=>mem_stats.total_bytes.unwrap_or_default(),
        "free_bytes"=>mem_stats.free_bytes.unwrap_or_default(),
        "free_loaned_bytes"=>mem_stats.free_loaned_bytes.unwrap_or_default(),
        "wired_bytes"=>mem_stats.wired_bytes.unwrap_or_default(),
        "total_heap_bytes"=>mem_stats.total_heap_bytes.unwrap_or_default(),
        "free_heap_bytes"=>mem_stats.free_heap_bytes.unwrap_or_default(),
        "vmo_bytes"=>mem_stats.vmo_bytes.unwrap_or_default(),
        "mmu_overhead_bytes"=>mem_stats.mmu_overhead_bytes.unwrap_or_default(),
        "ipc_bytes"=>mem_stats.ipc_bytes.unwrap_or_default(),
        "cache_bytes"=>mem_stats.cache_bytes.unwrap_or_default(),
        "slab_bytes"=>mem_stats.slab_bytes.unwrap_or_default(),
        "zram_bytes"=>mem_stats.zram_bytes.unwrap_or_default(),
        "other_bytes"=>mem_stats.other_bytes.unwrap_or_default()
    );
    counter!(CATEGORY_MEMORY_KERNEL, c"kmem_stats_b", 0,
        "vmo_reclaim_total_bytes"=>mem_stats.vmo_reclaim_total_bytes.unwrap_or_default(),
        "vmo_reclaim_newest_bytes"=>mem_stats.vmo_reclaim_newest_bytes.unwrap_or_default(),
        "vmo_reclaim_oldest_bytes"=>mem_stats.vmo_reclaim_oldest_bytes.unwrap_or_default(),
        "vmo_reclaim_disabled_bytes"=>mem_stats.vmo_reclaim_disabled_bytes.unwrap_or_default(),
        "vmo_discardable_locked_bytes"=>mem_stats.vmo_discardable_locked_bytes.unwrap_or_default(),
        "vmo_discardable_unlocked_bytes"=>mem_stats.vmo_discardable_unlocked_bytes.unwrap_or_default()
    );
    let cmp_stats = kernel_stats.get_memory_stats_compression().await?;
    counter!(CATEGORY_MEMORY_KERNEL, c"kmem_stats_compression", 0,
        "uncompressed_storage_bytes"=>cmp_stats.uncompressed_storage_bytes.unwrap_or_default(),
        "compressed_storage_bytes"=>cmp_stats.compressed_storage_bytes.unwrap_or_default(),
        "compressed_fragmentation_bytes"=>cmp_stats.compressed_fragmentation_bytes.unwrap_or_default(),
        "compression_time"=>cmp_stats.compression_time.unwrap_or_default(),
        "decompression_time"=>cmp_stats.decompression_time.unwrap_or_default(),
        "total_page_compression_attempts"=>cmp_stats.total_page_compression_attempts.unwrap_or_default(),
        "failed_page_compression_attempts"=>cmp_stats.failed_page_compression_attempts.unwrap_or_default(),
        "total_page_decompressions"=>cmp_stats.total_page_decompressions.unwrap_or_default(),
        "compressed_page_evictions"=>cmp_stats.compressed_page_evictions.unwrap_or_default(),
        "eager_page_compressions"=>cmp_stats.eager_page_compressions.unwrap_or_default(),
        "memory_pressure_page_compressions"=>cmp_stats.memory_pressure_page_compressions.unwrap_or_default(),
        "critical_memory_page_compressions"=>cmp_stats.critical_memory_page_compressions.unwrap_or_default()
    );

    if let Some(pd) = cmp_stats.pages_decompressed_within_log_time {
        counter!(
            CATEGORY_MEMORY_KERNEL,
            c"kmem_stats_compression_time",0,
            "pages_decompressed_unit_ns"=>cmp_stats.pages_decompressed_unit_ns.unwrap_or_default(),
            "pages_decompressed_within_log_time[0]"=>pd[0],
            "pages_decompressed_within_log_time[1]"=>pd[1],
            "pages_decompressed_within_log_time[2]"=>pd[2],
            "pages_decompressed_within_log_time[3]"=>pd[3],
            "pages_decompressed_within_log_time[4]"=>pd[4],
            "pages_decompressed_within_log_time[5]"=>pd[5],
            "pages_decompressed_within_log_time[6]"=>pd[6],
            "pages_decompressed_within_log_time[7]"=>pd[7]
        );
    }

    let stall_info = stall_provider.get_stall_info()?;
    counter!(
        CATEGORY_MEMORY_KERNEL,
        c"memory_stall",0,
        "stall_time_some_ns"=>u64::try_from(stall_info.some.as_nanos())?,
        "stall_time_full_ns"=>u64::try_from(stall_info.full.as_nanos())?,
        "page_refaults"=>page_refault_tracker.get_count()
    );

    Ok(())
}
