// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_kernel::{
    CpuStats, MemoryStats, MemoryStatsCompression, MemoryStatsExtended, StatsProxyInterface,
};
use stalls::StallProvider;
use stalls::refaults::RefaultProvider;
use std::future::{self, Ready};
use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

// LINT.IfChange
const TRACE_SAMPLE_INTERVAL: fuchsia_async::MonotonicDuration =
    fuchsia_async::MonotonicDuration::from_millis(200);
// LINT.ThenChange(//src/performance/memory/attribution/monitor/src/traces/kernel.rs)

#[derive(Clone)]
struct FakeStallProvider {}

impl StallProvider for FakeStallProvider {
    fn get_stall_info(&self) -> Result<stalls::MemoryStallMetrics, anyhow::Error> {
        Ok(stalls::MemoryStallMetrics {
            some: Duration::from_nanos(1),
            full: Duration::from_nanos(2),
        })
    }
}

#[derive(Default, Clone)]
struct FakeRefaultTracker {}

impl RefaultProvider for FakeRefaultTracker {
    fn get_count(&self) -> u64 {
        128
    }
}

struct FakeStatsProxy {
    counter: Arc<AtomicU64>,
}
impl FakeStatsProxy {
    fn new() -> FakeStatsProxy {
        FakeStatsProxy { counter: Arc::new(AtomicU64::new(0)) }
    }
}

impl StatsProxyInterface for FakeStatsProxy {
    type GetMemoryStatsResponseFut = Ready<Result<MemoryStats, fidl::Error>>;
    fn get_memory_stats(&self) -> <Self as StatsProxyInterface>::GetMemoryStatsResponseFut {
        let base = self.counter.fetch_add(1000, Ordering::Relaxed);
        future::ready(Ok(MemoryStats {
            total_bytes: Some(base + 1),
            free_bytes: Some(base + 2),
            wired_bytes: Some(base + 3),
            total_heap_bytes: Some(base + 4),
            free_heap_bytes: Some(base + 5),
            vmo_bytes: Some(base + 6),
            mmu_overhead_bytes: Some(base + 7),
            ipc_bytes: Some(base + 8),
            other_bytes: Some(base + 9),
            free_loaned_bytes: Some(base + 10),
            cache_bytes: Some(base + 11),
            slab_bytes: Some(base + 12),
            zram_bytes: Some(base + 13),
            vmo_reclaim_total_bytes: Some(base + 14),
            vmo_reclaim_newest_bytes: Some(base + 15),
            vmo_reclaim_oldest_bytes: Some(base + 16),
            vmo_reclaim_disabled_bytes: Some(base + 17),
            vmo_discardable_locked_bytes: Some(base + 18),
            vmo_discardable_unlocked_bytes: Some(base + 19),
            ..Default::default()
        }))
    }
    type GetMemoryStatsExtendedResponseFut = Ready<Result<MemoryStatsExtended, fidl::Error>>;
    fn get_memory_stats_extended(
        &self,
    ) -> <Self as StatsProxyInterface>::GetMemoryStatsExtendedResponseFut {
        unimplemented!();
    }
    type GetMemoryStatsCompressionResponseFut = Ready<Result<MemoryStatsCompression, fidl::Error>>;
    fn get_memory_stats_compression(
        &self,
    ) -> <Self as StatsProxyInterface>::GetMemoryStatsCompressionResponseFut {
        let base = self.counter.fetch_add(1000, Ordering::Relaxed);
        future::ready(Ok(MemoryStatsCompression {
            uncompressed_storage_bytes: Some(base + 100),
            compressed_storage_bytes: Some(base + 101),
            compressed_fragmentation_bytes: Some(base + 102),
            compression_time: Some((base + 103).try_into().unwrap()),
            decompression_time: Some((base + 104).try_into().unwrap()),
            total_page_compression_attempts: Some(base + 105),
            failed_page_compression_attempts: Some(base + 106),
            total_page_decompressions: Some(base + 107),
            compressed_page_evictions: Some(base + 108),
            eager_page_compressions: Some(base + 109),
            memory_pressure_page_compressions: Some(base + 110),
            critical_memory_page_compressions: Some(base + 111),
            pages_decompressed_unit_ns: Some(base + 112),
            pages_decompressed_within_log_time: Some([
                base + 113,
                114,
                115,
                116,
                117,
                118,
                119,
                120,
            ]),
            ..Default::default()
        }))
    }
    type GetCpuStatsResponseFut = Ready<Result<CpuStats, fidl::Error>>;
    fn get_cpu_stats(&self) -> <Self as StatsProxyInterface>::GetCpuStatsResponseFut {
        unimplemented!();
    }
    type GetCpuLoadResponseFut = Ready<Result<Vec<f32>, fidl::Error>>;
    fn get_cpu_load(&self, _: i64) -> <Self as StatsProxyInterface>::GetCpuLoadResponseFut {
        unimplemented!();
    }
}

async fn actual_main() {
    let stall_provider = FakeStallProvider {};
    let kernel_stats = FakeStatsProxy::new();
    let page_refaults_tracker = FakeRefaultTracker::default();
    traces::kernel::serve_forever_loop(kernel_stats, stall_provider, page_refaults_tracker).await;
}

static LOGGER_ONCE: Once = Once::new();

#[unsafe(no_mangle)]
pub extern "C" fn rs_init_logs() {
    LOGGER_ONCE.call_once(|| {
        diagnostics_log::initialize_sync(diagnostics_log::PublishOptions::default());
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_trace_two_records() {
    let mut executor = fuchsia_async::TestExecutor::new_with_fake_time();
    let start_time = executor.now();
    let mut fut = pin!(actual_main());
    assert!(
        executor.run_until_stalled(&mut fut).is_pending(),
        "Task should be waiting for the timer"
    );
    // One record has been written, the watcher is waiting for the timer.
    executor.set_fake_time(
        start_time + TRACE_SAMPLE_INTERVAL - fuchsia_async::MonotonicDuration::from_millis(10),
    );
    assert_eq!(
        false,
        executor.wake_expired_timers(),
        "Time should not wake until {}ms passed",
        TRACE_SAMPLE_INTERVAL.into_millis()
    );

    executor.set_fake_time(start_time + TRACE_SAMPLE_INTERVAL);
    assert_eq!(true, executor.wake_expired_timers(), "Time should wake now");
    assert!(
        executor.run_until_stalled(&mut fut).is_pending(),
        "Task should be waiting for the timers"
    );
    // Assertion on the traced record in ./test_runner.cc test_trace_two_records
}
#[unsafe(no_mangle)]
pub extern "C" fn rs_test_trace_no_record() {
    let mut executor = fuchsia_async::TestExecutor::new_with_fake_time();
    let start_time = executor.now();
    let mut fut = pin!(actual_main());
    assert!(
        executor.run_until_stalled(&mut fut).is_pending(),
        "Task should be waiting for the watcher"
    );

    executor.set_fake_time(start_time + fuchsia_async::MonotonicDuration::from_millis(10000));
    assert_eq!(
        false,
        executor.wake_expired_timers(),
        "No time should be involved as we are waiting on the watcher"
    );

    assert!(
        executor.run_until_stalled(&mut fut).is_pending(),
        "Task should be waiting for the watcher"
    );
    // Assertion on the traced record in ./test_runner.cc test_trace_no_record
}
