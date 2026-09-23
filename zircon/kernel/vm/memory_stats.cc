// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/memory_stats.h"

#include <lib/page/size.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <cstdint>

#include <vm/compression.h>
#include <vm/discardable_vmo_tracker.h>
#include <vm/pmm.h>
#include <vm/vm.h>

#include <ktl/enforce.h>

template <>
zx_info_vmo_v1_t VmoInfoToVersion(const zx_info_vmo& vmo) {
  zx_info_vmo_v1_t vmo_v1 = {};
  vmo_v1.koid = vmo.koid;
  memcpy(vmo_v1.name, vmo.name, sizeof(vmo.name));
  vmo_v1.size_bytes = vmo.size_bytes;
  vmo_v1.parent_koid = vmo.parent_koid;
  vmo_v1.num_children = vmo.num_children;
  vmo_v1.num_mappings = vmo.num_mappings;
  vmo_v1.share_count = vmo.share_count;
  vmo_v1.flags = vmo.flags;
  vmo_v1.committed_bytes = vmo.committed_bytes;
  vmo_v1.handle_rights = vmo.handle_rights;
  vmo_v1.cache_policy = vmo.cache_policy;
  return vmo_v1;
}

template <>
zx_info_vmo_v2_t VmoInfoToVersion(const zx_info_vmo& vmo) {
  zx_info_vmo_v2_t vmo_v2 = {};
  vmo_v2.koid = vmo.koid;
  memcpy(vmo_v2.name, vmo.name, sizeof(vmo.name));
  vmo_v2.size_bytes = vmo.size_bytes;
  vmo_v2.parent_koid = vmo.parent_koid;
  vmo_v2.num_children = vmo.num_children;
  vmo_v2.num_mappings = vmo.num_mappings;
  vmo_v2.share_count = vmo.share_count;
  vmo_v2.flags = vmo.flags;
  vmo_v2.committed_bytes = vmo.committed_bytes;
  vmo_v2.handle_rights = vmo.handle_rights;
  vmo_v2.cache_policy = vmo.cache_policy;
  vmo_v2.metadata_bytes = vmo.metadata_bytes;
  vmo_v2.committed_change_events = vmo.committed_change_events;
  return vmo_v2;
}

template <>
zx_info_vmo_v3_t VmoInfoToVersion(const zx_info_vmo_t& vmo) {
  zx_info_vmo_v3_t vmo_v3 = {};
  vmo_v3.koid = vmo.koid;
  memcpy(vmo_v3.name, vmo.name, sizeof(vmo.name));
  vmo_v3.size_bytes = vmo.size_bytes;
  vmo_v3.parent_koid = vmo.parent_koid;
  vmo_v3.num_children = vmo.num_children;
  vmo_v3.num_mappings = vmo.num_mappings;
  vmo_v3.share_count = vmo.share_count;
  vmo_v3.flags = vmo.flags;
  vmo_v3.committed_bytes = vmo.committed_bytes;
  vmo_v3.handle_rights = vmo.handle_rights;
  vmo_v3.cache_policy = vmo.cache_policy;
  vmo_v3.metadata_bytes = vmo.metadata_bytes;
  vmo_v3.committed_change_events = vmo.committed_change_events;
  vmo_v3.populated_bytes = vmo.populated_bytes;
  return vmo_v3;
}

template <>
zx_info_maps_v1_t MapsInfoToVersion(const zx_info_maps_t& maps) {
  zx_info_maps_v1_t maps_v1 = {};
  memcpy(maps_v1.name, maps.name, sizeof(maps.name));
  maps_v1.base = maps.base;
  maps_v1.size = maps.size;
  maps_v1.depth = maps.depth;
  maps_v1.type = maps.type;
  maps_v1.u.mapping.mmu_flags = maps.u.mapping.mmu_flags;
  maps_v1.u.mapping.vmo_koid = maps.u.mapping.vmo_koid;
  maps_v1.u.mapping.vmo_offset = maps.u.mapping.vmo_offset;
  maps_v1.u.mapping.committed_pages = maps.u.mapping.committed_bytes >> kPageShift;
  return maps_v1;
}

template <>
zx_info_maps_v2_t MapsInfoToVersion(const zx_info_maps_t& maps) {
  zx_info_maps_v2_t maps_v2 = {};
  memcpy(maps_v2.name, maps.name, sizeof(maps.name));
  maps_v2.base = maps.base;
  maps_v2.size = maps.size;
  maps_v2.depth = maps.depth;
  maps_v2.type = maps.type;
  maps_v2.u.mapping.mmu_flags = maps.u.mapping.mmu_flags;
  maps_v2.u.mapping.vmo_koid = maps.u.mapping.vmo_koid;
  maps_v2.u.mapping.vmo_offset = maps.u.mapping.vmo_offset;
  maps_v2.u.mapping.committed_pages = maps.u.mapping.committed_bytes >> kPageShift;
  maps_v2.u.mapping.populated_pages = maps.u.mapping.populated_bytes >> kPageShift;
  return maps_v2;
}

