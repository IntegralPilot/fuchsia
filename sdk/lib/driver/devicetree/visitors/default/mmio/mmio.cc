// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "lib/driver/devicetree/visitors/default/mmio/mmio.h"

#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/devicetree/devicetree.h>
#include <lib/driver/logging/cpp/logger.h>

namespace fdf {
using namespace fuchsia_driver_framework;
}

namespace fdf_devicetree {

constexpr const char kMmioProp[] = "reg";
constexpr const char kMmioNamesProp[] = "reg-names";
constexpr const char kMemoryRegionProp[] = "memory-region";
constexpr const char kMemoryRegionNamesProp[] = "memory-region-names";

MmioVisitor::MmioVisitor() {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(
      std::make_unique<fdf_devicetree::StringListProperty>(kMmioNamesProp, /* required */ false));
  properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kMemoryRegionProp, 0u, /* required */ false));
  properties.emplace_back(std::make_unique<fdf_devicetree::StringListProperty>(
      kMemoryRegionNamesProp, /* required */ false));
  mmio_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

zx::result<> MmioVisitor::RegPropertyParser(Node& node,
                                            fdf_devicetree::ParsedProperties& parsed_props,
                                            const devicetree::PropertyDecoder& decoder) {
  auto property = node.properties().find(kMmioProp);
  if (property == node.properties().end()) {
    fdf::debug("Node '{}' has no reg properties.", node.name());

    return zx::ok();
  }

  // Make sure value is a register array.
  auto reg_props = property->second.AsReg(decoder);
  if (reg_props == std::nullopt) {
    fdf::warn("Node '{}' has invalid reg property.", node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto mmio_names = parsed_props.Get<std::vector<std::string>>(kMmioNamesProp);
  if (mmio_names && mmio_names->size() > reg_props->size()) {
    fdf::error("Node '{}' has {} reg entries but has {} reg names.", node.name(), reg_props->size(),
               mmio_names->size());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  for (uint32_t i = 0; i < reg_props->size(); i++) {
    if ((*reg_props)[i].size()) {
      fuchsia_hardware_platform_bus::Mmio mmio;
      mmio.base() = decoder.TranslateAddress(*(*reg_props)[i].address());
      mmio.length() = (*reg_props)[i].size();
      if (mmio_names && i < mmio_names->size()) {
        mmio.name() = (*mmio_names)[i];
      }
      node_mmios_[node.id()].push_back(std::move(mmio));
    } else {
      fdf::debug("Node '{}' reg is not mmio.", node.name());

      break;
    }
  }

  return zx::ok();
}

zx::result<> MmioVisitor::MemoryRegionParser(Node& node,
                                             fdf_devicetree::ParsedProperties& parsed_props) {
  auto memory_regions = parsed_props.Get<References>(kMemoryRegionProp);
  if (!memory_regions) {
    return zx::ok();
  }

  auto memory_region_names = parsed_props.Get<std::vector<std::string>>(kMemoryRegionNamesProp);
  if (memory_region_names && memory_region_names->size() > memory_regions->size()) {
    fdf::error("Node '{}' has {} memory-region entries but has {} memory-region names.",
               node.name(), memory_regions->size(), memory_region_names->size());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  for (uint32_t index = 0; index < memory_regions->size(); index++) {
    auto& reference = (*memory_regions)[index];
    std::optional<std::string> name;
    if (memory_region_names && index < memory_region_names->size()) {
      name = (*memory_region_names)[index];
    }
    std::pair<Node*, std::optional<std::string>> reference_info = {&node, name};
    memory_region_nodes_[reference.reference_node().id()].push_back(std::move(reference_info));
  }

  return zx::ok();
}

zx::result<> MmioVisitor::Visit(Node& node, const devicetree::PropertyDecoder& decoder) {
  // Non-MMIO nodes (e.g. PCI devices, whose parent visitor modifies their register type
  // during the pre-order traversal) are skipped. This prevents the MMIO visitor from
  // attempting to parse differently-packed reg properties (such as 3-cell PCI addresses),
  // which would otherwise cause parsing failures.
  if (node.register_type() != RegisterType::kMmio) {
    return zx::ok();
  }

  auto parser_output = mmio_parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Mmio visitor failed for node '{}' : {}", node.name(), parser_output.status_value());

    return parser_output.take_error();
  }

  zx::result result = RegPropertyParser(node, *parser_output, decoder);
  if (!result.is_ok()) {
    return result;
  }

  result = MemoryRegionParser(node, *parser_output);
  if (!result.is_ok()) {
    return result;
  }

  return zx::ok();
}

zx::result<> MmioVisitor::FinalizeNode(Node& node) {
  if (node.register_type() != RegisterType::kMmio ||
      node_mmios_.find(node.id()) == node_mmios_.end()) {
    return zx::ok();
  }

  if (memory_region_nodes_.find(node.id()) != memory_region_nodes_.end()) {
    for (auto& memory_region : memory_region_nodes_[node.id()]) {
      fdf_devicetree::Node* referee = memory_region.first;
      for (auto&& mmio : node_mmios_[node.id()]) {
        fdf::debug("Memory region [{:#x}, {:#x}) added to node '{}'.", *mmio.base(),
                   *mmio.base() + *mmio.length(), referee->name());

        fuchsia_hardware_platform_bus::Mmio referee_mmio = mmio;
        if (memory_region.second) {
          referee_mmio.name() = *memory_region.second;
        }
        // Add the mmio to the referee.
        referee->AddMmio(std::move(referee_mmio));
      }
    }
  } else {
    for (auto&& mmio : node_mmios_[node.id()]) {
      fdf::debug("MMIO [{:#x}, {:#x}) added to node '{}'.", *mmio.base(),
                 *mmio.base() + *mmio.length(), node.name());

      node.AddMmio(std::move(mmio));
    }
  }

  return zx::ok();
}

}  // namespace fdf_devicetree
