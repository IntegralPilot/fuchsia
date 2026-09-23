// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_CPU_BIN_CPUCTL_PERFORMANCE_DOMAIN_H_
#define SRC_DEVICES_CPU_BIN_CPUCTL_PERFORMANCE_DOMAIN_H_

#include <fidl/fuchsia.hardware.cpu.ctrl/cpp/wire.h>

#include <tuple>
#include <utility>
#include <vector>

namespace cpuctrl = fuchsia_hardware_cpu_ctrl;

class CpuPerformanceDomain {
 public:
  static zx::result<CpuPerformanceDomain> CreateFromPath(const std::string& path);
  std::pair<zx_status_t, uint32_t> GetOperatingPointCount();
  std::pair<zx_status_t, uint64_t> GetNumLogicalCores();
  std::pair<zx_status_t, uint64_t> GetDomainId();
  std::pair<zx_status_t, uint64_t> GetRelativePerformance();
  std::pair<zx_status_t, uint64_t> GetRelativePerformance2();
  std::tuple<zx_status_t, uint64_t, cpuctrl::wire::CpuOperatingPointInfo>
  GetCurrentOperatingPoint();
  std::tuple<zx_status_t, const std::vector<cpuctrl::wire::CpuOperatingPointInfo>&>
  GetOperatingPoints();
  zx_status_t SetCurrentOperatingPoint(uint32_t new_opp);
  zx_status_t SetOperatingPointLimits(uint32_t min_opp, uint32_t max_opp);
  std::tuple<zx_status_t, uint32_t, uint32_t> GetCurrentOperatingPointLimits();

 protected:
  // Don't allow explicit construction.
  explicit CpuPerformanceDomain(fidl::ClientEnd<cpuctrl::Device> cpu_client)
      : cpu_client_(std::move(cpu_client)) {}

 private:
  fidl::WireSyncClient<cpuctrl::Device> cpu_client_;

  // Don't use this directly. Instead call GetOperatingPoints().
  std::vector<cpuctrl::wire::CpuOperatingPointInfo> cached_opps_;
};

#endif  // SRC_DEVICES_CPU_BIN_CPUCTL_PERFORMANCE_DOMAIN_H_
