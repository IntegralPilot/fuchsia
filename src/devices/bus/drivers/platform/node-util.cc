// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bus/drivers/platform/node-util.h"

#include <optional>
#include <string>

namespace platform_bus {

namespace {

// Most drivers only have a few of each resource, so we have just kept these as
// linear searches for simplicity. We can consider pre-constructing a map for
// each resource and storing it with the platform device if performance is an issue.

template <typename ResourceType>
std::optional<uint32_t> GetResourceIndex(
    const std::optional<std::vector<ResourceType>>& node_resources,
    std::string_view node_resource_name) {
  if (!node_resources.has_value()) {
    return std::nullopt;
  }

  const std::vector<ResourceType>& resources = node_resources.value();
  for (size_t resource_index = 0; resource_index < resources.size(); ++resource_index) {
    const ResourceType& resource = resources[resource_index];

    // It's valid for resources not to have names. Unnamed resources are
    // inaccessible using Get*ByName(), and can only be retrieved by Get*ById().
    const std::optional<std::string>& resource_name = resource.name();
    if (!resource_name.has_value()) {
      continue;
    }

    if (resource_name.value() == node_resource_name) {
      return resource_index;
    }
  }
  return std::nullopt;
}

}  // namespace

// Returns the index of the mmio that matches |mmio_name|.
std::optional<uint32_t> GetMmioIndex(const fuchsia_hardware_platform_bus::Node& node,
                                     std::string_view mmio_name) {
  return GetResourceIndex(node.mmio(), mmio_name);
}

// Returns the index of the irq that matches |irq_name|.
std::optional<uint32_t> GetIrqIndex(const fuchsia_hardware_platform_bus::Node& node,
                                    std::string_view irq_name) {
  return GetResourceIndex(node.irq(), irq_name);
}

// Returns the index of the bti that matches |bti_name|.
std::optional<uint32_t> GetBtiIndex(const fuchsia_hardware_platform_bus::Node& node,
                                    std::string_view bti_name) {
  return GetResourceIndex(node.bti(), bti_name);
}

// Returns the index of the smc that matches |smc_name|.
std::optional<uint32_t> GetSmcIndex(const fuchsia_hardware_platform_bus::Node& node,
                                    std::string_view smc_name) {
  return GetResourceIndex(node.smc(), smc_name);
}

}  // namespace platform_bus
