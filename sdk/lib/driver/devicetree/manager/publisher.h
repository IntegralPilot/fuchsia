// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_MANAGER_PUBLISHER_H_
#define LIB_DRIVER_DEVICETREE_MANAGER_PUBLISHER_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/zx/result.h>

#include <optional>
#include <string>

namespace fdf_devicetree {

struct BoardChildNode {
  std::string name;
  std::vector<fuchsia_driver_framework::NodeProperty2> properties;
  std::optional<std::string> driver_host;
  std::optional<fuchsia_driver_framework::BusInfo> bus_info;
};

class PublisherInterface {
 public:
  virtual ~PublisherInterface() = default;
  virtual zx::result<> AddPbusNode(
      fuchsia_hardware_platform_bus::Node& pbus_node,
      std::vector<std::optional<std::string>> metadata_text = {},
      std::vector<std::optional<std::string>> power_config_text = {}) = 0;
  virtual zx::result<> AddBoardChildNode(BoardChildNode args) = 0;
  virtual zx::result<> AddCompositeNodeSpec(
      std::string name, std::vector<fuchsia_driver_framework::ParentSpec2> parents,
      std::optional<std::string> driver_host = std::nullopt) = 0;
  virtual zx::result<> RegisterIommu(uint32_t iommu_id,
                                     fuchsia_hardware_platform_bus::Iommu iommu) = 0;
};

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_MANAGER_PUBLISHER_H_
