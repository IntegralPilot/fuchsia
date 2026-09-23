// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/cpu/drivers/aml-cpu/aml-cpu-driver.h"

#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/logging/cpp/logger.h>

#include <soc/aml-common/aml-cpu-metadata.h>

namespace amlogic_cpu {

zx::result<> AmlCpuDriver::Start(fdf::DriverContext context) {
  component_inspector_.emplace(context.CreateInspector(this));
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  auto pdev_client = incoming->Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (pdev_client.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client);
    return zx::error(pdev_client.take_error());
  }
  fdf::PDev pdev(std::move(pdev_client.value()));

  zx::result metadata = pdev.GetFidlMetadata<fuchsia_hardware_amlogic_metadata::CpuMetadata>();
  if (metadata.is_error()) {
    fdf::error("Failed to get metadata: {}", metadata);
    return metadata.take_error();
  }
  if (!metadata->performance_domains().has_value()) {
    fdf::error("Metadata missing `performance_domains` field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (!metadata->operating_points().has_value()) {
    fdf::error("Metadata missing `operating_points` field");
    return zx::error(ZX_ERR_INTERNAL);
  }

  zx::result config = LoadConfiguration(pdev);
  if (config.is_error()) {
    fdf::error("Failed to load cpu configuration: {}", config);
    return config.take_error();
  }

  // Build and publish each performance domain.
  for (const fuchsia_hardware_amlogic_metadata::PerformanceDomain& perf_domain :
       metadata->performance_domains().value()) {
    // Vector of operating points that belong to this power domain.
    std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint> pd_op_points =
        PerformanceDomainOpPoints(perf_domain, metadata->operating_points().value());
    auto device = BuildPerformanceDomain(perf_domain, pd_op_points, config.value(), incoming);
    if (device.is_error()) {
      fdf::error("Failed to build performance domain node: {}", device);
      return zx::error(device.error_value());
    }

    fuchsia_hardware_cpu_ctrl::Service::InstanceHandler handler({
        .device = device->GetHandler(dispatcher()),
    });

    auto result = outgoing()->AddService<fuchsia_hardware_cpu_ctrl::Service>(std::move(handler),
                                                                             device->GetName());
    if (result.is_error()) {
      fdf::error("Failed to add service: {}", result);
      return result.take_error();
    }

    performance_domains_.push_back(std::move(device.value()));
  }

  return zx::ok();
}

zx::result<std::unique_ptr<AmlCpuPerformanceDomain>> AmlCpuDriver::BuildPerformanceDomain(
    fuchsia_hardware_amlogic_metadata::PerformanceDomain perf_domain,
    std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint> pd_op_points,
    const AmlCpuConfiguration& config, const std::shared_ptr<fdf::Namespace>& incoming) {
  char fragment_name[32];
  fidl::ClientEnd<fuchsia_hardware_clock::Clock> pll_div16_client;
  fidl::ClientEnd<fuchsia_hardware_clock::Clock> cpu_div16_client;
  if (config.has_div16_clients) {
    const char* pll_name = perf_domain.id() == 1 ? "sys-pll-div16" : "sys-pllb-div16";
    zx::result pll_clock_client =
        incoming->Connect<fuchsia_hardware_clock::Service::Clock>(pll_name);
    if (pll_clock_client.is_error()) {
      fdf::error("Failed to get clock protocol from fragment '{}': {}\n", pll_name,
                 pll_clock_client);
      return zx::error(pll_clock_client.status_value());
    }
    pll_div16_client = std::move(*pll_clock_client);

    const char* cpu_div16_name = perf_domain.id() == 1 ? "sys-cpu-div16" : "sys-cpub-div16";
    zx::result cpu_clock_client =
        incoming->Connect<fuchsia_hardware_clock::Service::Clock>(cpu_div16_name);
    if (cpu_clock_client.is_error()) {
      fdf::error("Failed to get clock protocol from fragment '{}': {}\n", cpu_div16_name,
                 cpu_clock_client);
      return zx::error(cpu_clock_client.status_value());
    }
    cpu_div16_client = std::move(*cpu_clock_client);
  }

  const char* scaler_name = perf_domain.id() == 1 ? "sys-cpu-big-clk" : "sys-cpu-little-clk";
  zx::result clock_client = incoming->Connect<fuchsia_hardware_clock::Service::Clock>(scaler_name);
  if (clock_client.is_error()) {
    fdf::error("Failed to get clock protocol from fragment '{}': {}\n", scaler_name, clock_client);
    return zx::error(clock_client.status_value());
  }
  fidl::ClientEnd<fuchsia_hardware_clock::Clock> cpu_scaler_client{std::move(*clock_client)};

  // For A1, the CPU power is VDD_CORE, which share with other module.
  // The fixed voltage is 0.8v, we can't adjust it dynamically.
  fidl::ClientEnd<fuchsia_hardware_power::Device> power_client;
  if (config.has_power_client) {
    snprintf(fragment_name, sizeof(fragment_name), "power-%02d", perf_domain.id());
    zx::result client_end_result =
        incoming->Connect<fuchsia_hardware_power::Service::Device>(fragment_name);
    if (client_end_result.is_error()) {
      fdf::error("Failed to create power client, st = {}", client_end_result);
      return zx::error(client_end_result.error_value());
    }

    power_client = std::move(client_end_result.value());
  }

  auto device = std::make_unique<AmlCpuPerformanceDomain>(dispatcher(), std::move(pd_op_points),
                                                          std::move(perf_domain),
                                                          component_inspector_->inspector());

  auto st = device->Init(std::move(pll_div16_client), std::move(cpu_div16_client),
                         std::move(cpu_scaler_client), std::move(power_client));
  if (st != ZX_OK) {
    fdf::error("Failed to initialize device: {}", zx_status_get_string(st));
    return zx::error(st);
  }

  device->SetCpuInfo(config.cpu_version_packed);

  return zx::ok(std::move(device));
}

}  // namespace amlogic_cpu

FUCHSIA_DRIVER_EXPORT2(amlogic_cpu::AmlCpuDriver);
