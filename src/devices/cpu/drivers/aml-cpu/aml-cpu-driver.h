// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_CPU_DRIVERS_AML_CPU_AML_CPU_DRIVER_H_
#define SRC_DEVICES_CPU_DRIVERS_AML_CPU_AML_CPU_DRIVER_H_

#include <fidl/fuchsia.hardware.amlogic.metadata/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/fit/function.h>

#include "src/devices/cpu/drivers/aml-cpu/aml-cpu.h"

namespace amlogic_cpu {

class AmlCpuPerformanceDomain : public AmlCpu {
 public:
  AmlCpuPerformanceDomain(
      async_dispatcher_t* dispatcher,
      std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint> operating_points,
      fuchsia_hardware_amlogic_metadata::PerformanceDomain perf_domain, inspect::Inspector& inspect)
      : AmlCpu(std::move(operating_points), std::move(perf_domain), inspect) {}

  fidl::ProtocolHandler<fuchsia_hardware_cpu_ctrl::Device> GetHandler(
      async_dispatcher_t* dispatcher) {
    return bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure);
  }

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_cpu_ctrl::Device> bindings_;
};

class AmlCpuDriver : public fdf::DriverBase2 {
 public:
  explicit AmlCpuDriver() : fdf::DriverBase2("aml-cpu") {}

  zx::result<> Start(fdf::DriverContext context) override;

  zx::result<std::unique_ptr<AmlCpuPerformanceDomain>> BuildPerformanceDomain(
      fuchsia_hardware_amlogic_metadata::PerformanceDomain perf_domain,
      std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint> pd_op_points,
      const AmlCpuConfiguration& config, const std::shared_ptr<fdf::Namespace>& incoming);
  std::vector<std::unique_ptr<AmlCpuPerformanceDomain>>& performance_domains() {
    return performance_domains_;
  }

 protected:
 private:
  std::optional<inspect::ComponentInspector> component_inspector_;
  std::vector<std::unique_ptr<AmlCpuPerformanceDomain>> performance_domains_;
};

}  // namespace amlogic_cpu

#endif  // SRC_DEVICES_CPU_DRIVERS_AML_CPU_AML_CPU_DRIVER_H_
