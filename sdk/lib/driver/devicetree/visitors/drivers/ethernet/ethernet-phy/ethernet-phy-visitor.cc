// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ethernet-phy-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/cpp/bind.h>

namespace eth_phy_visitor_dt {

EthPhyVisitor::EthPhyVisitor() {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(
      std::make_unique<fdf_devicetree::ReferenceProperty>(kPhys, kPhyCells, /* required */ false));
  parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

bool EthPhyVisitor::is_match(const std::string& name) {
  return name.find("ethernet-phy") != std::string::npos;
}

zx::result<> EthPhyVisitor::Visit(fdf_devicetree::Node& node,
                                  const devicetree::PropertyDecoder& decoder) {
  auto parser_output = parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Ethernet phy visitor parse failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  // ethernet-phy references only have one entry.
  auto references = parser_output->Get<fdf_devicetree::References>(kPhys);
  if (!references || references->size() != 1u) {
    return zx::ok();
  }

  if (!is_match(references->at(0).reference_node().name())) {
    // This reference is not to a ethernet-phy.
    return zx::ok();
  }

  auto result = AddChildNodeSpec(node);
  if (result.is_error()) {
    return result.take_error();
  }

  return zx::ok();
}

zx::result<> EthPhyVisitor::AddChildNodeSpec(fdf_devicetree::Node& child) {
  auto phy_node = fuchsia_driver_framework::ParentSpec2{
      {.bind_rules =
           {
               fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE,
                                       "fuchsia.hardware.ethernet.board.Service"),
           },
       .properties = {
           fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.ethernet.board.Service"),
       }}};

  child.AddNodeSpec(phy_node);

  fdf::debug("Added ethernet phy bind rules to node '{}'.", child.name());

  return zx::ok();
}

}  // namespace eth_phy_visitor_dt

REGISTER_DEVICETREE_VISITOR(eth_phy_visitor_dt::EthPhyVisitor);