template <>
zx_info_kmem_stats_v1 KernelStatsInfoToVersion(const zx_info_kmem_stats_t& stats) {
  zx_info_kmem_stats_v1 stats_v1 = {};
  stats_v1.total_bytes = stats.total_bytes;
  stats_v1.free_bytes = stats.free_bytes + stats.free_loaned_bytes;
  stats_v1.wired_bytes = stats.wired_bytes;
  stats_v1.total_heap_bytes = stats.total_heap_bytes;
  stats_v1.free_heap_bytes = stats.free_heap_bytes;
  stats_v1.vmo_bytes = stats.vmo_bytes;
  stats_v1.mmu_overhead_bytes = stats.mmu_overhead_bytes;
  stats_v1.ipc_bytes = stats.ipc_bytes;
  stats_v1.other_bytes = stats.other_bytes;
  return stats_v1;
}

template <>
zx_info_kmem_stats_extended_t KernelStatsInfoToVersion(const zx_info_kmem_stats_t& stats) {
  zx_info_kmem_stats_extended_t stats_ext = {};
  stats_ext.total_bytes = stats.total_bytes;
  stats_ext.free_bytes = stats.free_bytes + stats.free_loaned_bytes;
  stats_ext.wired_bytes = stats.wired_bytes;
  stats_ext.total_heap_bytes = stats.total_heap_bytes;
  stats_ext.free_heap_bytes = stats.free_heap_bytes;
  stats_ext.vmo_bytes = stats.vmo_bytes;
  stats_ext.mmu_overhead_bytes = stats.mmu_overhead_bytes;
  stats_ext.ipc_bytes = stats.ipc_bytes;
  stats_ext.other_bytes = stats.other_bytes;
  stats_ext.vmo_pager_total_bytes = stats.vmo_reclaim_total_bytes;
  stats_ext.vmo_pager_newest_bytes = stats.vmo_reclaim_newest_bytes;
  stats_ext.vmo_pager_oldest_bytes = stats.vmo_reclaim_oldest_bytes;
  stats_ext.vmo_discardable_locked_bytes = stats.vmo_discardable_locked_bytes;
  stats_ext.vmo_discardable_unlocked_bytes = stats.vmo_discardable_unlocked_bytes;
  stats_ext.vmo_reclaim_disabled_bytes = stats.vmo_reclaim_disabled_bytes;
  return stats_ext;
}

