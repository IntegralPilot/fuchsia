// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fdomain_fuchsia_memory_attribution_plugin as fplugin;
use serde::Serialize;

#[derive(Serialize)]
pub struct CommandMemoryStatistics {
    pub timestamp: String,
    pub boot_time: i64,
    pub monotonic_time: i64,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub wired_bytes: u64,
    pub total_heap_bytes: u64,
    pub free_heap_bytes: u64,
    pub vmo_bytes: u64,
    pub mmu_overhead_bytes: u64,
    pub ipc_bytes: u64,
    pub other_bytes: u64,
    pub free_loaned_bytes: u64,
    pub cache_bytes: u64,
    pub slab_bytes: u64,
    pub zram_bytes: u64,
    pub vmo_reclaim_total_bytes: u64,
    pub vmo_reclaim_newest_bytes: u64,
    pub vmo_reclaim_oldest_bytes: u64,
    pub vmo_reclaim_disabled_bytes: u64,
    pub vmo_discardable_locked_bytes: u64,
    pub vmo_discardable_unlocked_bytes: u64,

    pub uncompressed_storage_bytes: u64,
    pub compressed_storage_bytes: u64,
    pub compressed_fragmentation_bytes: u64,
    pub compression_time: i64,
    pub decompression_time: i64,
    pub total_page_compression_attempts: u64,
    pub failed_page_compression_attempts: u64,
    pub total_page_decompressions: u64,
    pub compressed_page_evictions: u64,
    pub eager_page_compressions: u64,
    pub memory_pressure_page_compressions: u64,
    pub critical_memory_page_compressions: u64,
    pub pages_decompressed_unit_ns: u64,
    pub some_memory_stalls_ns: i64,
    pub full_memory_stalls_ns: i64,
    pub page_refaults: u64,
}

impl TryFrom<fplugin::MemoryStatistics> for CommandMemoryStatistics {
    type Error = anyhow::Error;

    fn try_from(value: fplugin::MemoryStatistics) -> Result<Self, Self::Error> {
        let result = Self::from_memory_stats(&value);
        result.context("Failed to convert MemoryStatistics!")
    }
}

