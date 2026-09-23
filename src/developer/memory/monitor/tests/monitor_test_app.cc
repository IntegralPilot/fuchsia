// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>

#include "src/developer/memory/metrics/capture.h"
#include "src/developer/memory/metrics/tests/test_utils.h"
#include "src/developer/memory/monitor/memory_monitor_config.h"
#include "src/developer/memory/monitor/monitor.h"
#include "zircon/syscalls/object.h"
namespace {
std::unique_ptr<memory::MockOS> CreateMockOS() {
  const zx_handle_t handle_process_0 = 1000;
  const zx_koid_t koid_vmo_0 = 200;
  const zx_handle_t handle_process_1 = 1001;
  static const zx_info_vmo_t _vmo_0 = {
      .koid = koid_vmo_0,
      .name = "vmo_0",
      .size_bytes = 0,
  };
  static const memory::GetInfoResponse vmo_0_info = {.handle = handle_process_0,
                                                     .topic = ZX_INFO_PROCESS_VMOS,
                                                     .values = &_vmo_0,
                                                     .value_size = sizeof(_vmo_0),
                                                     .value_count = 1,
                                                     .ret = ZX_OK};

  const zx_koid_t koid_vmo_1 = 201;

  static const zx_info_vmo_t _vmo_1 = {
      .koid = koid_vmo_1,
      .name = "vmo_0",
      .size_bytes = 0,
  };

  static const zx_info_kmem_stats_t kmem_stats = {
      .total_bytes = 40,
      .free_bytes = 41,
      .free_loaned_bytes = 42,
      .wired_bytes = 43,
      .total_heap_bytes = 44,
      .free_heap_bytes = 45,
      .vmo_bytes = 46,
      .mmu_overhead_bytes = 47,
      .ipc_bytes = 48,
      .cache_bytes = 49,
      .slab_bytes = 50,
      .zram_bytes = 51,
      .other_bytes = 52,
      .vmo_reclaim_total_bytes = 53,
      .vmo_reclaim_newest_bytes = 54,
      .vmo_reclaim_oldest_bytes = 55,
      .vmo_reclaim_disabled_bytes = 56,
      .vmo_discardable_locked_bytes = 57,
      .vmo_discardable_unlocked_bytes = 58,
  };

  static const memory::GetInfoResponse stat_resp = {.handle = 1,
                                                    .topic = ZX_INFO_KMEM_STATS,
                                                    .values = &kmem_stats,
                                                    .value_size = sizeof(kmem_stats),
                                                    .value_count = 1,
                                                    .ret = ZX_OK};

  static const zx_info_kmem_stats_compression_t kmem_stats_cmp = {
      .uncompressed_storage_bytes = 60,
      .compressed_storage_bytes = 61,
      .compressed_fragmentation_bytes = 62,
      .compression_time = 63,
      .decompression_time = 64,
      .total_page_compression_attempts = 65,
      .failed_page_compression_attempts = 66,
      .total_page_decompressions = 67,
      .compressed_page_evictions = 68,
      .eager_page_compressions = 69,
      .memory_pressure_page_compressions = 70,
      .critical_memory_page_compressions = 71,
      .pages_decompressed_unit_ns = 72,
      .pages_decompressed_within_log_time = {73, 74, 75, 76, 77, 78, 79, 80},

  };
  const memory::GetInfoResponse zram_stat = {.handle = 1,
                                             .topic = ZX_INFO_KMEM_STATS_COMPRESSION,
                                             .values = &kmem_stats_cmp,
                                             .value_size = sizeof(kmem_stats_cmp),
                                             .value_count = 1,
                                             .ret = ZX_OK};

  const memory::GetInfoResponse vmo_1_info = {.handle = handle_process_1,
                                              .topic = ZX_INFO_PROCESS_VMOS,
                                              .values = &_vmo_1,
                                              .value_size = sizeof(_vmo_1),
                                              .value_count = 1,
                                              .ret = ZX_OK};
  return std::make_unique<memory::MockOS>(
      memory::OsResponses{.get_info = {vmo_0_info, vmo_1_info, zram_stat, stat_resp}});
}
}  // namespace

int main(int argc, const char** argv) {
  auto os = CreateMockOS();
  auto capture_maker = memory::CaptureMaker::Create(CreateMockOS()).value();
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  monitor::Monitor app{loop.dispatcher(),
                       // Pick non-zero arbitrary delays.
                       memory_monitor_config::Config{{.critical_capture_delay_s = 10,
                                                      .imminent_oom_capture_delay_s = 10,
                                                      .normal_capture_delay_s = 10,
                                                      .warning_capture_delay_s = 10}},
                       std::move(capture_maker)};
  loop.Run();
  return 0;
}
