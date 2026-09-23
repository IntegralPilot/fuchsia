// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/metrics/tests/test_utils.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>
#include <zircon/syscalls/object.h>

#include <algorithm>
#include <memory>

#include <gtest/gtest.h>

#include "src/developer/memory/metrics/capture.h"
namespace memory {
namespace {
zx_info_kmem_stats_extended_t ExtendedStats(const zx_info_kmem_stats_t& stats) {
  return zx_info_kmem_stats_extended_t{
      .total_bytes = stats.total_bytes,
      .free_bytes = stats.free_bytes,
      .wired_bytes = stats.wired_bytes,
      .total_heap_bytes = stats.total_heap_bytes,
      .free_heap_bytes = stats.free_heap_bytes,
      .vmo_bytes = stats.vmo_bytes,
      .vmo_pager_total_bytes = stats.vmo_reclaim_total_bytes,
      .vmo_pager_newest_bytes = stats.vmo_reclaim_newest_bytes,
      .vmo_pager_oldest_bytes = stats.vmo_reclaim_oldest_bytes,
      .vmo_discardable_locked_bytes = stats.vmo_discardable_locked_bytes,
      .vmo_discardable_unlocked_bytes = stats.vmo_discardable_unlocked_bytes,
      .mmu_overhead_bytes = stats.mmu_overhead_bytes,
      .ipc_bytes = stats.ipc_bytes,
      .other_bytes = stats.other_bytes,
      .vmo_reclaim_disabled_bytes = stats.vmo_reclaim_disabled_bytes,
  };
}
}  // namespace

const zx_handle_t TestUtils::kRootHandle = 1;
const zx_handle_t TestUtils::kSelfHandle = 2;
const zx_koid_t TestUtils::kSelfKoid = 3;

MockOS::MockOS(OsResponses responses)
    : responses_(std::move(responses)), i_get_property_(0), clock_(0) {}

zx_status_t MockOS::GetKernelStats(fidl::WireSyncClient<fuchsia_kernel::Stats>* stats) {
  return ZX_OK;
}

zx_handle_t MockOS::ProcessSelf() { return TestUtils::kSelfHandle; }

zx_instant_boot_t MockOS::GetBoot() { return clock_; }

zx_status_t MockOS::GetProcesses(
    fit::function<zx_status_t(int, zx::handle, zx_koid_t, zx_koid_t)> cb) {
  const auto& r = responses_.get_processes;
  for (const auto& c : r.callbacks) {
    auto ret = cb(c.depth, zx::handle(c.handle), c.koid, c.parent_koid);
    if (ret != ZX_OK) {
      return ret;
    }
  }
  return r.ret;
}

zx_status_t MockOS::GetProperty(zx_handle_t handle, uint32_t property, void* value,
                                size_t name_len) {
  const auto& r = responses_.get_property.at(i_get_property_++);
  EXPECT_EQ(r.handle, handle);
  EXPECT_EQ(r.property, property);
  auto len = std::min(name_len, r.value_len);
  memcpy(value, r.value, len);
  return r.ret;
}

const GetInfoResponse* MockOS::GetGetInfoResponse(zx_handle_t handle, uint32_t topic) {
  for (const auto& resp : responses_.get_info) {
    if (resp.handle == handle && resp.topic == topic) {
      return &resp;
    }
  }
  FX_LOGS(ERROR) << "This should not be reached: handle " << handle << " topic " << topic;
  EXPECT_TRUE(false) << "This should not be reached: handle " << handle << " topic " << topic;
  return nullptr;
}

zx_status_t MockOS::GetInfo(zx_handle_t handle, uint32_t topic, void* buffer, size_t buffer_size,
                            size_t* actual, size_t* avail) {
  const GetInfoResponse* r = GetGetInfoResponse(handle, topic);
  if (r == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  EXPECT_EQ(r->handle, handle);
  EXPECT_EQ(r->topic, topic);
  size_t num_copied = 0;
  if (buffer != nullptr) {
    num_copied = std::min(r->value_count, buffer_size / r->value_size);
    memcpy(buffer, r->values, num_copied * r->value_size);
  }
  if (actual != nullptr) {
    *actual = num_copied;
  }
  // avail is the number of total available elements that can be read.
  if (avail != nullptr) {
    *avail = r->value_count;
  }
  return r->ret;
}

zx_status_t MockOS::GetKernelMemoryStats(
    const fidl::WireSyncClient<fuchsia_kernel::Stats>& stats_client, zx_info_kmem_stats_t& kmem) {
  const GetInfoResponse* r = GetGetInfoResponse(TestUtils::kRootHandle, ZX_INFO_KMEM_STATS);
  if (r == nullptr)
    return ZX_ERR_INVALID_ARGS;
  memcpy(&kmem, r->values, r->value_size);
  return r->ret;
}

zx_status_t MockOS::GetKernelMemoryStatsExtended(
    const fidl::WireSyncClient<fuchsia_kernel::Stats>& stats_client,
    zx_info_kmem_stats_extended_t& kmem_ext, zx_info_kmem_stats_t* kmem) {
  const GetInfoResponse* r1 = GetGetInfoResponse(TestUtils::kRootHandle, ZX_INFO_KMEM_STATS);
  if (r1 == nullptr)
    return ZX_ERR_INVALID_ARGS;
  memcpy(kmem, r1->values, r1->value_size);
  const GetInfoResponse* r2 = GetGetInfoResponse(TestUtils::kRootHandle, ZX_INFO_KMEM_STATS);
  if (r2 == nullptr)
    return ZX_ERR_INVALID_ARGS;
  zx_info_kmem_stats_t stats;
  memcpy(&stats, r2->values, r2->value_size);
  kmem_ext = ExtendedStats(stats);
  return r2->ret;
}

zx_status_t MockOS::GetKernelMemoryStatsCompression(
    const fidl::WireSyncClient<fuchsia_kernel::Stats>& stats_client,
    zx_info_kmem_stats_compression_t& kmem_compression) {
  const GetInfoResponse* r =
      GetGetInfoResponse(TestUtils::kRootHandle, ZX_INFO_KMEM_STATS_COMPRESSION);
  if (r == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  memcpy(&kmem_compression, r->values, r->value_size);
  return r->ret;
}

void TestUtils::CreateCapture(Capture* capture, const CaptureTemplate& t, CaptureLevel level) {
  capture->time_ = t.time;
  capture->kmem_ = t.kmem;
  if (level == CaptureLevel::KMEM) {
    return;
  }
  capture->kmem_extended_ = t.kmem_extended;
  if (level != CaptureLevel::VMO) {
    return;
  }
  for (const auto& vmo : t.vmos) {
    capture->koid_to_vmo_.emplace(vmo.koid, vmo);
  }
  for (const auto& process : t.processes) {
    capture->koid_to_process_.emplace(process.koid, process);
  }
  CaptureMaker::ReallocateDescendants(t.rooted_vmo_names, capture->koid_to_vmo_);
}

std::vector<ProcessSummary> TestUtils::GetProcessSummaries(const Summary& summary) {
  std::vector<ProcessSummary> summaries = summary.process_summaries();
  std::ranges::sort(summaries, [](const ProcessSummary& a, const ProcessSummary& b) {
    return a.koid() < b.koid();
  });
  return summaries;
}

zx_status_t TestUtils::GetCapture(Capture* capture, CaptureLevel level, const OsResponses& r) {
  CaptureMaker capture_maker({}, std::make_unique<MockOS>(r));
  return capture_maker.GetCapture(capture, level, Capture::kDefaultRootedVmoNames);
}

zx_status_t CaptureSupplier::GetCapture(Capture* capture, CaptureLevel level,
                                        bool use_capture_supplier_time) {
  auto& t = templates_.at(index_);
  if (!use_capture_supplier_time) {
    t.time = static_cast<int64_t>(index_);
  }
  index_++;
  TestUtils::CreateCapture(capture, t, level);
  return ZX_OK;
}

}  // namespace memory
