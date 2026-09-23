// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_CPU_DRIVERS_AML_CPU_AML_CPU_H_
#define SRC_DEVICES_CPU_DRIVERS_AML_CPU_AML_CPU_H_

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.amlogic.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <fidl/fuchsia.hardware.cpu.ctrl/cpp/wire.h>
#include <fidl/fuchsia.hardware.power/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/inspect/cpp/inspector.h>

#include <mutex>
#include <vector>

#include <soc/aml-common/aml-cpu-metadata.h>

namespace amlogic_cpu {

// Fragments are provided to this driver in groups of 4. Fragments are provided as follows:
// [4 fragments for cluster 0]
// [4 fragments for cluster 1]
// [...]
// [4 fragments for cluster n]
// Each fragment is a combination of the fixed string + id.
constexpr size_t kFragmentsPerPfDomain = 4;
constexpr size_t kFragmentsPerPfDomainA5 = 2;
constexpr size_t kFragmentsPerPfDomainA1 = 1;

constexpr zx_off_t kCpuVersionOffset = 0x220;
constexpr zx_off_t kCpuVersionOffsetA5 = 0x300;
constexpr zx_off_t kCpuVersionOffsetA1 = 0x220;

struct AmlCpuConfiguration {
  fdf::PDev::DeviceInfo info;
  size_t fragments_per_pf_domain;
  uint32_t cpu_version_packed;

  bool has_div16_clients;
  bool has_power_client;
};

class AmlCpu : public fidl::WireServer<fuchsia_hardware_cpu_ctrl::Device> {
 public:
  explicit AmlCpu(std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint> operating_points,
                  fuchsia_hardware_amlogic_metadata::PerformanceDomain perf_domain,
                  inspect::Inspector& inspect)
      : current_operating_point_(
            static_cast<uint32_t>(operating_points.size() -
                                  1))  // Assume the core is running at the slowest clock to begin.
        ,
        operating_points_(std::move(operating_points)),
        perf_domain_(std::move(perf_domain)),
        inspect_(inspect) {}

  zx_status_t Init(fidl::ClientEnd<fuchsia_hardware_clock::Clock> plldiv16,
                   fidl::ClientEnd<fuchsia_hardware_clock::Clock> cpudiv16,
                   fidl::ClientEnd<fuchsia_hardware_clock::Clock> cpuscaler,
                   fidl::ClientEnd<fuchsia_hardware_power::Device> pwr);

  zx_status_t SetCurrentOperatingPointInternal(uint32_t requested_opp, uint32_t* out_opp);

  // Set CpuInfo in inspect.
  void SetCpuInfo(uint32_t cpu_version_packed);

  uint32_t GetCurrentOperatingPoint() {
    std::scoped_lock lock(lock_);
    return current_operating_point_;
  }

  uint32_t GetOperatingPointCount() const {
    return static_cast<uint32_t>(operating_points_.size());
  }

  const std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint>& GetOperatingPoints() {
    return operating_points_;
  }

  uint32_t GetCoreCount() const { return perf_domain_.core_count(); }
  PerfDomainId GetDomainId() const { return perf_domain_.id(); }
  std::string_view GetName() const { return perf_domain_.name(); }

  // Fidl server interface implementation.
  void GetOperatingPointInfo(GetOperatingPointInfoRequestView request,
                             GetOperatingPointInfoCompleter::Sync& completer) override;
  void SetCurrentOperatingPoint(SetCurrentOperatingPointRequestView request,
                                SetCurrentOperatingPointCompleter::Sync& completer) override;
  void GetCurrentOperatingPoint(GetCurrentOperatingPointCompleter::Sync& completer) override;

  void SetMinimumOperatingPointLimit(
      SetMinimumOperatingPointLimitRequestView request,
      SetMinimumOperatingPointLimitCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void SetMaximumOperatingPointLimit(
      SetMaximumOperatingPointLimitRequestView request,
      SetMaximumOperatingPointLimitCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void SetOperatingPointLimits(SetOperatingPointLimitsRequestView request,
                               SetOperatingPointLimitsCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void GetCurrentOperatingPointLimits(
      GetCurrentOperatingPointLimitsCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

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

  inspect::Inspector& Inspector() { return inspect_; }

 private:
  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> plldiv16_;
  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> cpudiv16_;
  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> cpuscaler_;

  // This is from an optional fragment.
  fidl::WireSyncClient<fuchsia_hardware_power::Device> pwr_;

  std::mutex lock_;
  uint32_t current_operating_point_ __TA_GUARDED(lock_);
  const std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint> operating_points_;

  fuchsia_hardware_amlogic_metadata::PerformanceDomain perf_domain_;

  inspect::Inspector& inspect_;
  inspect::Node cpu_info_ = inspect_.GetRoot().CreateChild("cpu_info_service");

  inspect::UintProperty inspect_major_revision_;
  inspect::UintProperty inspect_minor_revision_;
  inspect::UintProperty inspect_package_id_;
};

std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint> PerformanceDomainOpPoints(
    const fuchsia_hardware_amlogic_metadata::PerformanceDomain& perf_domain,
    std::span<const fuchsia_hardware_amlogic_metadata::OperatingPoint> op_points);
zx_status_t GetPopularVoltageTable(const zx::resource& smc_resource);
zx::result<AmlCpuConfiguration> LoadConfiguration(fdf::PDev& pdev);

}  // namespace amlogic_cpu

#endif  // SRC_DEVICES_CPU_DRIVERS_AML_CPU_AML_CPU_H_
