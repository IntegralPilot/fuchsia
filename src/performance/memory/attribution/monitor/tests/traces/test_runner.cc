// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/trace/event.h>
#include <lib/trace/event_args.h>

#include <gtest/gtest.h>
#include <trace-test-utils/fixture.h>

#include "lib.h"

#define DEFAULT_BUFFER_SIZE_BYTES (1024ul * 1024ul)

// This is inspired by trace-test-utils/fixture_macros.h
// which cannot be used with gtest.
// TODO(b/377646085): Remove macros when trace-test-utils/fixture_macros.h gets compatible.
#define BEGIN_TRACE_TEST_ETC(attach_to_thread, mode, buffer_size, categories) \
  __attribute__((cleanup(fixture_scope_cleanup))) bool __scope = 0;           \
  (void)__scope;                                                              \
  fixture_set_up_with_categories((attach_to_thread), (mode), (buffer_size), categories)

#define BEGIN_TRACE_TEST_WITH_CATEGORIES(categories)                                             \
  BEGIN_TRACE_TEST_ETC(kAttachToThread, TRACE_BUFFERING_MODE_ONESHOT, DEFAULT_BUFFER_SIZE_BYTES, \
                       categories)

#define ASSERT_RECORDS(expected) \
  ASSERT_TRUE(fixture_compare_records(expected)) << "Record mismatch";

class MemoryMonitorTracesTestSuite : public ::testing::Test {
 protected:
  static void SetUpTestSuite() { rs_init_logs(); }
};