// TODO: figure out a better handle to hang this off to and push this copy code into
// that dispatcher.
zx_info_kmem_stats_t GetMemoryStats() {
  // |get_count| returns an estimate so the sum of the counts may not equal the total.
  uint64_t state_count[VmPageStateIndex(vm_page_state::COUNT_)] = {};
  for (uint32_t i = 0; i < VmPageStateIndex(vm_page_state::COUNT_); i++) {
    state_count[i] = vm_page_t::get_count(vm_page_state(i));
  }

  uint64_t free_heap_bytes = 0;
  heap_get_info(nullptr, &free_heap_bytes);

  // Note that this intentionally uses uint64_t instead of
  // size_t in case we ever have a 32-bit userspace but more
  // than 4GB physical memory.
  zx_info_kmem_stats_t stats = {};
  stats.total_bytes = pmm_count_total_bytes();

  // Holds the sum of bytes in the broken out states. This sum could be less than the total
  // because we aren't counting all possible states (e.g. vm_page_state::ALLOC). This sum could
  // be greater than the total because per-state counts are approximate.
  uint64_t sum_bytes = 0;

  stats.free_bytes = state_count[VmPageStateIndex(vm_page_state::FREE)] * kPageSize;
  sum_bytes += stats.free_bytes;

  stats.free_loaned_bytes = state_count[VmPageStateIndex(vm_page_state::FREE_LOANED)] * kPageSize;
  sum_bytes += stats.free_loaned_bytes;

  stats.wired_bytes = state_count[VmPageStateIndex(vm_page_state::WIRED)] * kPageSize;
  sum_bytes += stats.wired_bytes;

  stats.total_heap_bytes = state_count[VmPageStateIndex(vm_page_state::HEAP)] * kPageSize;
  sum_bytes += stats.total_heap_bytes;
  stats.free_heap_bytes = free_heap_bytes;

  stats.vmo_bytes = state_count[VmPageStateIndex(vm_page_state::OBJECT)] * kPageSize;
  sum_bytes += stats.vmo_bytes;

  stats.mmu_overhead_bytes = (state_count[VmPageStateIndex(vm_page_state::MMU)] +
                              state_count[VmPageStateIndex(vm_page_state::IOMMU)]) *
                             kPageSize;
  sum_bytes += stats.mmu_overhead_bytes;

  stats.ipc_bytes = state_count[VmPageStateIndex(vm_page_state::IPC)] * kPageSize;
  sum_bytes += stats.ipc_bytes;

  stats.cache_bytes = state_count[VmPageStateIndex(vm_page_state::CACHE)] * kPageSize;
  sum_bytes += state_count[VmPageStateIndex(vm_page_state::CACHE)] * kPageSize;

  stats.slab_bytes = state_count[VmPageStateIndex(vm_page_state::SLAB)] * kPageSize;
  sum_bytes += state_count[VmPageStateIndex(vm_page_state::SLAB)] * kPageSize;

  stats.zram_bytes = state_count[VmPageStateIndex(vm_page_state::ZRAM)] * kPageSize;
  sum_bytes += state_count[VmPageStateIndex(vm_page_state::ZRAM)] * kPageSize;

  // Is there unaccounted memory?
  if (stats.total_bytes > sum_bytes) {
    // Everything else gets counted as "other".
    stats.other_bytes = stats.total_bytes - sum_bytes;
  } else {
    // One or more of our per-state counts may have been off. We'll ignore it.
    stats.other_bytes = 0;
  }

  PageQueues::ReclaimCounts reclaim_counts = pmm_page_queues()->GetReclaimQueueCounts();
  PageQueues::Counts queue_counts = pmm_page_queues()->QueueCounts();

  stats.vmo_reclaim_total_bytes = reclaim_counts.total * kPageSize;
  stats.vmo_reclaim_newest_bytes = reclaim_counts.newest * kPageSize;
  stats.vmo_reclaim_oldest_bytes = reclaim_counts.oldest * kPageSize;
  stats.vmo_reclaim_disabled_bytes = queue_counts.high_priority * kPageSize;

  DiscardableVmoTracker::DiscardablePageCounts discardable_counts =
      DiscardableVmoTracker::DebugDiscardablePageCounts();

  stats.vmo_discardable_locked_bytes = discardable_counts.locked * kPageSize;
  stats.vmo_discardable_unlocked_bytes = discardable_counts.unlocked * kPageSize;

  return stats;
}

zx_info_kmem_stats_compression_t GetCompressionStats() {
  VmCompression* compression = Pmm::Node().GetPageCompression();
  if (compression == nullptr) {
    return {};
  }
  zx_info_kmem_stats_compression_t kstats = {};
  VmCompression::Stats stats = compression->GetStats();
  kstats.uncompressed_storage_bytes = stats.memory_usage.uncompressed_content_bytes;
  kstats.compressed_storage_bytes = stats.memory_usage.compressed_storage_bytes;
  kstats.compressed_fragmentation_bytes = stats.memory_usage.compressed_storage_bytes -
                                          stats.memory_usage.compressed_storage_used_bytes;
  kstats.compression_time = stats.compression_time;
  kstats.decompression_time = stats.decompression_time;
  kstats.total_page_compression_attempts = stats.total_page_compression_attempts;
  kstats.failed_page_compression_attempts = stats.failed_page_compression_attempts;
  kstats.total_page_decompressions = stats.total_page_decompressions;
  kstats.compressed_page_evictions = stats.compressed_page_evictions;
  kstats.eager_page_compressions = PageQueues::GetLruPagesCompressed();
  Evictor::EvictorStats evictor_stats = Evictor::GetGlobalStats();
  kstats.memory_pressure_page_compressions = evictor_stats.compression_other;
  kstats.critical_memory_page_compressions = evictor_stats.compression_oom;
  kstats.pages_decompressed_unit_ns = ZX_SEC(1);
  static_assert(8 <= VmCompression::kNumLogBuckets);
  for (int i = 0; i < 8; i++) {
    kstats.pages_decompressed_within_log_time[i] = stats.pages_decompressed_within_log_seconds[i];
  }
  return kstats;
}

zx_info_memory_stall_t GetStallStats() {
  StallAggregator::Stats stats = StallAggregator::GetStallAggregator()->ReadStats();
  zx_info_memory_stall_t info = {
      .stall_time_some = stats.stalled_time_some,
      .stall_time_full = stats.stalled_time_full,
  };
  return info;
}
