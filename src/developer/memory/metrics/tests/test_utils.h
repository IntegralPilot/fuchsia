// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_MEMORY_METRICS_TESTS_TEST_UTILS_H_
#define SRC_DEVELOPER_MEMORY_METRICS_TESTS_TEST_UTILS_H_

#include <zircon/types.h>

#include <cstdint>
#include <vector>

#include "src/developer/memory/metrics/capture.h"
#include "src/developer/memory/metrics/summary.h"

namespace memory {

struct CaptureTemplate {
  zx_instant_boot_t time;
  zx_info_kmem_stats_t kmem = {};
  zx_info_kmem_stats_extended_t kmem_extended = {};
  std::vector<zx_info_vmo_t> vmos;
  std::vector<Process> processes;
  std::unordered_set<std::string> rooted_vmo_names;
};

struct GetProcessesCallback {
  int depth;
  zx_handle_t handle;
  zx_koid_t koid;
  zx_koid_t parent_koid;
};

struct GetProcessesResponse {
  zx_status_t ret;
  std::vector<GetProcessesCallback> callbacks;
};

struct GetPropertyResponse {
  zx_handle_t handle;
  uint32_t property;
  const void* value;
  size_t value_len;
  zx_status_t ret;
};

struct GetInfoResponse {
  zx_handle_t handle;
  uint32_t topic;
  const void* values;
  size_t value_size;
  size_t value_count;
  zx_status_t ret;
};

struct OsResponses {
  const GetProcessesResponse get_processes;
  const std::vector<GetPropertyResponse> get_property;
  const std::vector<GetInfoResponse> get_info;
};

class CaptureSupplier {
 public:
  explicit CaptureSupplier(std::vector<CaptureTemplate> templates)
      : templates_(std::move(templates)) {}

  zx_status_t GetCapture(Capture* capture, CaptureLevel level,
                         bool use_capture_supplier_time = false);
  bool empty() const { return index_ == templates_.size(); }

 private:
  std::vector<CaptureTemplate> templates_;
  size_t index_ = 0;
};

class MockOS : public OS {
 public:
  explicit MockOS(OsResponses responses);

  zx_status_t GetKernelStats(fidl::WireSyncClient<fuchsia_kernel::Stats>* stats) override;

  zx_handle_t ProcessSelf() override;

  zx_instant_boot_t GetBoot() override;

  zx_status_t GetProcesses(
      fit::function<zx_status_t(int, zx::handle, zx_koid_t, zx_koid_t)> cb) override;

  zx_status_t GetProperty(zx_handle_t handle, uint32_t property, void* value,
                          size_t name_len) override;

  const GetInfoResponse* GetGetInfoResponse(zx_handle_t handle, uint32_t topic);

  zx_status_t GetInfo(zx_handle_t handle, uint32_t topic, void* buffer, size_t buffer_size,
                      size_t* actual, size_t* avail) override;

  zx_status_t GetKernelMemoryStats(const fidl::WireSyncClient<fuchsia_kernel::Stats>& stats_client,
                                   zx_info_kmem_stats_t& kmem) override;

  zx_status_t GetKernelMemoryStatsExtended(
      const fidl::WireSyncClient<fuchsia_kernel::Stats>& stats_client,
      zx_info_kmem_stats_extended_t& kmem_ext, zx_info_kmem_stats_t* kmem) override;
  zx_status_t GetKernelMemoryStatsCompression(
      const fidl::WireSyncClient<fuchsia_kernel::Stats>& stats_client,
      zx_info_kmem_stats_compression_t& kmem_compression) override;

 private:
  OsResponses responses_;
  uint32_t i_get_property_;
  zx_instant_boot_t clock_;
};

class TestUtils {
 public:
  const static zx_handle_t kRootHandle;
  const static zx_handle_t kSelfHandle;
  const static zx_koid_t kSelfKoid;

  static void CreateCapture(Capture* capture, const CaptureTemplate& t,
                            CaptureLevel level = CaptureLevel::VMO);
  static zx_status_t GetCapture(Capture* capture, CaptureLevel level, const OsResponses& r);

  // Sorted by koid.
  static std::vector<ProcessSummary> GetProcessSummaries(const Summary& summary);
};

}  // namespace memory

#endif  // SRC_DEVELOPER_MEMORY_METRICS_TESTS_TEST_UTILS_H_
