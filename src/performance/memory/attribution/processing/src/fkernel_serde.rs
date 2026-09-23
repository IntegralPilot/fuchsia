// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_kernel_common as fkernel;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(remote = "fkernel::MemoryStats")]
pub struct MemoryStatsDef {
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub wired_bytes: Option<u64>,
    pub total_heap_bytes: Option<u64>,
    pub free_heap_bytes: Option<u64>,
    pub vmo_bytes: Option<u64>,
    pub mmu_overhead_bytes: Option<u64>,
    pub ipc_bytes: Option<u64>,
    pub other_bytes: Option<u64>,
    pub free_loaned_bytes: Option<u64>,
    pub cache_bytes: Option<u64>,
    pub slab_bytes: Option<u64>,
    pub zram_bytes: Option<u64>,
    pub vmo_reclaim_total_bytes: Option<u64>,
    pub vmo_reclaim_newest_bytes: Option<u64>,
    pub vmo_reclaim_oldest_bytes: Option<u64>,
    pub vmo_reclaim_disabled_bytes: Option<u64>,
    pub vmo_discardable_locked_bytes: Option<u64>,
    pub vmo_discardable_unlocked_bytes: Option<u64>,
    #[doc(hidden)]
    #[serde(skip)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "fkernel::MemoryStatsCompression")]
pub struct MemoryStatsCompressionDef {
    pub uncompressed_storage_bytes: Option<u64>,
    pub compressed_storage_bytes: Option<u64>,
    pub compressed_fragmentation_bytes: Option<u64>,
    pub compression_time: Option<i64>,
    pub decompression_time: Option<i64>,
    pub total_page_compression_attempts: Option<u64>,
    pub failed_page_compression_attempts: Option<u64>,
    pub total_page_decompressions: Option<u64>,
    pub compressed_page_evictions: Option<u64>,
    pub eager_page_compressions: Option<u64>,
    pub memory_pressure_page_compressions: Option<u64>,
    pub critical_memory_page_compressions: Option<u64>,
    pub pages_decompressed_unit_ns: Option<u64>,
    pub pages_decompressed_within_log_time: Option<[u64; 8]>,
    #[doc(hidden)]
    #[serde(skip)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}