TEST_F(MemoryMonitorTracesTestSuite, TestTraceTwoRecords) {
  BEGIN_TRACE_TEST_WITH_CATEGORIES({"memory:kernel"});

  fixture_initialize_and_start_tracing();

  rs_test_trace_two_records();

  ASSERT_RECORDS(
      R"(String(index: 1, "memory:kernel")
String(index: 2, "total_bytes")
String(index: 3, "free_bytes")
String(index: 4, "free_loaned_bytes")
String(index: 5, "wired_bytes")
String(index: 6, "total_heap_bytes")
String(index: 7, "free_heap_bytes")
String(index: 8, "vmo_bytes")
String(index: 9, "mmu_overhead_bytes")
String(index: 10, "ipc_bytes")
String(index: 11, "cache_bytes")
String(index: 12, "slab_bytes")
String(index: 13, "zram_bytes")
String(index: 14, "other_bytes")
String(index: 15, "kmem_stats_a")
String(index: 16, "process")
KernelObject(koid: <>, type: thread, name: "initial-thread", {process: koid(<>)})
Thread(index: 1, <>)
Event(ts: <>, pt: <>, category: "memory:kernel", name: "kmem_stats_a", Counter(id: 0), {total_bytes: uint64(1), free_bytes: uint64(2), free_loaned_bytes: uint64(10), wired_bytes: uint64(3), total_heap_bytes: uint64(4), free_heap_bytes: uint64(5), vmo_bytes: uint64(6), mmu_overhead_bytes: uint64(7), ipc_bytes: uint64(8), cache_bytes: uint64(11), slab_bytes: uint64(12), zram_bytes: uint64(13), other_bytes: uint64(9)})
String(index: 17, "vmo_reclaim_total_bytes")
String(index: 18, "vmo_reclaim_newest_bytes")
String(index: 19, "vmo_reclaim_oldest_bytes")
String(index: 20, "vmo_reclaim_disabled_bytes")
String(index: 21, "vmo_discardable_locked_bytes")
String(index: 22, "vmo_discardable_unlocked_bytes")
String(index: 23, "kmem_stats_b")
Event(ts: <>, pt: <>, category: "memory:kernel", name: "kmem_stats_b", Counter(id: 0), {vmo_reclaim_total_bytes: uint64(14), vmo_reclaim_newest_bytes: uint64(15), vmo_reclaim_oldest_bytes: uint64(16), vmo_reclaim_disabled_bytes: uint64(17), vmo_discardable_locked_bytes: uint64(18), vmo_discardable_unlocked_bytes: uint64(19)})
String(index: 24, "uncompressed_storage_bytes")
String(index: 25, "compressed_storage_bytes")
String(index: 26, "compressed_fragmentation_bytes")
String(index: 27, "compression_time")
String(index: 28, "decompression_time")
String(index: 29, "total_page_compression_attempts")
String(index: 30, "failed_page_compression_attempts")
String(index: 31, "total_page_decompressions")
String(index: 32, "compressed_page_evictions")
String(index: 33, "eager_page_compressions")
String(index: 34, "memory_pressure_page_compressions")
String(index: 35, "critical_memory_page_compressions")
String(index: 36, "kmem_stats_compression")
Event(ts: <>, pt: <>, category: "memory:kernel", name: "kmem_stats_compression", Counter(id: 0), {uncompressed_storage_bytes: uint64(1100), compressed_storage_bytes: uint64(1101), compressed_fragmentation_bytes: uint64(1102), compression_time: int64(1103), decompression_time: int64(1104), total_page_compression_attempts: uint64(1105), failed_page_compression_attempts: uint64(1106), total_page_decompressions: uint64(1107), compressed_page_evictions: uint64(1108), eager_page_compressions: uint64(1109), memory_pressure_page_compressions: uint64(1110), critical_memory_page_compressions: uint64(1111)})
String(index: 37, "pages_decompressed_unit_ns")
String(index: 38, "pages_decompressed_within_log_time[0]")
String(index: 39, "pages_decompressed_within_log_time[1]")
String(index: 40, "pages_decompressed_within_log_time[2]")
String(index: 41, "pages_decompressed_within_log_time[3]")
String(index: 42, "pages_decompressed_within_log_time[4]")
String(index: 43, "pages_decompressed_within_log_time[5]")
String(index: 44, "pages_decompressed_within_log_time[6]")
String(index: 45, "pages_decompressed_within_log_time[7]")
String(index: 46, "kmem_stats_compression_time")
Event(ts: <>, pt: <>, category: "memory:kernel", name: "kmem_stats_compression_time", Counter(id: 0), {pages_decompressed_unit_ns: uint64(1112), pages_decompressed_within_log_time[0]: uint64(1113), pages_decompressed_within_log_time[1]: uint64(114), pages_decompressed_within_log_time[2]: uint64(115), pages_decompressed_within_log_time[3]: uint64(116), pages_decompressed_within_log_time[4]: uint64(117), pages_decompressed_within_log_time[5]: uint64(118), pages_decompressed_within_log_time[6]: uint64(119), pages_decompressed_within_log_time[7]: uint64(120)})
String(index: 47, "stall_time_some_ns")
String(index: 48, "stall_time_full_ns")
String(index: 49, "page_refaults")
String(index: 50, "memory_stall")
Event(ts: <>, pt: <>, category: "memory:kernel", name: "memory_stall", Counter(id: 0), {stall_time_some_ns: uint64(1), stall_time_full_ns: uint64(2), page_refaults: uint64(128)})
Event(ts: <>, pt: <>, category: "memory:kernel", name: "kmem_stats_a", Counter(id: 0), {total_bytes: uint64(2001), free_bytes: uint64(2002), free_loaned_bytes: uint64(2010), wired_bytes: uint64(2003), total_heap_bytes: uint64(2004), free_heap_bytes: uint64(2005), vmo_bytes: uint64(2006), mmu_overhead_bytes: uint64(2007), ipc_bytes: uint64(2008), cache_bytes: uint64(2011), slab_bytes: uint64(2012), zram_bytes: uint64(2013), other_bytes: uint64(2009)})
Event(ts: <>, pt: <>, category: "memory:kernel", name: "kmem_stats_b", Counter(id: 0), {vmo_reclaim_total_bytes: uint64(2014), vmo_reclaim_newest_bytes: uint64(2015), vmo_reclaim_oldest_bytes: uint64(2016), vmo_reclaim_disabled_bytes: uint64(2017), vmo_discardable_locked_bytes: uint64(2018), vmo_discardable_unlocked_bytes: uint64(2019)})
Event(ts: <>, pt: <>, category: "memory:kernel", name: "kmem_stats_compression", Counter(id: 0), {uncompressed_storage_bytes: uint64(3100), compressed_storage_bytes: uint64(3101), compressed_fragmentation_bytes: uint64(3102), compression_time: int64(3103), decompression_time: int64(3104), total_page_compression_attempts: uint64(3105), failed_page_compression_attempts: uint64(3106), total_page_decompressions: uint64(3107), compressed_page_evictions: uint64(3108), eager_page_compressions: uint64(3109), memory_pressure_page_compressions: uint64(3110), critical_memory_page_compressions: uint64(3111)})
Event(ts: <>, pt: <>, category: "memory:kernel", name: "kmem_stats_compression_time", Counter(id: 0), {pages_decompressed_unit_ns: uint64(3112), pages_decompressed_within_log_time[0]: uint64(3113), pages_decompressed_within_log_time[1]: uint64(114), pages_decompressed_within_log_time[2]: uint64(115), pages_decompressed_within_log_time[3]: uint64(116), pages_decompressed_within_log_time[4]: uint64(117), pages_decompressed_within_log_time[5]: uint64(118), pages_decompressed_within_log_time[6]: uint64(119), pages_decompressed_within_log_time[7]: uint64(120)})
Event(ts: <>, pt: <>, category: "memory:kernel", name: "memory_stall", Counter(id: 0), {stall_time_some_ns: uint64(1), stall_time_full_ns: uint64(2), page_refaults: uint64(128)})
)");
}

TEST_F(MemoryMonitorTracesTestSuite, TestTraceNoRecord) {
  BEGIN_TRACE_TEST_WITH_CATEGORIES({});

  fixture_initialize_and_start_tracing();

  rs_test_trace_no_record();

  ASSERT_RECORDS(R"()");
}
