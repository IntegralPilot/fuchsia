// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_MMIO_MMIO_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_MMIO_MMIO_H_

#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <memory>

namespace fdf_devicetree {

// The |MmioVisitor| populates the mmio properties of each device tree
// node based on the "reg" property or the `memory-region` property.
class MmioVisitor : public Visitor {
 public:
  MmioVisitor();
  ~MmioVisitor() override = default;
  zx::result<> Visit(Node& node, const devicetree::PropertyDecoder& decoder) override;
  zx::result<> FinalizeNode(Node& node) override;

 private:
  zx::result<> RegPropertyParser(Node& node, fdf_devicetree::ParsedProperties& parsed_props,
                                 const devicetree::PropertyDecoder& decoder);
  zx::result<> MemoryRegionParser(Node& node, fdf_devicetree::ParsedProperties& parsed_props);

  std::unique_ptr<fdf_devicetree::PropertyParser> mmio_parser_;
  // Map of nodes and their mmios derived from reg property.
  std::map<NodeID, std::vector<fuchsia_hardware_platform_bus::Mmio>> node_mmios_;
  // Map of nodes whose mmio is referenced by another node using `memory-region` property.
  // The std::pair is used to store the mmio name associated with the reference if exists.
  std::map<
      NodeID /*reference*/,
      std::vector<std::pair<Node* /*referee*/, std::optional<std::string> /*memory-region name*/>>>
      memory_region_nodes_;
};

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_MMIO_MMIO_H_
