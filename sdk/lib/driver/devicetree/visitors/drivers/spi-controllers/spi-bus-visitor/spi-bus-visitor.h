// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_SPI_CONTROLLERS_SPI_BUS_VISITOR_SPI_BUS_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_SPI_CONTROLLERS_SPI_BUS_VISITOR_SPI_BUS_VISITOR_H_

#include <fidl/fuchsia.hardware.spi.businfo/cpp/fidl.h>
#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <cstdint>

namespace spi_bus_dt {

class SpiBusVisitor : public fdf_devicetree::Visitor {
 public:
  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;

  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  struct SpiController {
    std::vector<fuchsia_hardware_spi_businfo::SpiChannel> channels;
    uint32_t bus_id;
  };

  // Create new instance of SpiController, returns error if one already exists for the node_name.
  zx::result<> CreateController(const std::string& node_name);

  void AddChildNodeSpec(fdf_devicetree::ChildNode& child, uint32_t global_id);

  zx::result<> ParseChild(SpiController& controller, fdf_devicetree::Node& parent,
                          fdf_devicetree::ChildNode& child);

  static bool is_match(fdf_devicetree::Node& node);

  // Mapping of devicetree node name to SPI controller struct.
  std::map<std::string, SpiController> spi_controllers_;
  uint32_t bus_id_counter_ = 0;
  uint32_t channel_id_counter_ = 0;
};

}  // namespace spi_bus_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_SPI_CONTROLLERS_SPI_BUS_VISITOR_SPI_BUS_VISITOR_H_
