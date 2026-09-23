// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_CPU_CTRL_SERVER_H_
#define SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_CPU_CTRL_SERVER_H_

#include <fidl/fuchsia.hardware.cpu.ctrl/cpp/wire.h>

#include <array>
#include <mutex>

namespace fake_driver {
using operating_point_t = struct operating_point {
  uint32_t freq_hz;
  uint32_t volt_uv;
};

// Protocol served to client components over devfs.
class CpuCtrlProtocolServer : public fidl::WireServer<fuchsia_hardware_cpu_ctrl::Device> {
 public:
  explicit CpuCtrlProtocolServer();

  // Fidl server interface implementation.
  void GetOperatingPointInfo(GetOperatingPointInfoRequestView request,
                             GetOperatingPointInfoCompleter::Sync& completer) override;
  void SetCurrentOperatingPoint(SetCurrentOperatingPointRequestView request,
                                SetCurrentOperatingPointCompleter::Sync& completer) override;
  void GetCurrentOperatingPoint(GetCurrentOperatingPointCompleter::Sync& completer) override;

  void SetMinimumOperatingPointLimit(
      SetMinimumOperatingPointLimitRequestView request,
      SetMinimumOperatingPointLimitCompleter::Sync& completer) override;
  void SetMaximumOperatingPointLimit(
      SetMaximumOperatingPointLimitRequestView request,
      SetMaximumOperatingPointLimitCompleter::Sync& completer) override;
  void SetOperatingPointLimits(SetOperatingPointLimitsRequestView request,
                               SetOperatingPointLimitsCompleter::Sync& completer) override;

  void GetCurrentOperatingPointLimits(
      GetCurrentOperatingPointLimitsCompleter::Sync& completer) override;

  void GetOperatingPointCount(GetOperatingPointCountCompleter::Sync& completer) override;
  void GetNumLogicalCores(GetNumLogicalCoresCompleter::Sync& completer) override;
  void GetLogicalCoreId(GetLogicalCoreIdRequestView request,
                        GetLogicalCoreIdCompleter::Sync& completer) override;
  void GetDomainId(GetDomainIdCompleter::Sync& completer) override;
  void GetRelativePerformance(GetRelativePerformanceCompleter::Sync& completer) override;
  void GetRelativePerformance2(GetRelativePerformance2Completer::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_cpu_ctrl::Device> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  void Serve(async_dispatcher_t* dispatcher,
             fidl::ServerEnd<fuchsia_hardware_cpu_ctrl::Device> server);

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_cpu_ctrl::Device> bindings_;

  static constexpr std::array<operating_point_t, 3> kOperatingPoints = {{
      {.freq_hz = static_cast<uint32_t>(2.0e9), .volt_uv = static_cast<uint32_t>(1.0 * 1e6)},
      {.freq_hz = static_cast<uint32_t>(1.5e9), .volt_uv = static_cast<uint32_t>(0.8 * 1e6)},
      {.freq_hz = static_cast<uint32_t>(1.5e9), .volt_uv = static_cast<uint32_t>(0.7 * 1e6)},
  }};

  static constexpr std::array<uint64_t, 4> kLogicalCoreIds = {{0, 1, 2, 3}};

  std::mutex lock_;
  uint32_t current_opp_ __TA_GUARDED(lock_) = 0;
  uint32_t minimum_opp_ __TA_GUARDED(lock_) = kOperatingPoints.size() - 1;
  uint32_t maximum_opp_ __TA_GUARDED(lock_) = 0;
};

}  // namespace fake_driver

#endif  // SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_CPU_CTRL_SERVER_H_