impl CommandMemoryStatistics {
    #[allow(clippy::or_fun_call)]
    fn from_memory_stats(value: &fplugin::MemoryStatistics) -> Result<Self> {
        let time = value.time.as_ref().ok_or(anyhow::anyhow!("no time"))?;
        let memory_stats = &value
            .kernel_statistics
            .as_ref()
            .ok_or(anyhow::anyhow!("no kernel_stats"))?
            .memory_stats
            .as_ref()
            .ok_or(anyhow::anyhow!("no memory_stats"))?;
        let compression_stats = &value
            .kernel_statistics
            .as_ref()
            .ok_or(anyhow::anyhow!("no kernel_stats"))?
            .compression_stats
            .as_ref()
            .ok_or(anyhow::anyhow!("no compression_stats"))?;
        Ok(Self {
            timestamp: chrono::Utc::now().to_rfc3339(),
            boot_time: time.boot_time.ok_or(anyhow::anyhow!("no boot_time"))?.into_nanos(),
            monotonic_time: time
                .monotonic_time
                .ok_or(anyhow::anyhow!("no monotonic_time"))?
                .into_nanos(),
            total_bytes: memory_stats.total_bytes.ok_or(anyhow::anyhow!("no stats value"))?,
            free_bytes: memory_stats.free_bytes.ok_or(anyhow::anyhow!("no stats value"))?,
            wired_bytes: memory_stats.wired_bytes.ok_or(anyhow::anyhow!("no stats value"))?,
            total_heap_bytes: memory_stats
                .total_heap_bytes
                .ok_or(anyhow::anyhow!("no stats value"))?,
            free_heap_bytes: memory_stats
                .free_heap_bytes
                .ok_or(anyhow::anyhow!("no stats value"))?,
            vmo_bytes: memory_stats.vmo_bytes.ok_or(anyhow::anyhow!("no stats value"))?,
            mmu_overhead_bytes: memory_stats
                .mmu_overhead_bytes
                .ok_or(anyhow::anyhow!("no stats value"))?,
            ipc_bytes: memory_stats.ipc_bytes.ok_or(anyhow::anyhow!("no stats value"))?,
            other_bytes: memory_stats.other_bytes.ok_or(anyhow::anyhow!("no stats value"))?,
            free_loaned_bytes: memory_stats
                .free_loaned_bytes
                .ok_or(anyhow::anyhow!("no stats value"))?,
            cache_bytes: memory_stats.cache_bytes.ok_or(anyhow::anyhow!("no stats value"))?,
            slab_bytes: memory_stats.slab_bytes.ok_or(anyhow::anyhow!("no stats value"))?,
            zram_bytes: memory_stats.zram_bytes.ok_or(anyhow::anyhow!("no stats value"))?,
            vmo_reclaim_total_bytes: memory_stats
                .vmo_reclaim_total_bytes
                .ok_or(anyhow::anyhow!("no stats value"))?,
            vmo_reclaim_newest_bytes: memory_stats
                .vmo_reclaim_newest_bytes
                .ok_or(anyhow::anyhow!("no stats value"))?,
            vmo_reclaim_oldest_bytes: memory_stats
                .vmo_reclaim_oldest_bytes
                .ok_or(anyhow::anyhow!("no stats value"))?,
            vmo_reclaim_disabled_bytes: memory_stats
                .vmo_reclaim_disabled_bytes
                .ok_or(anyhow::anyhow!("no stats value"))?,
            vmo_discardable_locked_bytes: memory_stats
                .vmo_discardable_locked_bytes
                .ok_or(anyhow::anyhow!("no stats value"))?,
            vmo_discardable_unlocked_bytes: memory_stats
                .vmo_discardable_unlocked_bytes
                .ok_or(anyhow::anyhow!("no stats value"))?,
            uncompressed_storage_bytes: compression_stats
                .uncompressed_storage_bytes
                .ok_or(anyhow::anyhow!("no stats value"))?,
            compressed_storage_bytes: compression_stats
                .compressed_storage_bytes
                .ok_or(anyhow::anyhow!("no stats value"))?,
            compressed_fragmentation_bytes: compression_stats
                .compressed_fragmentation_bytes
                .ok_or(anyhow::anyhow!("no stats value"))?,
            compression_time: compression_stats
                .compression_time
                .ok_or(anyhow::anyhow!("no stats value"))?,
            decompression_time: compression_stats
                .decompression_time
                .ok_or(anyhow::anyhow!("no stats value"))?,
            total_page_compression_attempts: compression_stats
                .total_page_compression_attempts
                .ok_or(anyhow::anyhow!("no stats value"))?,
            failed_page_compression_attempts: compression_stats
                .failed_page_compression_attempts
                .ok_or(anyhow::anyhow!("no stats value"))?,
            total_page_decompressions: compression_stats
                .total_page_decompressions
                .ok_or(anyhow::anyhow!("no stats value"))?,
            compressed_page_evictions: compression_stats
                .compressed_page_evictions
                .ok_or(anyhow::anyhow!("no stats value"))?,
            eager_page_compressions: compression_stats
                .eager_page_compressions
                .ok_or(anyhow::anyhow!("no stats value"))?,
            memory_pressure_page_compressions: compression_stats
                .memory_pressure_page_compressions
                .ok_or(anyhow::anyhow!("no stats value"))?,
            critical_memory_page_compressions: compression_stats
                .critical_memory_page_compressions
                .ok_or(anyhow::anyhow!("no stats value"))?,
            pages_decompressed_unit_ns: compression_stats
                .pages_decompressed_unit_ns
                .ok_or(anyhow::anyhow!("no stats value"))?,
            some_memory_stalls_ns: value
                .performance_metrics
                .as_ref()
                .ok_or(anyhow::anyhow!("no performance_metrics"))?
                .some_memory_stalls_ns
                .ok_or(anyhow::anyhow!("no some_memory_stalls"))?,
            full_memory_stalls_ns: value
                .performance_metrics
                .as_ref()
                .ok_or(anyhow::anyhow!("no performance_metrics"))?
                .full_memory_stalls_ns
                .ok_or(anyhow::anyhow!("no full_memory_stalls"))?,
            page_refaults: value
                .performance_metrics
                .as_ref()
                .ok_or(anyhow::anyhow!("no performance_metrics"))?
                .page_refaults
                .ok_or(anyhow::anyhow!("no page_refaults"))?,
        })
    }
}
