// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/driver/devicetree/manager/publisher-dev.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/status.h>

namespace fdf_devicetree {

zx::result<> PublisherDev::AddPbusNode(fuchsia_hardware_platform_bus::Node& pbus_node,
                                       std::vector<std::optional<std::string>> metadata_text,
                                       std::vector<std::optional<std::string>> power_config_text) {
  fdf::Arena arena('PBUS');
  fidl::Arena fidl_arena;
  auto result = pbus_.buffer(arena)->NodeAdd(fidl::ToWire(fidl_arena, pbus_node));
  if (!result.ok()) {
    fdf::error("NodeAdd request failed: {}", result.FormatDescription());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    fdf::error("NodeAdd failed: {}", result->error_value());
    return zx::error(result->error_value());
  }
  return zx::ok();
}

zx::result<> PublisherDev::AddBoardChildNode(BoardChildNode args) {
  fuchsia_driver_framework::NodeAddArgs node_add_args;
  node_add_args.name() = std::move(args.name);
  if (args.driver_host) {
    node_add_args.driver_host() = *args.driver_host;
  }

  node_add_args.properties2() = std::move(args.properties);
  if (args.bus_info) {
    node_add_args.bus_info() = std::move(*args.bus_info);
  }
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  auto result = fdf_node_->AddChild({std::move(node_add_args), std::move(server_end), {}});
  if (result.is_error()) {
    fdf::error("AddChild request failed: {}", result.error_value().FormatDescription());
    return zx::error(ZX_ERR_INTERNAL);
  }
  return zx::ok();
}

zx::result<> PublisherDev::AddCompositeNodeSpec(
    std::string name, std::vector<fuchsia_driver_framework::ParentSpec2> parents,
    std::optional<std::string> driver_host) {
  fuchsia_driver_framework::CompositeNodeSpec group{{
      .name = name,
      .parents2 = std::move(parents),
  }};
  if (driver_host) {
    group.driver_host() = *driver_host;
  }

  auto devicegroup_result = mgr_->AddSpec({std::move(group)});
  if (devicegroup_result.is_error()) {
    fdf::error("Failed to create composite node: {}",
               devicegroup_result.error_value().FormatDescription());
    return zx::error(devicegroup_result.error_value().is_framework_error()
                         ? devicegroup_result.error_value().framework_error().status()
                         : ZX_ERR_INVALID_ARGS);
  }
  return zx::ok();
}

zx::result<> PublisherDev::RegisterIommu(uint32_t iommu_id,
                                         fuchsia_hardware_platform_bus::Iommu iommu) {
  fdf::Arena arena('PBUS');
  auto iommu_wire = iommu.stub_iommu()
                        ? fuchsia_hardware_platform_bus::wire::Iommu::WithStubIommu({})
                        : fuchsia_hardware_platform_bus::wire::Iommu::WithArmSmmu(
                              arena, iommu.arm_smmu()->base_address());
  auto result = pbus_.buffer(arena)->RegisterIommu(iommu_id, iommu_wire);
  if (!result.ok()) {
    fdf::error("Failed to publish iommu: {} : {}", iommu_id, result.FormatDescription());
    return zx::error(result.status());
  }

  if (result->is_error()) {
    fdf::error("Failed to publish iommu: {} : {}", iommu_id, result->error_value());
    return zx::error(result->error_value());
  }
  return zx::ok();
}

}  // namespace fdf_devicetree
